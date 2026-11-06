//! Smoke test for `evaluate_conditionals`: two `{% if equals(...) %}` guards
//! against the same variable, only one of which can match.
//!
//! With `model` set to the base robot, neither variant guard holds, so the
//! expected output is the unconditional `ALWAYS` line alone.

use markdoc::{
    evaluate_conditionals, parse,
    types::{Context, Scalar},
};
use std::collections::HashMap;

fn main() {
    let src = r#"{% if equals($model, $mir250_shelf_carrier) %}SHELF_ONLY{% /if %}
{% if equals($model, $mir250_hook) %}HOOK_ONLY{% /if %}
ALWAYS"#;
    let doc = parse(src, None).unwrap();
    let mut vars = HashMap::new();
    vars.insert("model".into(), Scalar::String("MiR250 Base Robot".into()));
    vars.insert(
        "mir250_shelf_carrier".into(),
        Scalar::String("MiR250 Shelf Carrier".into()),
    );
    vars.insert("mir250_hook".into(), Scalar::String("MiR250 Hook".into()));
    let ctx = Context { variables: vars };
    let result = evaluate_conditionals(&doc, &ctx).unwrap();
    println!("{result:#?}");
}
