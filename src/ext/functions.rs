//! E7: functions beyond upstream's six (`and`, `or`, `not`, `equals`, `default`, `debug`).
//!
//! A function a document calls must be in `config.functions`; one that is not resolves to
//! `null` without any error (upstream's rule), so a condition on it is silently false. Nothing
//! here is registered unless the caller registers it.
//!
//! - [`include()`] and [`includes()`] (one function, two spellings): `includes($list, value)` is
//!   true when the first parameter is an array that contains the second, as JavaScript's
//!   `Array.prototype.includes` compares (SameValueZero). It is what the web pipeline defined
//!   (`Array.isArray(array) ? array.includes(value) : false`, over `Object.values` of the
//!   parameters) and what the crate's old `include` did; it never fails.
//! - [`positional`] adapts a function written in the old style, `Fn(&[Scalar]) ->
//!   Result<Scalar>`, to a [`ConfigFunction`]: it receives the parameters' values in their
//!   order (upstream keys positional ones `"0"`, `"1"`, ...), and an `Err` is what the
//!   transform returns.
//! - [`all`] is every function here, by name, to extend `config.functions` with.
//!
//! ```
//! use markdoc::ext::{ParseOptions, functions, parse};
//! use markdoc::next::schema::{Config, Variables, value::Value};
//! use markdoc::next::{index, renderers::html};
//! use markdoc::types::{MarkdocError, Scalar};
//! use markdoc::IndexMap;
//!
//! let mut config = Config::default();
//! config.functions.extend(functions::all());
//! config.functions.insert(
//!     "upper".into(),
//!     Some(functions::positional(|args: &[Scalar]| match args.first() {
//!         Some(Scalar::String(s)) => Ok(Scalar::String(s.to_uppercase())),
//!         _ => Err(MarkdocError::TransformError("upper takes a string".into())),
//!     })),
//! );
//! let products = Value::array(vec![Value::string("Bot250")]);
//! config.variables = Some(Variables::Value(Value::object(IndexMap::from([(
//!     "products".to_string(),
//!     products,
//! )]))));
//!
//! let doc = parse(
//!     "{% if includes($products, \"Bot250\") %}\n{% upper(\"yes\") %}\n{% /if %}\n",
//!     &ParseOptions::default(),
//! )
//! .unwrap();
//! let out = html::render(&index::transform(&doc, &config).unwrap()).unwrap();
//! // An interpolation on a line of its own is no paragraph (upstream's rule).
//! assert_eq!(out, "<article>YES</article>");
//! ```

use crate::next::schema::value::{self, Value};
use crate::next::schema::{Config, ConfigFunction, FunctionTransformFn};
use crate::types::{MarkdocError, Result, Scalar};
use indexmap::IndexMap;
use std::sync::Arc;

/// `includes(list, value)` (see the module's documentation).
pub fn includes() -> ConfigFunction {
    let transform: FunctionTransformFn = Arc::new(|parameters, _config: &Config| {
        // `Object.values(parameters)`: the own properties' values, in order.
        let mut values = value::own_entries(parameters).into_iter().map(|(_, v)| v);
        let (Some(list), value) = (values.next(), values.next()) else {
            return Ok(Value::Boolean(false));
        };
        Ok(Value::Boolean(array_includes(
            &list,
            &value.unwrap_or(Value::Undefined),
        )))
    });
    ConfigFunction {
        transform: Some(transform),
        description: Some(
            "True when the first parameter is an array that contains the second".into(),
        ),
        ..Default::default()
    }
}

/// `include(list, value)`: [`includes`] under the other spelling the corpus uses.
pub fn include() -> ConfigFunction {
    includes()
}

/// Every function of this module, by name: `include` and `includes`.
pub fn all() -> IndexMap<String, Option<ConfigFunction>> {
    IndexMap::from([
        ("include".to_string(), Some(include())),
        ("includes".to_string(), Some(includes())),
    ])
}

/// A function of the parameters' values in their order, as a [`ConfigFunction`] (see the
/// module's documentation). The values are given as JSON keeps them ([`Value::to_scalar`]);
/// the result is taken as a value ([`Value::from_scalar`]).
pub fn positional<F>(f: F) -> ConfigFunction
where
    F: Fn(&[Scalar]) -> Result<Scalar> + Send + Sync + 'static,
{
    let transform: FunctionTransformFn = Arc::new(move |parameters, _config: &Config| {
        let args = value::own_entries(parameters)
            .into_iter()
            .map(|(_, v)| v.to_scalar().map_err(MarkdocError::TransformError))
            .collect::<Result<Vec<_>>>()?;
        f(&args).map(|result| Value::from_scalar(&result))
    });
    ConfigFunction {
        transform: Some(transform),
        ..Default::default()
    }
}

/// `Array.isArray(list) && list.includes(value)`, compared as SameValueZero: primitives by
/// value (NaN matches NaN). An object or array matches only itself, which a value read back
/// from JSON cannot show, so an object or array `value` never matches here.
fn array_includes(list: &Value, value: &Value) -> bool {
    if !matches!(list, Value::Array(_)) {
        return false;
    }
    let Ok(scalar) = list.to_scalar() else {
        return false;
    };
    let Scalar::Array(items) = &scalar else {
        return false;
    };
    let needle = match value {
        Value::Null => Scalar::Null,
        Value::Boolean(b) => Scalar::Boolean(*b),
        Value::Number(n) => Scalar::Number(*n),
        Value::String(_) => match value.to_scalar() {
            Ok(s) => s,
            Err(_) => return false,
        },
        // `undefined` (a hole or an `undefined` element) reads back as null; objects, arrays
        // and tags by identity.
        Value::Undefined
        | Value::Array(_)
        | Value::Object(_)
        | Value::Tag(_)
        | Value::Builtin(_) => return false,
    };
    items.iter().any(|item| match (item, &needle) {
        (Scalar::Number(a), Scalar::Number(b)) => a == b || (a.is_nan() && b.is_nan()),
        (Scalar::String(a), Scalar::String(b)) => a == b,
        (Scalar::Boolean(a), Scalar::Boolean(b)) => a == b,
        (Scalar::Null, Scalar::Null) => true,
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ext::{ParseOptions, parse};
    use crate::next::index;
    use crate::next::renderers::html;
    use crate::next::schema::Variables;

    /// Verbatim from the corpus's `AUTHORING.md` (as the old crate's test has it).
    const FROM_THE_GUIDE: &str = "\
{% if includes($products, \"Bot1200 Pallet Jack\") %}
Pallet Jack only.
{% /if %}
";

    fn config(products: &[&str], functions: bool) -> Config {
        let mut config = Config::default();
        if functions {
            config.functions.extend(all());
        }
        let list = Value::array(products.iter().map(|p| Value::string(*p)).collect());
        config.variables = Some(Variables::Value(Value::object(IndexMap::from([(
            "products".to_string(),
            list,
        )]))));
        config
    }

    fn rendered(source: &str, config: &Config) -> String {
        let doc = parse(source, &ParseOptions::default()).unwrap();
        html::render(&index::transform(&doc, config).unwrap()).unwrap()
    }

    #[test]
    fn the_documented_spelling_gates_on_membership() {
        let yes = config(&["Bot1200 Pallet Jack"], true);
        assert!(rendered(FROM_THE_GUIDE, &yes).contains("Pallet Jack only"));
        for products in [&["Bot250"][..], &[]] {
            let no = config(products, true);
            assert!(!rendered(FROM_THE_GUIDE, &no).contains("Pallet Jack only"));
        }
        let other = FROM_THE_GUIDE.replace("includes(", "include(");
        assert!(rendered(&other, &yes).contains("Pallet Jack only"));
    }

    #[test]
    fn unregistered_the_call_is_upstreams_null() {
        // Off by default: upstream resolves an unknown function to null, so the section is
        // hidden and an interpolation prints nothing.
        let off = config(&["Bot1200 Pallet Jack"], false);
        assert_eq!(rendered(FROM_THE_GUIDE, &off), "<article></article>");
        assert_eq!(
            rendered("A{% includes($products, \"x\") %}B\n", &off),
            "<article><p>AB</p></article>"
        );
    }

    #[test]
    fn it_answers_as_the_old_include_did() {
        // The old pipeline's `include(list, value)` (removed at the switch-over): true when
        // `list` is an array holding `value`, false for anything else.
        let new = includes();
        let transform = new.transform.as_ref().unwrap();
        let a = || Scalar::String("a".into());
        let cases: Vec<(Scalar, Scalar, bool)> = vec![
            (Scalar::Array(vec![a(), Scalar::Number(1.0)]), a(), true),
            (
                Scalar::Array(vec![a(), Scalar::Number(1.0)]),
                Scalar::Number(1.0),
                true,
            ),
            (Scalar::Array(vec![a()]), Scalar::String("b".into()), false),
            (Scalar::Array(vec![Scalar::Null]), Scalar::Null, true),
            (
                Scalar::Array(vec![Scalar::Boolean(true)]),
                Scalar::Boolean(true),
                true,
            ),
            (Scalar::String("abc".into()), a(), false),
            (Scalar::Null, Scalar::Null, false),
            (Scalar::Array(vec![]), a(), false),
        ];
        for (list, value, want) in cases {
            let params = Value::object(IndexMap::from([
                ("0".to_string(), Value::from_scalar(&list)),
                ("1".to_string(), Value::from_scalar(&value)),
            ]));
            let got = transform(&params, &Config::default()).unwrap();
            assert_eq!(
                got.to_scalar().unwrap(),
                Scalar::Boolean(want),
                "{list:?} {value:?}"
            );
        }
        // Where the old function failed (a wrong count), the web pipeline's never did.
        let one = Value::object(IndexMap::from([("0".to_string(), Value::Null)]));
        assert!(matches!(
            transform(&one, &Config::default()).unwrap(),
            Value::Boolean(false)
        ));
        let nan = Value::object(IndexMap::from([
            ("0".to_string(), Value::array(vec![Value::Number(f64::NAN)])),
            ("1".to_string(), Value::Number(f64::NAN)),
        ]));
        assert!(matches!(
            transform(&nan, &Config::default()).unwrap(),
            Value::Boolean(true)
        ));
    }

    #[test]
    fn positional_passes_the_values_in_order() {
        let mut config = Config::default();
        config.functions.insert(
            "join".into(),
            Some(positional(|args: &[Scalar]| {
                Ok(Scalar::String(
                    args.iter()
                        .map(|a| match a {
                            Scalar::String(s) => s.clone(),
                            Scalar::Number(n) => n.to_string(),
                            other => format!("{other:?}"),
                        })
                        .collect::<Vec<_>>()
                        .join("+"),
                ))
            })),
        );
        config.functions.insert(
            "fail".into(),
            Some(positional(|_: &[Scalar]| {
                Err(MarkdocError::TransformError("no".into()))
            })),
        );
        assert_eq!(
            rendered("{% join(\"a\", 2, k=\"c\") %}\n", &config),
            "<article>a+2+c</article>"
        );
        let doc = parse("{% fail() %}\n", &ParseOptions::default()).unwrap();
        assert!(index::transform(&doc, &config).is_err());
    }
}
