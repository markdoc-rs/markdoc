//! Control flow and custom functions, end to end, through markdoc's pipeline and the `ext`
//! layers.
//!
//! ```sh
//! cargo run --example control_flow
//! ```
//!
//! [`control_flow.md`](control_flow.md) is the document; this file is the wiring:
//!
//! * E1: the document is parsed with its frontmatter as YAML (`ParseOptions`).
//! * E2 and E3: the built-in schema bundle declares the tags (`callout`, ...);
//!   `control_flow::apply_bundle` gives `switch`, `case` and `for` their behaviour.
//! * E7: `count` and `upper`, functions this crate does not define, written as plain
//!   `fn(&[Scalar]) -> Result<Scalar>` and registered through `functions::positional`.
//! * E8: `$markdoc.frontmatter` and `$fleet` bound into `config.variables`.
//!
//! Upstream evaluates `if`, `switch`, `for`, interpolations and attributes all at transform
//! time, against the one config: there is no second pass to keep in step.

use markdoc::ext::bundle::Bundle;
use markdoc::ext::functions::positional;
use markdoc::ext::variables::frontmatter_variables;
use markdoc::ext::{Frontmatter, ParseOptions, control_flow, parse};
use markdoc::renderers::html;
use markdoc::schema::Config;
use markdoc::schema::value::Value;
use markdoc::transform;
use markdoc::types::{MarkdocError, Result, Scalar};

/// `count($array)`: the length of an array.
fn count(args: &[Scalar]) -> Result<Scalar> {
    match args {
        [Scalar::Array(items)] => Ok(Scalar::Number(items.len() as f64)),
        _ => Err(MarkdocError::TransformError(
            "count() takes one array".into(),
        )),
    }
}

/// `upper($string)`: upper-cased; anything else as it is.
fn upper(args: &[Scalar]) -> Result<Scalar> {
    match args {
        [Scalar::String(s)] => Ok(Scalar::String(s.to_uppercase())),
        [other] => Ok(other.clone()),
        _ => Err(MarkdocError::TransformError(
            "upper() takes one argument".into(),
        )),
    }
}

/// The data the document is rendered against.
fn fleet() -> serde_json::Value {
    serde_json::json!([
        {"name": "Bot100", "status": "ok", "payload": 100, "modules": ["shelf lift", "top module"]},
        {"name": "Bot250", "status": "service", "payload": 250, "modules": ["hook"]},
        {"name": "Bot1200", "status": "retired", "payload": 1200, "modules": []}
    ])
}

/// The configuration: the bundle's tags, the control-flow layer, the two functions and the
/// variables.
pub fn config_for(document: &markdoc::Node) -> Config {
    let bundle = Bundle::builtin();
    let mut config = bundle.config();
    control_flow::apply_bundle(&bundle, &mut config);
    config
        .functions
        .insert("count".into(), Some(positional(count)));
    config
        .functions
        .insert("upper".into(), Some(positional(upper)));
    frontmatter_variables(document)
        .with_value("fleet", Value::from_json(&fleet()))
        .apply(&mut config);
    config
}

fn main() {
    let options = ParseOptions {
        frontmatter: Frontmatter::Yaml,
        ..ParseOptions::default()
    };
    let doc = parse(include_str!("control_flow.md"), &options).expect("the document parses");
    let config = config_for(&doc);
    let tree = transform(&doc, &config).expect("the document transforms");
    println!("{}", html::render(&tree).expect("the tree renders"));
}
