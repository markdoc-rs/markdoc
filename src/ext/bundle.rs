//! E2: the TOML schema bundle and overlays, loaded into a [`Config`].
//!
//! A schema file declares tags (`[tags.<name>]`), attributes added to upstream's node schemas
//! (`[nodes.<type>]`), the parse options every tool reading the corpus shares (`[parse]`,
//! `[render]`; see [`ParseOptions::from_toml`]), upstream's `config.validation`
//! (`[validation]`) and the functions the corpus calls (`[functions.<name>]`, see
//! "Functions"). The crate's own file, `schema/tags.toml`, is
//! [`Bundle::builtin`]; a corpus layers its vocabulary on top with [`Bundle::overlay`]. The
//! bundle also keeps what editors read and a [`Config`] has no field for: aliases
//! ([`Bundle::aliases`]), engine labels ([`Bundle::engines`]) and the layer a tag needs.
//!
//! Without a bundle nothing changes: [`Config::default()`] is upstream's empty config.
//!
//! # From TOML to schemas
//!
//! 1. A tag renders under its own name unless it says otherwise (`render = "name"`, or
//!    `render = false` to render only its children). An alias renders as its canonical tag.
//! 2. `alias_of = "x"` gives a tag `x`'s schema (with the alias's own `description`, if any).
//! 3. Upstream's built-in tags (`else`, `if`, `partial`, `slot`, `table`) are never replaced:
//!    an entry for one may give its `description`, an `engine` label and new attributes (an
//!    attribute the built-in already has takes only a `description`). `builtin = true` says
//!    so in the data. Anything else is a load error, because a TOML `if` would lose its
//!    transform (both branches render), a `partial` would render nothing, and a `table` with
//!    `render = "table"` would nest a table in a table.
//! 4. `inline` is honoured when given; omitted, either placement is fine (`None`).
//! 5. `one_of` becomes `matches` (a list), `pattern` a `matches` RegExp (JavaScript's); both
//!    together accept a value that satisfies either (one attribute `validate` function).
//! 6. `children` lists tag names (unlike upstream's `children`, which lists node types): a
//!    `validate` function reports any other direct child as `child-invalid`, as upstream
//!    words it.
//! 7. `type` is a list of `String`, `Number`, `Boolean`, `Object`, `Array` (one name is that
//!    constructor, several a union); `default` and an attribute's `render` are values.
//! 8. `[nodes.<type>.attributes.<name>]` adds attributes to upstream's node schema of that type,
//!    under rule 3's terms (new attributes whole; an existing one only its `description`).
//! 9. `primary` is never added implicitly; a tag that takes one declares it.
//! 10. `layer = "<name>"` marks a tag an extension layer implements (`switch`, `case` and `for`:
//!     `control_flow`). [`Bundle::config`] leaves such tags out; the layer takes their schemas
//!     from [`Bundle::layer_schemas`] and adds its behaviour.
//!
//! Unknown keys are errors, so a misspelt key fails the load instead of meaning nothing.
//!
//! # Overlays
//!
//! An overlay merges into what is loaded, field by field (D24): an entry for a tag that exists
//! changes only the fields and attributes it states, and an attribute it states changes only
//! the fields it states. `[parse]`, `[render]` and `[validation]` keys override one by one, and
//! a `[functions.<name>]` entry merges as a tag's does (its `parameters` as attributes).
//!
//! # Functions
//!
//! `[validation] validate_functions = true` is upstream's `config.validation.validateFunctions`:
//! a call of a function the config does not know is `function-undefined`. A
//! `[functions.<name>]` entry declares a function the corpus calls: its `description`, its
//! `returns` (a list of type names, as `type` is) and its `parameters` (`parameters.0`,
//! `parameters.1`, ... for positional arguments, named ones by name; each takes an attribute's
//! keys, of which `validate` reads `type` and `required`). A declaration has no code:
//! [`Bundle::apply`] merges it, field by field, under the entry the config already has (the
//! transform a tool registered, or upstream's own for one of its six functions), and adds a
//! declared function the config does not have without a transform. Such a function resolves
//! to `null` without an error, which hides whatever it guards (MIGRATION-MAP R3), so a tool
//! registers its functions' code first, applies the bundle, and calls
//! [`Bundle::check_functions`], which names every declared function the config cannot run.
//!
//! ```
//! use markdoc::ext::bundle::Bundle;
//! use markdoc::ext::functions::includes;
//! use markdoc::schema::Config;
//!
//! let mut bundle = Bundle::builtin();
//! bundle
//!     .overlay(
//!         "[validation]\nvalidate_functions = true\n\n\
//!          [functions.includes]\ndescription = \"True when the array holds the value\"\n\
//!          [functions.includes.parameters.0]\ntype = [\"Array\"]\nrequired = true\n\
//!          [functions.includes.parameters.1]\nrequired = true\n\n\
//!          [functions.higher]\n",
//!     )
//!     .unwrap();
//! let mut config = Config::default();
//! config.functions.insert("includes".into(), Some(includes()));
//! bundle.apply(&mut config);
//! assert!(config.validation.validate_functions);
//! let declared = config.functions["includes"].as_ref().unwrap();
//! assert!(declared.transform.is_some() && declared.parameters.is_some());
//! // `higher` is declared, but nothing runs it.
//! assert_eq!(
//!     bundle.check_functions(&config).unwrap_err().to_string(),
//!     "Parse error: schema: declared functions without code: higher (register a \
//!      transform for each before starting)"
//! );
//! ```
//!
//! ```
//! use markdoc::ext::bundle::Bundle;
//! use markdoc::ext::parse;
//! use markdoc::next::{index, renderers::html};
//!
//! let mut bundle = Bundle::builtin();
//! bundle
//!     .overlay(
//!         "[parse]\nallow_indentation = true\n\n\
//!          [tags.lightindicators]\nself_closing = true\n",
//!     )
//!     .unwrap();
//! let config = bundle.config();
//!
//! let doc = parse(
//!     "{% notice type=\"warning\" %}\nMind the gap.\n{% /notice %}\n\n{% lightindicators /%}\n",
//!     &bundle.parse_options(),
//! )
//! .unwrap();
//! assert!(index::validate(&doc, &config).unwrap().is_empty());
//! assert_eq!(
//!     html::render(&index::transform(&doc, &config).unwrap()).unwrap(),
//!     "<article><callout type=\"warning\"><p>Mind the gap.</p></callout>\
//!      <lightindicators></lightindicators></article>"
//! );
//! assert_eq!(bundle.canonical_name("notice"), "callout");
//! ```

use super::ParseOptions;
use crate::ast::Node;
use crate::next::functions::default_functions;
use crate::next::schema::value::Value;
use crate::next::schema::{
    AttributeValidateFn, BuiltinType, Config, ConfigFunction, JsRegExp, Matches, Schema,
    SchemaAttribute, SchemaKind, TypeSpec, ValidateFn, ValidationContext, default_nodes,
};
use crate::next::tags::default_tags;
use crate::types::{
    MarkdocError, NodeType, Result, Scalar, ValidationError, ValidationLevel, js_insert,
};
use indexmap::IndexMap;
use serde::Deserialize;
use std::sync::{Arc, LazyLock};

/// The crate's own schema file.
const BUILTIN_TOML: &str = include_str!("../../schema/tags.toml");

/// A schema file.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileDef {
    #[serde(default)]
    tags: IndexMap<String, TagDef>,
    #[serde(default)]
    nodes: IndexMap<String, NodeDef>,
    #[serde(default)]
    parse: Option<toml::Table>,
    #[serde(default)]
    render: Option<toml::Table>,
    #[serde(default)]
    validation: Option<ValidationDef>,
    #[serde(default)]
    functions: IndexMap<String, FunctionDef>,
}

/// The `[validation]` table: upstream's `config.validation`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidationDef {
    validate_functions: Option<bool>,
    environment: Option<String>,
}

/// One `[functions.<name>]` block: what a function takes (see "Functions"). Every field is
/// optional so that an overlay states only what it changes.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FunctionDef {
    description: Option<String>,
    returns: Option<Vec<String>>,
    #[serde(default)]
    parameters: IndexMap<String, AttrDef>,
}

impl FunctionDef {
    /// Field by field, `over` on top of `self` (D24).
    fn merge(&mut self, over: FunctionDef) {
        let FunctionDef {
            description,
            returns,
            parameters,
        } = over;
        set(&mut self.description, description);
        set(&mut self.returns, returns);
        merge_attributes(&mut self.parameters, parameters);
    }
}

/// A declared function, converted: what [`Bundle::apply`] merges under a config's entry.
#[derive(Debug, Clone)]
struct FunctionDecl {
    def: FunctionDef,
    returns: Option<Vec<TypeSpec>>,
    parameters: IndexMap<String, SchemaAttribute>,
}

impl FunctionDecl {
    fn of(name: &str, def: &FunctionDef) -> Result<FunctionDecl> {
        let at = format!("functions.{name}");
        let returns = match def.returns.as_deref() {
            None => None,
            Some(names) => Some(
                names
                    .iter()
                    .map(|n| {
                        BuiltinType::from_name(n)
                            .map(TypeSpec::Builtin)
                            .ok_or_else(|| {
                                error(format!(
                                    "{at}: unknown type {n:?} in `returns` (expected String, \
                                 Number, Boolean, Object or Array)"
                                ))
                            })
                    })
                    .collect::<Result<Vec<_>>>()?,
            ),
        };
        let parameters = def
            .parameters
            .iter()
            .map(|(key, attr)| Ok((key.clone(), attribute(&at, key, attr)?)))
            .collect::<Result<IndexMap<_, _>>>()?;
        Ok(FunctionDecl {
            def: def.clone(),
            returns,
            parameters,
        })
    }

    /// The declaration's stated fields on top of `function` (its code untouched).
    fn merge_into(&self, function: &mut ConfigFunction) {
        set(&mut function.description, self.def.description.clone());
        set(&mut function.returns, self.returns.clone());
        if self.parameters.is_empty() {
            return;
        }
        let parameters = function.parameters.get_or_insert_with(IndexMap::new);
        for (key, converted) in &self.parameters {
            match parameters.get_mut(key) {
                Some(existing) => merge_attribute(existing, &self.def.parameters[key], converted),
                None => {
                    parameters.insert(key.clone(), converted.clone());
                }
            }
        }
    }
}

/// The fields `def` states, from `converted` (its conversion), on top of `base`.
fn merge_attribute(base: &mut SchemaAttribute, def: &AttrDef, converted: &SchemaAttribute) {
    if def.ty.is_some() {
        base.attr_type = converted.attr_type.clone();
    }
    if def.required.is_some() {
        base.required = converted.required;
    }
    if def.default.is_some() {
        base.default = converted.default.clone();
    }
    if def.render.is_some() {
        base.render = converted.render.clone();
    }
    if def.description.is_some() {
        base.description = converted.description.clone();
    }
    if def.one_of.is_some() || def.pattern.is_some() {
        base.matches = converted.matches.clone();
        base.validate = converted.validate.clone();
    }
    if def.error_level.is_some() {
        base.error_level = converted.error_level.clone();
    }
}

/// One `[tags.<name>]` block. Every field is optional so that an overlay states only what it
/// changes.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TagDef {
    description: Option<String>,
    render: Option<RenderDef>,
    children: Option<Vec<String>>,
    self_closing: Option<bool>,
    inline: Option<bool>,
    #[serde(default)]
    attributes: IndexMap<String, AttrDef>,
    engine: Option<String>,
    alias_of: Option<String>,
    builtin: Option<bool>,
    layer: Option<String>,
}

/// One `[nodes.<type>]` block.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct NodeDef {
    description: Option<String>,
    #[serde(default)]
    attributes: IndexMap<String, AttrDef>,
}

/// One `attributes.<name>` block.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct AttrDef {
    #[serde(rename = "type")]
    ty: Option<Vec<String>>,
    required: Option<bool>,
    default: Option<Scalar>,
    render: Option<RenderDef>,
    description: Option<String>,
    one_of: Option<Vec<Scalar>>,
    pattern: Option<String>,
    error_level: Option<String>,
}

/// `render = true`, `render = false` or `render = "name"`.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum RenderDef {
    Bool(bool),
    Name(String),
}

impl TagDef {
    /// Field by field, `over` on top of `self` (D24).
    fn merge(&mut self, over: TagDef) {
        let TagDef {
            description,
            render,
            children,
            self_closing,
            inline,
            attributes,
            engine,
            alias_of,
            builtin,
            layer,
        } = over;
        set(&mut self.description, description);
        set(&mut self.render, render);
        set(&mut self.children, children);
        set(&mut self.self_closing, self_closing);
        set(&mut self.inline, inline);
        set(&mut self.engine, engine);
        set(&mut self.alias_of, alias_of);
        set(&mut self.builtin, builtin);
        set(&mut self.layer, layer);
        merge_attributes(&mut self.attributes, attributes);
    }

    /// Does it give anything a built-in tag cannot take (rule 3)?
    fn replaces_a_builtin(&self) -> bool {
        self.render.is_some()
            || self.children.is_some()
            || self.self_closing.is_some()
            || self.inline.is_some()
            || self.alias_of.is_some()
            || self.layer.is_some()
            || self.builtin == Some(false)
    }
}

impl AttrDef {
    fn merge(&mut self, over: AttrDef) {
        let AttrDef {
            ty,
            required,
            default,
            render,
            description,
            one_of,
            pattern,
            error_level,
        } = over;
        set(&mut self.ty, ty);
        set(&mut self.required, required);
        set(&mut self.default, default);
        set(&mut self.render, render);
        set(&mut self.description, description);
        set(&mut self.one_of, one_of);
        set(&mut self.pattern, pattern);
        set(&mut self.error_level, error_level);
    }

    /// Does it give more than a description (what an attribute a built-in has may take)?
    fn more_than_a_description(&self) -> bool {
        self.ty.is_some()
            || self.required.is_some()
            || self.default.is_some()
            || self.render.is_some()
            || self.one_of.is_some()
            || self.pattern.is_some()
            || self.error_level.is_some()
    }
}

fn set<T>(slot: &mut Option<T>, over: Option<T>) {
    if over.is_some() {
        *slot = over;
    }
}

fn merge_attributes(base: &mut IndexMap<String, AttrDef>, over: IndexMap<String, AttrDef>) {
    for (name, attr) in over {
        match base.get_mut(&name) {
            Some(existing) => existing.merge(attr),
            None => {
                base.insert(name, attr);
            }
        }
    }
}

fn error(message: impl std::fmt::Display) -> MarkdocError {
    MarkdocError::ParseError(format!("schema: {message}"))
}

/// What the bundle adds to one of upstream's schemas (a built-in tag, or a node).
#[derive(Debug, Clone, Default)]
struct Additions {
    description: Option<String>,
    /// Attributes the schema did not have.
    attributes: IndexMap<String, SchemaAttribute>,
    /// Descriptions of attributes it had.
    descriptions: IndexMap<String, String>,
}

impl Additions {
    /// The additions an entry makes to `base` (named `what` in errors), under rule 3's terms.
    fn of(
        what: &str,
        base: &Schema,
        description: &Option<String>,
        attributes: &IndexMap<String, AttrDef>,
    ) -> Result<Additions> {
        let mut out = Additions {
            description: description.clone(),
            ..Default::default()
        };
        let existing = base.attributes.as_ref();
        for (name, attr) in attributes {
            if existing.is_some_and(|a| a.contains_key(name)) {
                if attr.more_than_a_description() {
                    return Err(error(format!(
                        "{what}.{name}: upstream's schema has this attribute; only its \
                         `description` may be given"
                    )));
                }
                if let Some(d) = &attr.description {
                    out.descriptions.insert(name.clone(), d.clone());
                }
            } else {
                out.attributes
                    .insert(name.clone(), attribute(what, name, attr)?);
            }
        }
        Ok(out)
    }

    fn apply_to(&self, schema: &mut Schema) {
        if self.description.is_some() {
            schema.description = self.description.clone();
        }
        if self.attributes.is_empty() && self.descriptions.is_empty() {
            return;
        }
        let attributes = schema.attributes.get_or_insert_with(IndexMap::new);
        for (name, description) in &self.descriptions {
            if let Some(attribute) = attributes.get_mut(name) {
                attribute.description = Some(description.clone());
            }
        }
        for (name, attribute) in &self.attributes {
            js_insert(attributes, name.clone(), attribute.clone());
        }
    }
}

/// A tag as the bundle holds it after conversion.
#[derive(Debug, Clone)]
enum Entry {
    /// A schema of its own.
    Own(Schema),
    /// One of upstream's built-in tags (`base`; the tag's own name, or the canonical name of
    /// an alias of one) with additions.
    Extends { base: String, additions: Additions },
}

/// The schema data of a file and its overlays (see the module's documentation).
#[derive(Debug, Clone, Default)]
pub struct Bundle {
    /// The merged data, kept for later overlays and for the metadata.
    tag_defs: IndexMap<String, TagDef>,
    node_defs: IndexMap<String, NodeDef>,
    parse: toml::Table,
    render: toml::Table,
    validation: ValidationDef,
    function_defs: IndexMap<String, FunctionDef>,
    /// The converted tags, node additions and function declarations.
    tags: IndexMap<String, Entry>,
    nodes: IndexMap<String, Additions>,
    functions: IndexMap<String, FunctionDecl>,
}

static BUILTIN: LazyLock<Bundle> = LazyLock::new(|| {
    Bundle::from_toml(BUILTIN_TOML)
        .unwrap_or_else(|e| panic!("the bundled schema/tags.toml does not load: {e}"))
});

impl Bundle {
    /// An empty bundle: no tags, no node additions, default parse options.
    pub fn new() -> Bundle {
        Bundle::default()
    }

    /// The crate's own schema, `schema/tags.toml` (loaded once per process).
    pub fn builtin() -> Bundle {
        BUILTIN.clone()
    }

    /// A bundle of one schema file.
    pub fn from_toml(text: &str) -> Result<Bundle> {
        let mut bundle = Bundle::new();
        bundle.overlay(text)?;
        Ok(bundle)
    }

    /// Merges a schema file into the bundle (see "Overlays"). On `Err` the bundle is
    /// unchanged.
    pub fn overlay(&mut self, text: &str) -> Result<()> {
        let file: FileDef = toml::from_str(text).map_err(error)?;
        let mut tag_defs = self.tag_defs.clone();
        for (name, def) in file.tags {
            match tag_defs.get_mut(&name) {
                Some(existing) => existing.merge(def),
                None => {
                    tag_defs.insert(name, def);
                }
            }
        }
        let mut node_defs = self.node_defs.clone();
        for (name, def) in file.nodes {
            let existing = node_defs.entry(name).or_default();
            set(&mut existing.description, def.description);
            merge_attributes(&mut existing.attributes, def.attributes);
        }
        let mut parse = self.parse.clone();
        parse.extend(file.parse.unwrap_or_default());
        let mut render = self.render.clone();
        render.extend(file.render.unwrap_or_default());
        // Both tables must read as parse options.
        let mut options = ParseOptions::default();
        options.apply_toml(&toml::Value::Table(parse.clone()), "parse")?;
        options.apply_toml(&toml::Value::Table(render.clone()), "render")?;
        let mut validation = self.validation.clone();
        if let Some(over) = file.validation {
            set(&mut validation.validate_functions, over.validate_functions);
            set(&mut validation.environment, over.environment);
        }
        let mut function_defs = self.function_defs.clone();
        for (name, def) in file.functions {
            match function_defs.get_mut(&name) {
                Some(existing) => existing.merge(def),
                None => {
                    function_defs.insert(name, def);
                }
            }
        }

        let tags = convert_tags(&tag_defs)?;
        let nodes = convert_nodes(&node_defs)?;
        let functions = function_defs
            .iter()
            .map(|(name, def)| Ok((name.clone(), FunctionDecl::of(name, def)?)))
            .collect::<Result<IndexMap<_, _>>>()?;
        *self = Bundle {
            tag_defs,
            node_defs,
            parse,
            render,
            validation,
            function_defs,
            tags,
            nodes,
            functions,
        };
        Ok(())
    }

    /// A config with the bundle's tags and node schemas (see [`Bundle::apply`]); the rest is
    /// empty, as [`Config::default()`] is.
    pub fn config(&self) -> Config {
        let mut config = Config::default();
        self.apply(&mut config);
        config
    }

    /// Puts the bundle's tags, node schemas, `[validation]` and function declarations into
    /// `config`. A tag of the bundle's replaces the config's entry of that name; the built-in
    /// tags and the nodes the bundle extends are extended where they are (the config's own
    /// entry, else upstream's default), and stay hidden where the config hides them (`None`).
    /// Tags of a layer are left out. A `[validation]` key the bundle states replaces the
    /// config's. A declared function merges, field by field, under the config's entry (the
    /// code a tool registered, else upstream's default of that name), stays hidden where the
    /// config hides it, and is added without code where neither has it (see "Functions" and
    /// [`Bundle::check_functions`]).
    pub fn apply(&self, config: &mut Config) {
        // Built-in tags before their aliases, so that an alias sees the extended schema.
        let (direct, aliases): (Vec<_>, Vec<_>) = self
            .tags
            .iter()
            .filter(|(name, _)| self.layer(name).is_none())
            .partition(|(name, entry)| match entry {
                Entry::Extends { base, .. } => base == *name,
                Entry::Own(_) => true,
            });
        for (name, entry) in direct.into_iter().chain(aliases) {
            match entry {
                Entry::Own(schema) => {
                    config.tags.insert(name.clone(), Some(schema.clone()));
                }
                Entry::Extends { base, additions } => {
                    let schema = match config.tags.get(base) {
                        Some(found) => found.clone(),
                        None => default_tags().get(base).cloned().flatten(),
                    };
                    if let Some(mut schema) = schema {
                        additions.apply_to(&mut schema);
                        config.tags.insert(name.clone(), Some(schema));
                    }
                }
            }
        }
        for (name, additions) in &self.nodes {
            let schema = match config.nodes.get(name) {
                Some(found) => found.clone(),
                None => default_nodes().get(name).cloned().flatten(),
            };
            if let Some(mut schema) = schema {
                additions.apply_to(&mut schema);
                config.nodes.insert(name.clone(), Some(schema));
            }
        }
        if let Some(on) = self.validation.validate_functions {
            config.validation.validate_functions = on;
        }
        if let Some(environment) = &self.validation.environment {
            config.validation.environment = Some(environment.clone());
        }
        for (name, declaration) in &self.functions {
            let function = match config.functions.get(name) {
                Some(None) => continue,
                Some(Some(found)) => found.clone(),
                None => default_functions()
                    .get(name)
                    .cloned()
                    .flatten()
                    .unwrap_or_default(),
            };
            let mut function = function;
            declaration.merge_into(&mut function);
            config.functions.insert(name.clone(), Some(function));
        }
    }

    /// The functions the bundle declares (`[functions.<name>]`), in file order.
    pub fn function_names(&self) -> impl Iterator<Item = &str> {
        self.function_defs.keys().map(String::as_str)
    }

    /// The functions the bundle declares that `config` cannot run: hidden there (`None`), or
    /// without a `transform` in the config's entry and in upstream's default of that name.
    /// A call of one resolves to `null` without an error (MIGRATION-MAP R3).
    pub fn functions_without_code(&self, config: &Config) -> Vec<String> {
        self.function_defs
            .keys()
            .filter(|name| {
                let runs = match config.functions.get(*name) {
                    Some(Some(function)) => function.transform.is_some(),
                    Some(None) => false,
                    None => default_functions()
                        .get(*name)
                        .is_some_and(|f| f.as_ref().is_some_and(|f| f.transform.is_some())),
                };
                !runs
            })
            .cloned()
            .collect()
    }

    /// `Err` naming every function the bundle declares that `config` cannot run
    /// ([`Bundle::functions_without_code`]). A renderer calls it once its functions are
    /// registered and the bundle applied, and refuses to start on `Err` (MIGRATION-MAP R3):
    /// otherwise each call of such a function renders as nothing, and hides what it guards.
    pub fn check_functions(&self, config: &Config) -> Result<()> {
        let missing = self.functions_without_code(config);
        if missing.is_empty() {
            return Ok(());
        }
        Err(error(format!(
            "declared functions without code: {} (register a transform for each before \
             starting)",
            missing.join(", ")
        )))
    }

    /// The schema the bundle gives tag `name` (a tag of a layer included), with upstream's
    /// built-in under it where it extends one.
    pub fn schema(&self, name: &str) -> Option<Schema> {
        match self.tags.get(name)? {
            Entry::Own(schema) => Some(schema.clone()),
            Entry::Extends { base, additions } => {
                let mut schema = default_tags().get(base).cloned().flatten()?;
                additions.apply_to(&mut schema);
                Some(schema)
            }
        }
    }

    /// The schemas of the tags `layer` implements (`layer = "<name>"` in the data), for that
    /// layer to add its behaviour to.
    pub fn layer_schemas(&self, layer: &str) -> IndexMap<String, Schema> {
        self.tags
            .keys()
            .filter(|name| self.layer(name) == Some(layer))
            .filter_map(|name| Some((name.clone(), self.schema(name)?)))
            .collect()
    }

    /// The layer tag `name` needs, if any (an alias needs its canonical tag's).
    pub fn layer(&self, name: &str) -> Option<&str> {
        let def = self.tag_defs.get(name)?;
        match (&def.layer, &def.alias_of) {
            (Some(layer), _) => Some(layer),
            (None, Some(target)) => self.tag_defs.get(target)?.layer.as_deref(),
            (None, None) => None,
        }
    }

    /// The tags the bundle declares, aliases and tags of layers included, in file order.
    pub fn tag_names(&self) -> impl Iterator<Item = &str> {
        self.tag_defs.keys().map(String::as_str)
    }

    /// A tag's description, as the data gives it.
    pub fn description(&self, name: &str) -> Option<&str> {
        self.tag_defs.get(name)?.description.as_deref()
    }

    /// Every alias, as `name -> canonical name`.
    pub fn aliases(&self) -> IndexMap<String, String> {
        self.tag_defs
            .iter()
            .filter_map(|(name, def)| Some((name.clone(), def.alias_of.clone()?)))
            .collect()
    }

    /// Every engine label (`engine = "..."`: an opaque label naming what implements the tag,
    /// which editors use to colour control flow), as `name -> label`.
    pub fn engines(&self) -> IndexMap<String, String> {
        self.tag_defs
            .iter()
            .filter_map(|(name, def)| Some((name.clone(), def.engine.clone()?)))
            .collect()
    }

    /// The tag a name means, following `alias_of` once; `name` when it is no alias.
    pub fn canonical_name<'a>(&'a self, name: &'a str) -> &'a str {
        self.tag_defs
            .get(name)
            .and_then(|def| def.alias_of.as_deref())
            .unwrap_or(name)
    }

    /// The options every tool reading the corpus parses with: the `[parse]` table.
    pub fn parse_options(&self) -> ParseOptions {
        let mut options = ParseOptions::default();
        options
            .apply_toml(&toml::Value::Table(self.parse.clone()), "parse")
            .expect("checked when loaded");
        options
    }

    /// The options a renderer parses with: [`Bundle::parse_options`] with the `[render]` table
    /// applied on top (the typographer, for one, which changes text but not what is a tag).
    pub fn render_options(&self) -> ParseOptions {
        let mut options = self.parse_options();
        options
            .apply_toml(&toml::Value::Table(self.render.clone()), "render")
            .expect("checked when loaded");
        options
    }
}

/// The built-in bundle's aliases ([`Bundle::aliases`] of [`Bundle::builtin`]).
pub fn aliases() -> &'static IndexMap<String, String> {
    static ALIASES: LazyLock<IndexMap<String, String>> = LazyLock::new(|| BUILTIN.aliases());
    &ALIASES
}

/// The built-in bundle's engine labels ([`Bundle::engines`] of [`Bundle::builtin`]).
pub fn engines() -> &'static IndexMap<String, String> {
    static ENGINES: LazyLock<IndexMap<String, String>> = LazyLock::new(|| BUILTIN.engines());
    &ENGINES
}

/// The tag a name means in the built-in bundle ([`Bundle::canonical_name`]).
pub fn canonical_name(name: &str) -> &str {
    aliases().get(name).map(String::as_str).unwrap_or(name)
}

/// The names of upstream's built-in tags.
fn is_builtin(name: &str) -> bool {
    default_tags().contains_key(name)
}

fn convert_tags(defs: &IndexMap<String, TagDef>) -> Result<IndexMap<String, Entry>> {
    let mut out = IndexMap::with_capacity(defs.len());
    for (name, def) in defs {
        if is_builtin(name) && def.replaces_a_builtin() {
            return Err(error(format!(
                "{name}: upstream's built-in tag; an entry may give only `description`, \
                 `engine`, `builtin` and new attributes"
            )));
        }
        let entry = if let Some(target) = &def.alias_of {
            alias(name, target, def, defs)?
        } else if is_builtin(name) {
            let base = default_tags()[name.as_str()].as_ref().expect("a built-in");
            Entry::Extends {
                base: name.clone(),
                additions: Additions::of(name, base, &def.description, &def.attributes)?,
            }
        } else {
            if def.builtin == Some(true) {
                return Err(error(format!(
                    "{name}: `builtin = true`, but upstream has no built-in tag of that name"
                )));
            }
            Entry::Own(own_schema(name, def)?)
        };
        out.insert(name.clone(), entry);
    }
    Ok(out)
}

/// An alias's entry: the canonical tag's, with the alias's description.
fn alias(name: &str, target: &str, def: &TagDef, defs: &IndexMap<String, TagDef>) -> Result<Entry> {
    if def.render.is_some()
        || def.children.is_some()
        || def.self_closing.is_some()
        || def.inline.is_some()
        || !def.attributes.is_empty()
        || def.builtin.is_some()
        || def.layer.is_some()
    {
        return Err(error(format!(
            "{name}: an alias takes only `alias_of`, `description` and `engine`"
        )));
    }
    let canonical = match defs.get(target) {
        Some(canonical) if canonical.alias_of.is_some() => {
            return Err(error(format!(
                "{name}: alias_of names {target:?}, which is itself an alias"
            )));
        }
        Some(canonical) => Some(canonical),
        None if is_builtin(target) => None,
        None => {
            return Err(error(format!(
                "{name}: alias_of names {target:?}, which is not defined"
            )));
        }
    };
    let description = def
        .description
        .clone()
        .or_else(|| canonical.and_then(|c| c.description.clone()));
    if is_builtin(target) {
        let base = default_tags()[target].as_ref().expect("a built-in");
        let (descr, attributes) = match canonical {
            Some(c) => (&c.description, &c.attributes),
            None => (&None, &IndexMap::new()),
        };
        let mut additions = Additions::of(target, base, descr, attributes)?;
        additions.description = description;
        return Ok(Entry::Extends {
            base: target.to_string(),
            additions,
        });
    }
    let canonical = canonical.expect("defined");
    let mut schema = own_schema(target, canonical)?;
    schema.description = description;
    Ok(Entry::Own(schema))
}

/// The schema of a tag that is neither a built-in nor an alias.
fn own_schema(name: &str, def: &TagDef) -> Result<Schema> {
    let render = match &def.render {
        None | Some(RenderDef::Bool(true)) => Some(name.to_string()),
        Some(RenderDef::Bool(false)) => None,
        Some(RenderDef::Name(n)) if n.is_empty() => None,
        Some(RenderDef::Name(n)) => Some(n.clone()),
    };
    let attributes = if def.attributes.is_empty() {
        None
    } else {
        let mut map = IndexMap::with_capacity(def.attributes.len());
        for (attr, a) in &def.attributes {
            js_insert(&mut map, attr.clone(), attribute(name, attr, a)?);
        }
        Some(map)
    };
    Ok(Schema {
        render: render.map(|name| Value::string(name.as_str())),
        attributes,
        self_closing: def.self_closing.unwrap_or(false),
        inline: def.inline.map(Value::Boolean),
        validate: def.children.clone().map(child_tags),
        description: def.description.clone(),
        ..Default::default()
    })
}

fn convert_nodes(defs: &IndexMap<String, NodeDef>) -> Result<IndexMap<String, Additions>> {
    let mut out = IndexMap::with_capacity(defs.len());
    for (name, def) in defs {
        let Some(Some(base)) = default_nodes().get(name) else {
            return Err(error(format!(
                "nodes.{name}: upstream has no node schema of that type"
            )));
        };
        out.insert(
            name.clone(),
            Additions::of(
                &format!("nodes.{name}"),
                base,
                &def.description,
                &def.attributes,
            )?,
        );
    }
    Ok(out)
}

/// One attribute's schema (rules 5 and 7).
fn attribute(tag: &str, name: &str, def: &AttrDef) -> Result<SchemaAttribute> {
    let at = || format!("{tag}.{name}");
    let attr_type = match def.ty.as_deref() {
        None => None,
        Some([]) => return Err(error(format!("{}: `type` is empty", at()))),
        Some(names) => {
            let mut types =
                names
                    .iter()
                    .map(|n| {
                        BuiltinType::from_name(n).map(TypeSpec::Builtin).ok_or_else(|| {
                        error(format!(
                            "{}: unknown type {n:?} (expected String, Number, Boolean, Object \
                             or Array)",
                            at()
                        ))
                    })
                    })
                    .collect::<Result<Vec<_>>>()?;
            Some(if types.len() == 1 {
                types.remove(0)
            } else {
                TypeSpec::Union(types)
            })
        }
    };
    let error_level = match def.error_level.as_deref() {
        None => None,
        Some(level) => Some(level_named(level).ok_or_else(|| {
            error(format!(
                "{}: unknown error_level {level:?} (expected debug, info, warning, error or \
                 critical)",
                at()
            ))
        })?),
    };
    let one_of = def
        .one_of
        .as_ref()
        .map(|values| values.iter().map(Value::from_scalar).collect::<Vec<_>>());
    let pattern = match &def.pattern {
        None => None,
        Some(p) => Some(Arc::new(
            JsRegExp::new(p, "").map_err(|e| error(format!("{}: pattern: {e}", at())))?,
        )),
    };
    let (matches, validate) = match (one_of, pattern) {
        (Some(list), Some(regexp)) => (
            None,
            Some(one_of_or_pattern(
                list,
                regexp,
                error_level.clone().unwrap_or(ValidationLevel::Error),
            )),
        ),
        (Some(list), None) => (Some(Matches::Array(list)), None),
        (None, Some(regexp)) => (Some(Matches::RegExp(regexp)), None),
        (None, None) => (None, None),
    };
    Ok(SchemaAttribute {
        kind: SchemaKind::Object,
        attr_type,
        render: def.render.as_ref().map(|r| match r {
            RenderDef::Bool(b) => Value::Boolean(*b),
            RenderDef::Name(n) => Value::string(n.as_str()),
        }),
        default: def.default.as_ref().map(Value::from_scalar),
        required: def.required.unwrap_or(false),
        matches,
        validate,
        error_level,
        description: def.description.clone(),
    })
}

fn level_named(name: &str) -> Option<ValidationLevel> {
    Some(match name {
        "debug" => ValidationLevel::Debug,
        "info" => ValidationLevel::Info,
        "warning" => ValidationLevel::Warning,
        "error" => ValidationLevel::Error,
        "critical" => ValidationLevel::Critical,
        _ => return None,
    })
}

/// A validation error without a location (the validator places it at the node).
fn validation_error(id: &str, level: ValidationLevel, message: String) -> ValidationError {
    ValidationError::new(id, level, message)
}

/// `String(value)` for the values an attribute holds after the validator's checks (strings,
/// numbers, booleans, null, arrays and plain objects).
fn js_text(value: &Scalar) -> String {
    match value {
        Scalar::Undefined => "undefined".into(),
        Scalar::Null => "null".into(),
        Scalar::Boolean(b) => b.to_string(),
        Scalar::Number(n) => crate::markdown_it::js::number_to_string(*n),
        Scalar::String(s) => s.clone(),
        Scalar::Array(items) => items
            .iter()
            .map(|item| match item {
                Scalar::Undefined | Scalar::Null => String::new(),
                other => js_text(other),
            })
            .collect::<Vec<_>>()
            .join(","),
        Scalar::Object(_) | Scalar::Variable(_) | Scalar::Function(_) => "[object Object]".into(),
        Scalar::Tag(_) => "[object Object]".into(),
        // A JavaScript object the plain variants cannot hold: `String()` as JavaScript makes
        // it (through its prototype chain).
        Scalar::Js(js) => js
            .value()
            .to_js_string()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "[object Object]".into()),
    }
}

/// `SameValueZero(a, b)`, as `Array.prototype.includes` compares.
fn same_value_zero(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Undefined, Value::Undefined) | (Value::Null, Value::Null) => true,
        (Value::Boolean(a), Value::Boolean(b)) => a == b,
        (Value::Number(a), Value::Number(b)) => a == b || (a.is_nan() && b.is_nan()),
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Array(a), Value::Array(b)) => Arc::ptr_eq(a, b),
        (Value::Object(a), Value::Object(b)) => Arc::ptr_eq(a, b),
        (Value::Tag(a), Value::Tag(b)) => Arc::ptr_eq(a, b),
        _ => false,
    }
}

/// `one_of` and `pattern` together (rule 5): a value in the list or one the pattern matches.
fn one_of_or_pattern(
    list: Vec<Value>,
    regexp: Arc<JsRegExp>,
    level: ValidationLevel,
) -> AttributeValidateFn {
    let shown = serde_json::to_string(
        &list
            .iter()
            .map(|v| v.to_scalar().map(|s| s.to_json()).unwrap_or_default())
            .collect::<Vec<_>>(),
    )
    .unwrap_or_default();
    Arc::new(
        move |value: &Scalar, _cx: &ValidationContext<'_>, key: &str| {
            let as_value = Value::from_scalar(value);
            let text = js_text(value);
            // The RegExp throws where V8 does (`RangeError` past its backtracking limit);
            // the validator passes that on as upstream's thrown error.
            if list.iter().any(|v| same_value_zero(v, &as_value))
                || regexp.test(&text).map_err(MarkdocError::ValidationError)?
            {
                return Ok(Vec::new());
            }
            Ok(vec![validation_error(
                "attribute-value-invalid",
                level.clone(),
                format!(
                    "Attribute '{key}' must match one of {shown} or {}. Got '{text}' instead.",
                    regexp.to_js_string()
                ),
            )])
        },
    )
}

/// A `children` list of tag names (rule 6): every direct child is a tag named in it.
fn child_tags(allowed: Vec<String>) -> ValidateFn {
    Arc::new(move |node: &Node, _cx: &ValidationContext<'_>| {
        let parent = node.tag.as_deref().unwrap_or(node.node_type.as_str());
        Ok(node
            .children
            .iter()
            .filter_map(|child| {
                let name = match (&child.node_type, &child.tag) {
                    (NodeType::Tag, Some(tag)) => tag.as_str(),
                    _ => child.node_type.as_str(),
                };
                (!allowed.iter().any(|a| a == name)).then(|| {
                    validation_error(
                        "child-invalid",
                        ValidationLevel::Warning,
                        format!("Can't nest '{name}' in '{parent}'"),
                    )
                })
            })
            .collect())
    })
}

#[cfg(test)]
mod tests;
