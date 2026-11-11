//! E8: `config.variables` from a document's frontmatter and from maps the caller passes.
//!
//! Upstream reads variables only from `config.variables` and binds nothing itself: a document's
//! `{% $markdoc.frontmatter.title %}` is `undefined` (renders nothing) unless the caller binds
//! it. [`Bindings`] collects top-level variables the way the old pipeline's `Context` did
//! (`with_variable`, `with_frontmatter` for `$markdoc.frontmatter`, `with_config` for
//! `$config`, `with_globals` for `$global`) and puts them into a config ([`Bindings::apply`]).
//! [`frontmatter_variables`] binds a parsed document's own frontmatter; [`bind`] sets one
//! variable of a config.
//!
//! ```
//! use markdoc::ext::{Frontmatter, ParseOptions, parse, variables::frontmatter_variables};
//! use markdoc::next::schema::Config;
//! use markdoc::next::{index, renderers::html};
//! use markdoc::types::Scalar;
//!
//! let options = ParseOptions { frontmatter: Frontmatter::Yaml, ..Default::default() };
//! let doc = parse("---\ntitle: Hello\n---\n# {% $markdoc.frontmatter.title %} {% $config.v %}\n", &options)
//!     .unwrap();
//! let mut config = Config::default();
//! frontmatter_variables(&doc)
//!     .with_config(&Scalar::Object([("v".to_string(), Scalar::Number(2.0))].into()))
//!     .apply(&mut config);
//! let out = html::render(&index::transform(&doc, &config).unwrap()).unwrap();
//! assert_eq!(out, "<article><h1>Hello 2</h1></article>");
//! ```

use crate::ast::Node;
use crate::next::schema::value::{Value, own_entries};
use crate::next::schema::{Config, Variables};
use crate::types::Scalar;
use indexmap::IndexMap;

/// Top-level variables to put into a config (see the module's documentation), in the order
/// they were first bound.
#[derive(Clone, Default)]
pub struct Bindings {
    values: IndexMap<String, Value>,
}

impl std::fmt::Debug for Bindings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.values.iter()).finish()
    }
}

impl Bindings {
    pub fn new() -> Bindings {
        Bindings::default()
    }

    /// Binds `$name` to `value` (replacing an earlier binding of `name`).
    pub fn with_variable(self, name: impl Into<String>, value: &Scalar) -> Bindings {
        self.with_value(name, Value::from_scalar(value))
    }

    /// Binds `$name` to a value as it is.
    pub fn with_value(mut self, name: impl Into<String>, value: Value) -> Bindings {
        self.values.insert(name.into(), value);
        self
    }

    /// Binds `$markdoc.frontmatter` to `frontmatter`, keeping any other member of an object
    /// bound to `$markdoc`.
    pub fn with_frontmatter(mut self, frontmatter: &Scalar) -> Bindings {
        let mut markdoc = IndexMap::new();
        // (`Scalar`'s drop is its own: take the members out rather than move them.)
        if let Some(Ok(mut scalar)) = self.values.get("markdoc").map(Value::to_scalar)
            && let Scalar::Object(members) = &mut scalar
        {
            markdoc = std::mem::take(members);
        }
        markdoc.insert("frontmatter".to_string(), frontmatter.clone());
        self.values.insert(
            "markdoc".to_string(),
            Value::from_scalar(&Scalar::Object(markdoc)),
        );
        self
    }

    /// Binds `$config`.
    pub fn with_config(self, config: &Scalar) -> Bindings {
        self.with_variable("config", config)
    }

    /// Binds `$global`.
    pub fn with_globals(self, globals: &Scalar) -> Bindings {
        self.with_variable("global", globals)
    }

    /// Binds every member of `map` at the top level.
    pub fn with_all<'a>(
        mut self,
        map: impl IntoIterator<Item = (&'a String, &'a Scalar)>,
    ) -> Bindings {
        for (name, value) in map {
            self = self.with_variable(name.clone(), value);
        }
        self
    }

    /// The bound variables as an object.
    pub fn to_value(&self) -> Value {
        Value::object(self.values.clone())
    }

    /// Puts the bindings into `config.variables`: into the object it holds (a binding replaces
    /// a member of the same name), or as a new object when it holds none, or holds something
    /// other than an object (a function of the path included, which the bindings replace).
    pub fn apply(&self, config: &mut Config) {
        // `{...variables, ...bindings}`: the object's own members as they are (no round trip
        // through `Scalar`, which cost a deep copy per call and dropped what JSON cannot hold).
        let mut merged: IndexMap<String, Value> = match &config.variables {
            Some(Variables::Value(value @ Value::Object(_))) => {
                own_entries(value).into_iter().collect()
            }
            _ => IndexMap::new(),
        };
        for (name, value) in &self.values {
            merged.insert(name.clone(), value.clone());
        }
        config.variables = Some(Variables::Value(Value::object(merged)));
    }
}

/// `$markdoc.frontmatter` of a parsed document: its `frontmatter` attribute (an object when
/// it was parsed with [`crate::ext::Frontmatter::Yaml`], else upstream's raw text), or
/// nothing when it has none.
pub fn frontmatter_variables(document: &Node) -> Bindings {
    match document.attributes.get("frontmatter") {
        Some(frontmatter) => Bindings::new().with_frontmatter(frontmatter),
        None => Bindings::new(),
    }
}

/// Sets the top-level variable `$name` of `config` (see [`Bindings::apply`]).
pub fn bind(config: &mut Config, name: impl Into<String>, value: Value) {
    Bindings::new().with_value(name, value).apply(config);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ext::{Frontmatter, ParseOptions, parse};
    use crate::next::index;
    use crate::next::renderers::html;

    const SOURCE: &str = "---\ntitle: Hello\nproducts: [Bot250]\n---\n\
                          # {% $markdoc.frontmatter.title %}\n\n\
                          {% $config.product %} / {% $global.site %} / {% $edition %}\n";

    fn yaml() -> ParseOptions {
        ParseOptions {
            frontmatter: Frontmatter::Yaml,
            ..Default::default()
        }
    }

    fn rendered(source: &str, config: &Config) -> String {
        let doc = parse(source, &yaml()).unwrap();
        html::render(&index::transform(&doc, config).unwrap()).unwrap()
    }

    fn object(pairs: &[(&str, &str)]) -> Scalar {
        Scalar::Object(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), Scalar::String(v.to_string())))
                .collect(),
        )
    }

    #[test]
    fn without_bindings_upstream_binds_nothing() {
        assert_eq!(
            rendered(SOURCE, &Config::default()),
            "<article><h1></h1><p> /  / </p></article>"
        );
    }

    #[test]
    fn bindings_render_as_the_old_context_did() {
        let doc = parse(SOURCE, &yaml()).unwrap();
        let mut config = Config::default();
        frontmatter_variables(&doc)
            .with_config(&object(&[("product", "Bot250")]))
            .with_globals(&object(&[("site", "Support")]))
            .with_variable("edition", &Scalar::String("2.0".into()))
            .apply(&mut config);
        let new = rendered(SOURCE, &config);
        // What the old pipeline (removed at the switch-over) rendered from the same variables
        // through its `Context`, apart from its root element (a `div` with an empty
        // `frontmatter` attribute) and the heading id its parser added.
        assert_eq!(
            new,
            "<article><h1>Hello</h1><p>Bot250 / Support / 2.0</p></article>"
        );
    }

    #[test]
    fn apply_merges_into_the_configs_object() {
        let mut config = Config::default();
        bind(&mut config, "a", Value::string("1"));
        bind(&mut config, "b", Value::string("2"));
        Bindings::new()
            .with_variable("a", &Scalar::String("3".into()))
            .apply(&mut config);
        let Some(Variables::Value(v)) = &config.variables else {
            panic!("an object")
        };
        assert_eq!(v.to_scalar().unwrap(), object(&[("a", "3"), ("b", "2")]));
        // `$markdoc` keeps its other members.
        let b = Bindings::new()
            .with_variable("markdoc", &object(&[("other", "x")]))
            .with_frontmatter(&Scalar::String("raw".into()));
        assert_eq!(
            b.to_value().to_scalar().unwrap(),
            Scalar::Object(IndexMap::from([(
                "markdoc".to_string(),
                object(&[("other", "x"), ("frontmatter", "raw")])
            )]))
        );
    }

    #[test]
    fn raw_frontmatter_binds_as_the_raw_text() {
        let doc = parse(
            "---\ntitle: x\n---\n{% $markdoc.frontmatter %}\n",
            &ParseOptions::default(),
        )
        .unwrap();
        let mut config = Config::default();
        frontmatter_variables(&doc).apply(&mut config);
        assert_eq!(
            html::render(&index::transform(&doc, &config).unwrap()).unwrap(),
            "<article><p>title: x</p></article>"
        );
        let none = parse("# No frontmatter\n", &ParseOptions::default()).unwrap();
        assert!(
            frontmatter_variables(&none).to_value().to_scalar().unwrap()
                == Scalar::Object(IndexMap::new())
        );
    }
}
