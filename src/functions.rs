use crate::types::*;
use std::collections::HashMap;

pub type FunctionImpl = fn(&[Scalar]) -> Result<Scalar>;

pub fn default_functions() -> HashMap<String, (ConfigFunction, FunctionImpl)> {
    let mut functions = HashMap::new();

    // equals function
    functions.insert(
        "equals".to_string(),
        (
            ConfigFunction {
                returns: Some(vec![ValidationType::Boolean]),
                parameters: None,
            },
            equals as FunctionImpl,
        ),
    );

    // and function
    functions.insert(
        "and".to_string(),
        (
            ConfigFunction {
                returns: Some(vec![ValidationType::Boolean]),
                parameters: None,
            },
            and as FunctionImpl,
        ),
    );

    // or function
    functions.insert(
        "or".to_string(),
        (
            ConfigFunction {
                returns: Some(vec![ValidationType::Boolean]),
                parameters: None,
            },
            or as FunctionImpl,
        ),
    );

    // not function
    functions.insert(
        "not".to_string(),
        (
            ConfigFunction {
                returns: Some(vec![ValidationType::Boolean]),
                parameters: None,
            },
            not as FunctionImpl,
        ),
    );

    // default function — returns the fallback when the first argument is
    // undefined (missing variables resolve to Null in this port).
    functions.insert(
        "default".to_string(),
        (
            ConfigFunction {
                returns: None,
                parameters: None,
            },
            default_value as FunctionImpl,
        ),
    );

    // debug function — serialize the argument(s) as JSON for troubleshooting.
    functions.insert(
        "debug".to_string(),
        (
            ConfigFunction {
                returns: None,
                parameters: None,
            },
            debug_value as FunctionImpl,
        ),
    );

    // include(array, value) — true when array contains value (MiR web parity).
    functions.insert(
        "include".to_string(),
        (
            ConfigFunction {
                returns: Some(vec![ValidationType::Boolean]),
                parameters: None,
            },
            include as FunctionImpl,
        ),
    );

    functions
}

/// Flatten a scalar to a plain string for string interpolation
/// (`"…{$var}…"`). Whole-number floats (YAML integers arrive as `f64`) print
/// without a fractional part, so a document number splices into a URL
/// cleanly; arrays / objects have no sensible flat form and contribute
/// nothing.
pub(crate) fn scalar_to_plain_string(s: &Scalar) -> String {
    match s {
        Scalar::String(v) => v.clone(),
        Scalar::Number(n) if n.is_finite() && n.fract() == 0.0 => format!("{}", *n as i64),
        Scalar::Number(n) => n.to_string(),
        Scalar::Boolean(b) => b.to_string(),
        Scalar::Null | Scalar::Array(_) | Scalar::Object(_) => String::new(),
    }
}

fn equals(args: &[Scalar]) -> Result<Scalar> {
    if args.len() != 2 {
        return Err(MarkdocError::TransformError(
            "equals requires exactly 2 arguments".to_string(),
        ));
    }

    Ok(Scalar::Boolean(scalar_eq(&args[0], &args[1])))
}

fn and(args: &[Scalar]) -> Result<Scalar> {
    for arg in args {
        if !truthy(arg) {
            return Ok(Scalar::Boolean(false));
        }
    }
    Ok(Scalar::Boolean(true))
}

fn or(args: &[Scalar]) -> Result<Scalar> {
    for arg in args {
        if truthy(arg) {
            return Ok(Scalar::Boolean(true));
        }
    }
    Ok(Scalar::Boolean(false))
}

fn not(args: &[Scalar]) -> Result<Scalar> {
    if args.len() != 1 {
        return Err(MarkdocError::TransformError(
            "not requires exactly 1 argument".to_string(),
        ));
    }

    Ok(Scalar::Boolean(!truthy(&args[0])))
}

fn include(args: &[Scalar]) -> Result<Scalar> {
    if args.len() != 2 {
        return Err(MarkdocError::TransformError(
            "include requires exactly 2 arguments".to_string(),
        ));
    }

    let found = match &args[0] {
        Scalar::Array(items) => items.iter().any(|item| scalar_eq(item, &args[1])),
        _ => false,
    };

    Ok(Scalar::Boolean(found))
}

fn default_value(args: &[Scalar]) -> Result<Scalar> {
    if args.len() != 2 {
        return Err(MarkdocError::TransformError(
            "default requires exactly 2 arguments".to_string(),
        ));
    }
    // Markdoc returns the second argument when the first is undefined.
    // Missing variables resolve to Null here, so Null is the trigger.
    Ok(match &args[0] {
        Scalar::Null => args[1].clone(),
        present => present.clone(),
    })
}

fn debug_value(args: &[Scalar]) -> Result<Scalar> {
    // Serialize for troubleshooting: a single argument as itself, several
    // as a JSON array.
    let json = if args.len() == 1 {
        serde_json::to_string(&args[0])
    } else {
        serde_json::to_string(args)
    }
    .unwrap_or_else(|_| "null".to_string());
    Ok(Scalar::String(json))
}

fn truthy(value: &Scalar) -> bool {
    match value {
        Scalar::Null => false,
        Scalar::Boolean(b) => *b,
        Scalar::Number(n) => *n != 0.0,
        Scalar::String(s) => !s.is_empty(),
        Scalar::Array(a) => !a.is_empty(),
        Scalar::Object(o) => !o.is_empty(),
    }
}

fn scalar_eq(a: &Scalar, b: &Scalar) -> bool {
    match (a, b) {
        (Scalar::Null, Scalar::Null) => true,
        (Scalar::Boolean(a), Scalar::Boolean(b)) => a == b,
        (Scalar::Number(a), Scalar::Number(b)) => (a - b).abs() < f64::EPSILON,
        (Scalar::String(a), Scalar::String(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_to_plain_string_stringifies_whole_numbers() {
        // A YAML integer (f64) prints without a trailing `.0`, so a document
        // number splices cleanly into an interpolated URL; a fractional
        // number keeps its decimals; null / arrays / objects are empty.
        assert_eq!(
            scalar_to_plain_string(&Scalar::Number(1234567.0)),
            "1234567"
        );
        assert_eq!(scalar_to_plain_string(&Scalar::Number(2.5)), "2.5");
        assert_eq!(scalar_to_plain_string(&Scalar::Boolean(true)), "true");
        assert_eq!(scalar_to_plain_string(&Scalar::Null), "");
    }
}
