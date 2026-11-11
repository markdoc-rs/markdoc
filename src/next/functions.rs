//! Port of upstream Markdoc v0.5.11's `src/functions/index.ts`.
//!
//! Owner: the semantics agent.
//!
//! `and`, `or`, `not`, `equals`, `default` and `debug`, with Markdoc's `truthy` (anything but
//! `false`, `undefined` and `null`) and JavaScript's `===`.

use crate::next::schema::value::{self, Value, strict_equals, truthy};
use crate::next::schema::{Config, ConfigFunction, FunctionTransformFn, SchemaAttribute};
use crate::types::{MarkdocError, Result};
use indexmap::IndexMap;
use std::sync::{Arc, LazyLock};

/// What a function's `transform` gets: `resolve(fn.parameters)`, any value (see
/// [`FunctionTransformFn`]).
type Parameters = Value;

fn function(transform: fn(&Parameters, &Config) -> Result<Value>) -> ConfigFunction {
    let transform: FunctionTransformFn = Arc::new(transform);
    ConfigFunction {
        transform: Some(transform),
        ..Default::default()
    }
}

fn throw(message: String) -> MarkdocError {
    MarkdocError::TransformError(message)
}

/// `parameters[key]`, read through the parameters' prototype (`null` and `undefined` throw).
fn param(parameters: &Parameters, key: &str) -> Result<Value> {
    value::get(parameters, key).map_err(throw)
}

/// `Object.values(parameters)` (`null` and `undefined` throw).
fn values(parameters: &Parameters) -> Result<Vec<Value>> {
    value::own_values_of(parameters).map_err(throw)
}

/// `and`: `Object.values(parameters).every((x) => truthy(x))`.
fn and(parameters: &Parameters, _config: &Config) -> Result<Value> {
    Ok(Value::Boolean(values(parameters)?.iter().all(truthy)))
}

/// `or`: `Object.values(parameters).find((x) => truthy(x)) !== undefined` (a truthy value is
/// never `undefined`).
fn or(parameters: &Parameters, _config: &Config) -> Result<Value> {
    Ok(Value::Boolean(values(parameters)?.iter().any(truthy)))
}

/// `not`: `!truthy(parameters[0])`.
fn not(parameters: &Parameters, _config: &Config) -> Result<Value> {
    Ok(Value::Boolean(!truthy(&param(parameters, "0")?)))
}

/// `equals`: `values.every((v) => v === values[0])` over `Object.values(parameters)`.
fn equals(parameters: &Parameters, _config: &Config) -> Result<Value> {
    let values = values(parameters)?;
    Ok(Value::Boolean(
        values.iter().all(|v| strict_equals(v, &values[0])),
    ))
}

/// `debug`: `JSON.stringify(parameters[0], null, 2)` (a `RangeError` for a value nested past
/// [`value::MAX_JSON_DEPTH`]).
fn debug(parameters: &Parameters, _config: &Config) -> Result<Value> {
    Ok(value::json_stringify(&param(parameters, "0")?, "  ")
        .map_err(MarkdocError::TransformError)?
        .map(Value::from)
        .unwrap_or(Value::Undefined))
}

/// `default`: `parameters[0] === undefined ? parameters[1] : parameters[0]`.
fn default_fn(parameters: &Parameters, _config: &Config) -> Result<Value> {
    let first = param(parameters, "0")?;
    Ok(if first.is_undefined() {
        param(parameters, "1")?
    } else {
        param(parameters, "0")?
    })
}

static DEFAULT_FUNCTIONS: LazyLock<IndexMap<String, Option<ConfigFunction>>> =
    LazyLock::new(|| {
        let mut not_fn = function(not);
        let mut parameters = IndexMap::new();
        parameters.insert(
            "0".to_string(),
            SchemaAttribute {
                required: true,
                ..Default::default()
            },
        );
        not_fn.parameters = Some(parameters);
        [
            ("and", function(and)),
            ("or", function(or)),
            ("not", not_fn),
            ("equals", function(equals)),
            ("default", function(default_fn)),
            ("debug", function(debug)),
        ]
        .into_iter()
        .map(|(name, f)| (name.to_string(), Some(f)))
        .collect()
    });

/// Upstream's `functions`: `and`, `or`, `not`, `equals`, `default`, `debug`.
pub fn default_functions() -> &'static IndexMap<String, Option<ConfigFunction>> {
    &DEFAULT_FUNCTIONS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Function, PathSegment, Variable};
    use crate::next::schema::Variables;
    use crate::next::transformer::resolve_function;
    use crate::types::Scalar;

    fn config(foo: &str) -> Config {
        let mut variables = IndexMap::new();
        variables.insert("foo".to_string(), Value::from(foo));
        Config {
            functions: default_functions().clone(),
            variables: Some(Variables::Value(Value::object(variables))),
            ..Default::default()
        }
    }

    fn equals_foo_bar() -> Function {
        let mut parameters = IndexMap::new();
        parameters.insert(
            "0".to_string(),
            Scalar::Variable(Variable::new(vec![PathSegment::from("foo")])),
        );
        parameters.insert("1".to_string(), Scalar::String("bar".into()));
        Function::new("equals".into(), parameters)
    }

    // functions/index.test.ts: "true match with variable"
    #[test]
    fn equals_true_match_with_variable() {
        let output = resolve_function(&equals_foo_bar(), &config("bar")).unwrap();
        assert!(matches!(output, Value::Boolean(true)));
    }

    // functions/index.test.ts: "false match with variable"
    #[test]
    fn equals_false_match_with_variable() {
        let output = resolve_function(&equals_foo_bar(), &config("baz")).unwrap();
        assert!(matches!(output, Value::Boolean(false)));
    }

    #[test]
    fn the_others() {
        let c = Config::default();
        let p = |values: Vec<Value>| -> Parameters {
            Value::object(
                values
                    .into_iter()
                    .enumerate()
                    .map(|(i, v)| (i.to_string(), v))
                    .collect::<IndexMap<String, Value>>(),
            )
        };
        let is = |v: Result<Value>, b: bool| matches!(v, Ok(Value::Boolean(x)) if x == b);
        assert!(is(
            and(&p(vec![Value::Number(0.0), Value::from("")]), &c),
            true
        ));
        assert!(is(and(&p(vec![Value::Null]), &c), false));
        assert!(is(and(&p(vec![]), &c), true));
        assert!(is(
            or(&p(vec![Value::Boolean(false), Value::Undefined]), &c),
            false
        ));
        assert!(is(
            or(&p(vec![Value::Boolean(false), Value::Number(0.0)]), &c),
            true
        ));
        assert!(is(not(&p(vec![]), &c), true));
        assert!(is(equals(&p(vec![]), &c), true));
        let a = Value::array(vec![]);
        assert!(is(equals(&p(vec![a.clone(), a]), &c), true));
        assert!(is(
            equals(&p(vec![Value::array(vec![]), Value::array(vec![])]), &c),
            false
        ));
        assert!(matches!(
            default_fn(&p(vec![Value::Undefined, Value::from(1.0)]), &c),
            Ok(Value::Number(n)) if n == 1.0
        ));
        assert!(matches!(
            default_fn(&p(vec![Value::Null, Value::from(1.0)]), &c),
            Ok(Value::Null)
        ));
        assert!(matches!(
            debug(&p(vec![Value::array(vec![Value::from(1.0)])]), &c),
            Ok(Value::String(ref s)) if s == "[\n  1\n]"
        ));
        assert!(matches!(debug(&p(vec![]), &c), Ok(Value::Undefined)));
    }
}
