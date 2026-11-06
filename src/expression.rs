//! Expression parsing & evaluation for Markdoc tag attributes.
//!
//! Used by the transformer to resolve `Node.expressions` (raw source for
//! variables and function calls captured at parse time) into `Scalar`
//! values that get merged into the rendered tree's attribute map.
//!
//! The grammar is intentionally tiny:
//!
//! ```text
//! Expression  := Variable | Function | Literal
//! Variable    := '$' IDENT ('.' IDENT)*
//! Function    := IDENT '(' (Expression (',' Expression)*)? ')'
//! Literal     := QuotedString | Number | 'true' | 'false' | 'null'
//! QuotedString:= '"' ... '"' | "'" ... "'"
//! Number      := /-?\d+(\.\d+)?/
//! IDENT       := /[A-Za-z_][A-Za-z0-9_]*/
//! ```
//!
//! No operator precedence, no infix syntax — Markdoc style.

use crate::functions::{FunctionImpl, default_functions};
use crate::types::*;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub enum Expression {
    Literal(Scalar),
    Variable(Vec<String>),
    Function {
        name: String,
        args: Vec<Expression>,
    },
    Array(Vec<Expression>),
    Object(Vec<(String, Expression)>),
    /// A string literal carrying `{$var}` interpolation spans, e.g.
    /// `"https://x/{$markdoc.frontmatter.documentNumber}"`. Evaluates by
    /// stringifying each part and concatenating (whole numbers without a
    /// trailing `.0`), so a value can be spliced into a URL without a
    /// `concat(...)` call.
    Interpolate(Vec<Expression>),
}

/// Evaluate an expression against a context using the given function impls.
pub fn evaluate(
    expr: &Expression,
    ctx: &Context,
    fns: &HashMap<String, FunctionImpl>,
) -> Result<Scalar> {
    match expr {
        Expression::Literal(s) => Ok(s.clone()),
        Expression::Variable(path) => Ok(ctx.resolve_variable(path)),
        Expression::Function { name, args } => {
            let resolved: Vec<Scalar> = args
                .iter()
                .map(|a| evaluate(a, ctx, fns))
                .collect::<Result<Vec<_>>>()?;
            let f = fns
                .get(name)
                .ok_or_else(|| MarkdocError::TransformError(format!("Unknown function: {name}")))?;
            f(&resolved)
        }
        Expression::Array(items) => {
            let resolved = items
                .iter()
                .map(|e| evaluate(e, ctx, fns))
                .collect::<Result<Vec<_>>>()?;
            Ok(Scalar::Array(resolved))
        }
        Expression::Object(pairs) => {
            let mut map = HashMap::new();
            for (k, e) in pairs {
                map.insert(k.clone(), evaluate(e, ctx, fns)?);
            }
            Ok(Scalar::Object(map))
        }
        Expression::Interpolate(parts) => {
            let mut out = String::new();
            for part in parts {
                out.push_str(&crate::functions::scalar_to_plain_string(&evaluate(
                    part, ctx, fns,
                )?));
            }
            Ok(Scalar::String(out))
        }
    }
}

/// Build a table of the built-ins with `extra` layered on top. Callers that
/// define their own functions pass the result to `evaluate`, or to the
/// `*_with_functions` entry points on `conditionals` and `transformer`.
///
/// Merged once per document rather than per expression: the map is small and
/// the alternative is a lookup closure threaded through every recursion.
pub fn functions_with(extra: &HashMap<String, FunctionImpl>) -> HashMap<String, FunctionImpl> {
    let mut merged = default_function_impls().clone();
    for (name, impl_) in extra {
        merged.insert(name.clone(), *impl_);
    }
    merged
}

/// Evaluate using the built-in default function table (equals/and/or/not).
pub fn evaluate_default(expr: &Expression, ctx: &Context) -> Result<Scalar> {
    evaluate(expr, ctx, default_function_impls())
}

/// Static map of the built-in function impls. Cheap to access; avoids
/// rebuilding the map per evaluation.
pub fn default_function_impls() -> &'static HashMap<String, FunctionImpl> {
    static IMPLS: OnceLock<HashMap<String, FunctionImpl>> = OnceLock::new();
    IMPLS.get_or_init(|| {
        default_functions()
            .into_iter()
            .map(|(k, (_sig, impl_))| (k, impl_))
            .collect()
    })
}

/// Parse an expression from source text.
pub fn parse_expression(source: &str) -> Result<Expression> {
    let mut p = Parser {
        src: source.as_bytes(),
        pos: 0,
    };
    let expr = p.parse_expr()?;
    p.skip_ws();
    if p.pos < p.src.len() {
        return Err(MarkdocError::ParseError(format!(
            "Unexpected trailing content in expression: {:?}",
            std::str::from_utf8(&p.src[p.pos..]).unwrap_or("<non-utf8>")
        )));
    }
    Ok(expr)
}

// ────────────────────────────────────────────────────────────────────────────
// Recursive-descent parser
// ────────────────────────────────────────────────────────────────────────────

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn parse_expr(&mut self) -> Result<Expression> {
        self.skip_ws();
        match self.peek() {
            Some(b'$') => self.parse_variable(),
            Some(b'"') | Some(b'\'') => self.parse_string(),
            Some(b'[') => self.parse_array(),
            Some(b'{') => self.parse_object(),
            Some(c) if c.is_ascii_digit() || c == b'-' => self.parse_number(),
            Some(c) if c.is_ascii_alphabetic() || c == b'_' => self.parse_identifier_or_call(),
            Some(c) => Err(MarkdocError::ParseError(format!(
                "Unexpected character {:?} at position {} in expression",
                c as char, self.pos
            ))),
            None => Err(MarkdocError::ParseError(
                "Unexpected end of expression".into(),
            )),
        }
    }

    fn parse_variable(&mut self) -> Result<Expression> {
        self.pos += 1; // consume `$`
        let first = self.parse_path_ident();
        if first.is_empty() {
            return Err(MarkdocError::ParseError("Empty variable name".into()));
        }
        let mut path = vec![first];
        // Trailing `.key` and `[index]` / `["key"]` accessors. An index
        // and a string key both reach `resolve_variable` as a path
        // segment; a numeric segment indexes an array, a name keys an
        // object.
        loop {
            match self.peek() {
                Some(b'.') => {
                    self.pos += 1;
                    let seg = self.parse_path_ident();
                    if seg.is_empty() {
                        return Err(MarkdocError::ParseError(
                            "Empty path segment after `.`".into(),
                        ));
                    }
                    path.push(seg);
                }
                Some(b'[') => {
                    self.pos += 1;
                    self.skip_ws();
                    let seg = match self.peek() {
                        Some(b'"') | Some(b'\'') => self.read_quoted()?,
                        Some(c) if c.is_ascii_digit() => {
                            let start = self.pos;
                            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                                self.pos += 1;
                            }
                            std::str::from_utf8(&self.src[start..self.pos])
                                .unwrap()
                                .to_string()
                        }
                        _ => {
                            return Err(MarkdocError::ParseError(
                                "Expected index or quoted key inside `[...]`".into(),
                            ));
                        }
                    };
                    self.skip_ws();
                    if self.peek() != Some(b']') {
                        return Err(MarkdocError::ParseError(
                            "Unterminated `[...]` accessor".into(),
                        ));
                    }
                    self.pos += 1; // consume `]`
                    path.push(seg);
                }
                _ => break,
            }
        }
        Ok(Expression::Variable(path))
    }

    /// Consume an identifier segment (`[A-Za-z0-9_]*`) and return it.
    fn parse_path_ident(&mut self) -> String {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        std::str::from_utf8(&self.src[start..self.pos])
            .unwrap()
            .to_string()
    }

    /// Read a quoted string's raw contents (without the surrounding quotes),
    /// consuming both quote characters. Used where a bare `String` is wanted
    /// (object keys, bracket accessors) — no interpolation.
    fn read_quoted(&mut self) -> Result<String> {
        let quote = self.bump().unwrap();
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c == quote {
                break;
            }
            self.pos += 1;
        }
        if self.peek() != Some(quote) {
            return Err(MarkdocError::ParseError("Unterminated string".into()));
        }
        let s = std::str::from_utf8(&self.src[start..self.pos])
            .map_err(|e| MarkdocError::ParseError(format!("Invalid UTF-8 in string: {e}")))?
            .to_string();
        self.pos += 1; // consume closing quote
        Ok(s)
    }

    fn parse_string(&mut self) -> Result<Expression> {
        let s = self.read_quoted()?;
        // A string carrying `{$var}` spans becomes an interpolation template;
        // a plain string stays a literal scalar.
        match split_interpolation(&s) {
            Some(parts) => Ok(Expression::Interpolate(parts)),
            None => Ok(Expression::Literal(Scalar::String(s))),
        }
    }

    fn parse_number(&mut self) -> Result<Expression> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || c == b'.' {
                self.pos += 1;
            } else {
                break;
            }
        }
        let s = std::str::from_utf8(&self.src[start..self.pos]).unwrap();
        s.parse::<f64>()
            .map(|n| Expression::Literal(Scalar::Number(n)))
            .map_err(|_| MarkdocError::ParseError(format!("Invalid number: {s}")))
    }

    fn parse_identifier_or_call(&mut self) -> Result<Expression> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        let name = std::str::from_utf8(&self.src[start..self.pos])
            .unwrap()
            .to_string();
        match name.as_str() {
            "true" => return Ok(Expression::Literal(Scalar::Boolean(true))),
            "false" => return Ok(Expression::Literal(Scalar::Boolean(false))),
            "null" => return Ok(Expression::Literal(Scalar::Null)),
            _ => {}
        }
        self.skip_ws();
        if self.peek() != Some(b'(') {
            return Err(MarkdocError::ParseError(format!(
                "Bare identifier {name:?}: expected `(` to begin function call"
            )));
        }
        self.pos += 1; // consume `(`
        let mut args = Vec::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(b')') {
                self.pos += 1;
                break;
            }
            args.push(self.parse_expr()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b')') => {
                    self.pos += 1;
                    break;
                }
                _ => {
                    return Err(MarkdocError::ParseError(
                        "Expected `,` or `)` in function call".into(),
                    ));
                }
            }
        }
        Ok(Expression::Function { name, args })
    }

    fn parse_array(&mut self) -> Result<Expression> {
        self.pos += 1; // consume `[`
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(b']') {
                self.pos += 1;
                break;
            }
            items.push(self.parse_expr()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    break;
                }
                _ => {
                    return Err(MarkdocError::ParseError(
                        "Expected `,` or `]` in array literal".into(),
                    ));
                }
            }
        }
        Ok(Expression::Array(items))
    }

    fn parse_object(&mut self) -> Result<Expression> {
        self.pos += 1; // consume `{`
        let mut pairs = Vec::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(b'}') {
                self.pos += 1;
                break;
            }
            let key = self.parse_object_key()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(MarkdocError::ParseError(
                    "Expected `:` after object key".into(),
                ));
            }
            self.pos += 1; // consume `:`
            let value = self.parse_expr()?;
            pairs.push((key, value));
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                _ => {
                    return Err(MarkdocError::ParseError(
                        "Expected `,` or `}` in object literal".into(),
                    ));
                }
            }
        }
        Ok(Expression::Object(pairs))
    }

    /// Object keys are bare identifiers (`id`) or quoted strings (`"id"`).
    fn parse_object_key(&mut self) -> Result<String> {
        match self.peek() {
            Some(b'"') | Some(b'\'') => self.read_quoted(),
            Some(c) if c.is_ascii_alphabetic() || c == b'_' => {
                let start = self.pos;
                while let Some(c) = self.peek() {
                    if c.is_ascii_alphanumeric() || c == b'_' || c == b'-' {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                Ok(std::str::from_utf8(&self.src[start..self.pos])
                    .unwrap()
                    .to_string())
            }
            _ => Err(MarkdocError::ParseError("Expected object key".into())),
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// String-literal interpolation — `"…{$var}…"`
// ────────────────────────────────────────────────────────────────────────────

/// Does this string literal carry a `{$…}` interpolation span? A cheap check
/// the attribute parser uses to route interpolating strings through
/// expression evaluation instead of storing them as plain literals.
pub(crate) fn contains_interpolation(s: &str) -> bool {
    match s.find("{$") {
        Some(i) => s[i..].contains('}'),
        None => false,
    }
}

/// Split a string literal into interpolation parts: each `{$var.path}` span
/// becomes a `Variable` expression and the surrounding text becomes `Literal`
/// parts. Returns `None` when there is no valid `{$…}` span, so the caller
/// keeps the whole string as one literal. Only `$variable` references
/// interpolate — a bare `{` or `{word}` stays literal — so the syntax never
/// collides with braces that appear literally in URLs or prose, and a
/// variable path (no quotes, no nested braces) always ends at the next `}`.
fn split_interpolation(s: &str) -> Option<Vec<Expression>> {
    if !s.contains("{$") {
        return None;
    }
    let mut parts: Vec<Expression> = Vec::new();
    let mut literal = String::new();
    let mut rest = s;
    let mut found = false;
    while let Some(open) = rest.find("{$") {
        if let Some(close_rel) = rest[open..].find('}') {
            let inner = rest[open + 1..open + close_rel].trim();
            if let Ok(expr @ Expression::Variable(_)) = parse_expression(inner) {
                literal.push_str(&rest[..open]);
                if !literal.is_empty() {
                    parts.push(Expression::Literal(Scalar::String(std::mem::take(
                        &mut literal,
                    ))));
                }
                parts.push(expr);
                found = true;
                rest = &rest[open + close_rel + 1..];
                continue;
            }
        }
        // Not a resolvable `{$…}` span — keep the `{` as a literal character
        // and carry on scanning after it.
        literal.push_str(&rest[..open + 1]);
        rest = &rest[open + 1..];
    }
    literal.push_str(rest);
    if !found {
        return None;
    }
    if !literal.is_empty() {
        parts.push(Expression::Literal(Scalar::String(literal)));
    }
    Some(parts)
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with(pairs: &[(&str, Scalar)]) -> Context {
        let mut c = Context::new();
        for (k, v) in pairs {
            c = c.with_variable(*k, v.clone());
        }
        c
    }

    #[test]
    fn parses_variable_path() {
        let e = parse_expression("$config.foo.bar").unwrap();
        match e {
            Expression::Variable(p) => assert_eq!(p, vec!["config", "foo", "bar"]),
            _ => panic!(),
        }
    }

    #[test]
    fn parses_function_call() {
        let e = parse_expression("equals($a, $b)").unwrap();
        match e {
            Expression::Function { name, args } => {
                assert_eq!(name, "equals");
                assert_eq!(args.len(), 2);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_nested_function_call() {
        let e = parse_expression(r#"and(equals($a, "x"), not($b))"#).unwrap();
        match e {
            Expression::Function { name, args } => {
                assert_eq!(name, "and");
                assert_eq!(args.len(), 2);
                assert!(matches!(args[0], Expression::Function { .. }));
                assert!(matches!(args[1], Expression::Function { .. }));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_literals() {
        assert!(matches!(
            parse_expression("true").unwrap(),
            Expression::Literal(Scalar::Boolean(true))
        ));
        assert!(matches!(
            parse_expression("42").unwrap(),
            Expression::Literal(Scalar::Number(_))
        ));
        assert!(matches!(
            parse_expression(r#""hello""#).unwrap(),
            Expression::Literal(Scalar::String(_))
        ));
    }

    #[test]
    fn rejects_trailing_garbage() {
        assert!(parse_expression("$a $b").is_err());
        assert!(parse_expression("equals($a, $b) trailing").is_err());
    }

    #[test]
    fn evaluates_variable_via_context() {
        let mut cfg = HashMap::new();
        cfg.insert(
            "severity".to_string(),
            Scalar::String("warning".to_string()),
        );
        let ctx = ctx_with(&[("config", Scalar::Object(cfg))]);
        let e = parse_expression("$config.severity").unwrap();
        let v = evaluate_default(&e, &ctx).unwrap();
        assert_eq!(v, Scalar::String("warning".to_string()));
    }

    #[test]
    fn evaluates_function_call_with_variables() {
        // equals($a, $b) where both variables resolve to "x"
        let mut top = HashMap::new();
        top.insert("a".to_string(), Scalar::String("x".to_string()));
        top.insert("b".to_string(), Scalar::String("x".to_string()));
        let ctx = Context { variables: top };
        let e = parse_expression("equals($a, $b)").unwrap();
        let v = evaluate_default(&e, &ctx).unwrap();
        assert_eq!(v, Scalar::Boolean(true));
    }

    #[test]
    fn nested_logical_evaluation() {
        // and(equals($a, $b), not($c)) — true && !false → true
        let mut top = HashMap::new();
        top.insert("a".to_string(), Scalar::String("x".to_string()));
        top.insert("b".to_string(), Scalar::String("x".to_string()));
        top.insert("c".to_string(), Scalar::Boolean(false));
        let ctx = Context { variables: top };
        let e = parse_expression("and(equals($a, $b), not($c))").unwrap();
        let v = evaluate_default(&e, &ctx).unwrap();
        assert_eq!(v, Scalar::Boolean(true));
    }

    #[test]
    fn missing_variable_resolves_to_null() {
        let ctx = Context::new();
        let e = parse_expression("$config.does_not_exist").unwrap();
        assert_eq!(evaluate_default(&e, &ctx).unwrap(), Scalar::Null);
    }

    #[test]
    fn unknown_function_errors() {
        let ctx = Context::new();
        let e = parse_expression("nope($a)").unwrap();
        assert!(evaluate_default(&e, &ctx).is_err());
    }

    #[test]
    fn resolves_array_index() {
        let arr = Scalar::Array(vec![
            Scalar::String("first".into()),
            Scalar::String("second".into()),
        ]);
        let ctx = ctx_with(&[("a", arr)]);
        assert_eq!(
            evaluate_default(&parse_expression("$a[0]").unwrap(), &ctx).unwrap(),
            Scalar::String("first".into())
        );
        assert_eq!(
            evaluate_default(&parse_expression("$a[1]").unwrap(), &ctx).unwrap(),
            Scalar::String("second".into())
        );
        // Out-of-range index → Null (tolerant lookup).
        assert_eq!(
            evaluate_default(&parse_expression("$a[9]").unwrap(), &ctx).unwrap(),
            Scalar::Null
        );
    }

    #[test]
    fn resolves_bracket_string_key() {
        // `$a["k"]` keys into an object, like `$a.k`.
        let mut o = HashMap::new();
        o.insert("k".to_string(), Scalar::Number(7.0));
        let ctx = ctx_with(&[("a", Scalar::Object(o))]);
        assert_eq!(
            evaluate_default(&parse_expression(r#"$a["k"]"#).unwrap(), &ctx).unwrap(),
            Scalar::Number(7.0)
        );
    }

    #[test]
    fn evaluates_array_literal() {
        let ctx = Context::new();
        let v = evaluate_default(&parse_expression("[1, 2, 3]").unwrap(), &ctx).unwrap();
        assert_eq!(
            v,
            Scalar::Array(vec![
                Scalar::Number(1.0),
                Scalar::Number(2.0),
                Scalar::Number(3.0),
            ])
        );
    }

    #[test]
    fn evaluates_object_literal_with_variable() {
        // Bare-identifier key, mixed literal and variable values.
        let ctx = ctx_with(&[("who", Scalar::String("Bob".into()))]);
        let v =
            evaluate_default(&parse_expression(r#"{id: "x", name: $who}"#).unwrap(), &ctx).unwrap();
        let Scalar::Object(map) = v else {
            panic!("expected object");
        };
        assert_eq!(map.get("id"), Some(&Scalar::String("x".into())));
        assert_eq!(map.get("name"), Some(&Scalar::String("Bob".into())));
    }

    #[test]
    fn default_function_uses_fallback_on_null() {
        let ctx = Context::new();
        // Missing first arg (Null) → fallback.
        assert_eq!(
            evaluate_default(
                &parse_expression(r#"default($missing, "fb")"#).unwrap(),
                &ctx
            )
            .unwrap(),
            Scalar::String("fb".into())
        );
        // Present first arg → returned unchanged.
        let ctx2 = ctx_with(&[("x", Scalar::String("here".into()))]);
        assert_eq!(
            evaluate_default(&parse_expression(r#"default($x, "fb")"#).unwrap(), &ctx2).unwrap(),
            Scalar::String("here".into())
        );
    }

    #[test]
    fn debug_function_serializes_json() {
        let ctx = Context::new();
        let v = evaluate_default(&parse_expression(r#"debug("hi")"#).unwrap(), &ctx).unwrap();
        assert_eq!(v, Scalar::String("\"hi\"".into()));
    }

    #[test]
    fn plain_string_stays_literal() {
        assert!(matches!(
            parse_expression(r#""just text""#).unwrap(),
            Expression::Literal(Scalar::String(_))
        ));
        // A brace that isn't `{$…}` is literal, not interpolation.
        assert!(matches!(
            parse_expression(r#""a {word} b""#).unwrap(),
            Expression::Literal(Scalar::String(_))
        ));
        assert!(!contains_interpolation("a {word} b"));
        assert!(contains_interpolation("a {$word} b"));
    }

    #[test]
    fn interpolates_single_variable() {
        let ctx = ctx_with(&[("name", Scalar::String("Ada".into()))]);
        let e = parse_expression(r#""Hello, {$name}!""#).unwrap();
        assert!(matches!(e, Expression::Interpolate(_)));
        assert_eq!(
            evaluate_default(&e, &ctx).unwrap(),
            Scalar::String("Hello, Ada!".into())
        );
    }

    #[test]
    fn interpolates_multiple_variables_and_whole_number() {
        // Two variables in one string, plus a YAML-integer document number
        // that stringifies without a trailing `.0` — the `concat` replacement.
        let mut fm = HashMap::new();
        fm.insert("documentNumber".to_string(), Scalar::Number(1234567.0));
        let mut markdoc = HashMap::new();
        markdoc.insert("frontmatter".to_string(), Scalar::Object(fm));
        let ctx = ctx_with(&[
            ("markdoc", Scalar::Object(markdoc)),
            ("env", Scalar::String("prod".into())),
        ]);
        let e = parse_expression(
            r#""https://{$env}.example.com/acme/{$markdoc.frontmatter.documentNumber}""#,
        )
        .unwrap();
        assert_eq!(
            evaluate_default(&e, &ctx).unwrap(),
            Scalar::String("https://prod.example.com/acme/1234567".into())
        );
    }

    #[test]
    fn missing_interpolated_variable_is_empty() {
        // An unresolved variable contributes nothing (Null → empty string),
        // leaving the literal frame intact.
        let ctx = Context::new();
        let e = parse_expression(r#""x{$nope}y""#).unwrap();
        assert_eq!(
            evaluate_default(&e, &ctx).unwrap(),
            Scalar::String("xy".into())
        );
    }

    #[test]
    fn interpolation_matches_only_variables() {
        // `{$…}` interpolates; a bare function-call brace stays literal.
        assert!(matches!(
            parse_expression(r#""v{$x}""#).unwrap(),
            Expression::Interpolate(_)
        ));
        assert!(matches!(
            parse_expression(r#""v{equals($a,$b)}""#).unwrap(),
            Expression::Literal(Scalar::String(_))
        ));
    }
}
