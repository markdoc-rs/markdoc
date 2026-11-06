use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum MarkdocError {
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("Transform error: {0}")]
    TransformError(String),
    #[error("Validation error: {0}")]
    ValidationError(String),
}

pub type Result<T> = std::result::Result<T, MarkdocError>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NodeType {
    #[serde(rename = "document")]
    Document,
    #[serde(rename = "heading")]
    Heading,
    #[serde(rename = "paragraph")]
    Paragraph,
    #[serde(rename = "text")]
    Text,
    #[serde(rename = "code")]
    Code,
    #[serde(rename = "fence")]
    Fence,
    #[serde(rename = "blockquote")]
    Blockquote,
    #[serde(rename = "list")]
    List,
    #[serde(rename = "item")]
    Item,
    #[serde(rename = "link")]
    Link,
    #[serde(rename = "image")]
    Image,
    #[serde(rename = "em")]
    Em,
    #[serde(rename = "strong")]
    Strong,
    #[serde(rename = "s")]
    Strikethrough,
    #[serde(rename = "hr")]
    Hr,
    #[serde(rename = "hardbreak")]
    Hardbreak,
    #[serde(rename = "softbreak")]
    Softbreak,
    #[serde(rename = "table")]
    Table,
    #[serde(rename = "thead")]
    Thead,
    #[serde(rename = "tbody")]
    Tbody,
    #[serde(rename = "tr")]
    Tr,
    #[serde(rename = "th")]
    Th,
    #[serde(rename = "td")]
    Td,
    #[serde(rename = "tag")]
    Tag,
    #[serde(rename = "error")]
    Error,
    #[serde(rename = "comment")]
    Comment,
    #[serde(rename = "inline")]
    Inline,
    #[serde(rename = "node")]
    Node,
}

impl std::fmt::Display for NodeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            NodeType::Document => "document",
            NodeType::Heading => "heading",
            NodeType::Paragraph => "paragraph",
            NodeType::Text => "text",
            NodeType::Code => "code",
            NodeType::Fence => "fence",
            NodeType::Blockquote => "blockquote",
            NodeType::List => "list",
            NodeType::Item => "item",
            NodeType::Link => "link",
            NodeType::Image => "image",
            NodeType::Em => "em",
            NodeType::Strong => "strong",
            NodeType::Strikethrough => "s",
            NodeType::Hr => "hr",
            NodeType::Hardbreak => "hardbreak",
            NodeType::Softbreak => "softbreak",
            NodeType::Table => "table",
            NodeType::Thead => "thead",
            NodeType::Tbody => "tbody",
            NodeType::Tr => "tr",
            NodeType::Th => "th",
            NodeType::Td => "td",
            NodeType::Tag => "tag",
            NodeType::Error => "error",
            NodeType::Comment => "comment",
            NodeType::Inline => "inline",
            NodeType::Node => "node",
        };
        write!(f, "{}", s)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    pub file: Option<String>,
    pub start: LocationEdge,
    pub end: LocationEdge,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocationEdge {
    /// 1-based, as every editor counts lines.
    pub line: usize,
    /// 0-based, in **UTF-16 code units** -- what the Language Server
    /// Protocol counts, and not the same as bytes or as `char`s the moment
    /// a document contains an accent or an emoji.
    pub character: Option<usize>,
    /// Byte offset into the original file.
    ///
    /// Carried as well as the line/character pair because the two answer
    /// different questions: an editor wants line and column, and a lint rule
    /// wants to slice the source it is complaining about. Deriving one from
    /// the other at every call site means re-indexing the file at every call
    /// site.
    ///
    /// `#[serde(default)]` so an AST serialised before this field existed
    /// still deserialises; it reads back as 0, which is why nothing should
    /// treat 0 as meaningful without checking `line` too.
    #[serde(default)]
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ValidationLevel {
    #[serde(rename = "debug")]
    Debug,
    #[serde(rename = "info")]
    Info,
    #[serde(rename = "warning")]
    Warning,
    #[serde(rename = "error")]
    Error,
    #[serde(rename = "critical")]
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationError {
    pub id: String,
    pub level: ValidationLevel,
    pub message: String,
    pub location: Option<Location>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Scalar {
    Null,
    Boolean(bool),
    Number(f64),
    String(String),
    Array(Vec<Scalar>),
    Object(HashMap<String, Scalar>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttributeValue {
    #[serde(rename = "type")]
    pub attr_type: String,
    pub name: String,
    pub value: Scalar,
}

#[derive(Debug, Clone)]
pub enum ValidationType {
    String,
    Number,
    Boolean,
    Object,
    Array,
}

#[derive(Debug, Clone)]
pub struct SchemaAttribute {
    pub attr_type: Option<Vec<ValidationType>>,
    pub render: Option<SchemaRender>,
    pub default: Option<Scalar>,
    pub required: bool,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub enum SchemaRender {
    Bool(bool),
    String(String),
}

#[derive(Debug, Clone)]
pub struct Schema {
    pub render: Option<String>,
    pub children: Option<Vec<String>>,
    pub attributes: Option<HashMap<String, SchemaAttribute>>,
    pub self_closing: bool,
    pub inline: bool,
    pub description: Option<String>,
}

// `Default` lives in `tags.rs` so it can populate `nodes`/`tags` from
// `schema::default_nodes()` / `tags::default_tags()` without an import cycle.
#[derive(Debug, Clone)]
pub struct Config {
    pub nodes: HashMap<String, Schema>,
    pub tags: HashMap<String, Schema>,
    pub variables: HashMap<String, Scalar>,
    pub functions: HashMap<String, ConfigFunction>,
    pub partials: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ConfigFunction {
    pub returns: Option<Vec<ValidationType>>,
    pub parameters: Option<HashMap<String, SchemaAttribute>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tag {
    pub name: String,
    pub attributes: HashMap<String, Scalar>,
    pub children: Vec<RenderableTreeNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RenderableTreeNode {
    Tag(Box<Tag>),
    Scalar(Scalar),
}

#[derive(Debug, Clone)]
pub struct ParserArgs {
    pub file: Option<String>,
    pub slots: bool,
    pub location: bool,
}

/// Per-document evaluation context for the transformer.
///
/// Holds the named variable namespaces — typically `markdoc` (with
/// `frontmatter` underneath), `config`, and `global` — used to resolve
/// expressions like `$markdoc.frontmatter.title` or `$config.modelName`.
///
/// Not in scope for stage 0.1 (added later as the milestone progresses):
///   - Partial resolver (0.3)
///   - Cycle-detection set for partial inclusion (0.3)
#[derive(Debug, Clone, Default)]
pub struct Context {
    pub variables: HashMap<String, Scalar>,
}

impl Context {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a top-level variable namespace (e.g. `"config"`).
    pub fn with_variable(mut self, name: impl Into<String>, value: Scalar) -> Self {
        self.variables.insert(name.into(), value);
        self
    }

    /// Convenience: place `fm` under `markdoc.frontmatter`.
    pub fn with_frontmatter(self, fm: Scalar) -> Self {
        let mut markdoc = match self.variables.get("markdoc") {
            Some(Scalar::Object(o)) => o.clone(),
            _ => HashMap::new(),
        };
        markdoc.insert("frontmatter".to_string(), fm);
        self.with_variable("markdoc", Scalar::Object(markdoc))
    }

    /// Convenience: bind `cfg` to `$config.*`.
    pub fn with_config(self, cfg: Scalar) -> Self {
        self.with_variable("config", cfg)
    }

    /// Convenience: bind `globals` to `$global.*`.
    pub fn with_globals(self, globals: Scalar) -> Self {
        self.with_variable("global", globals)
    }

    /// Walk a dotted path through the variable namespaces. Returns
    /// `Scalar::Null` for any segment that doesn't resolve, matching
    /// Markdoc's tolerant lookup semantics.
    pub fn resolve_variable(&self, path: &[String]) -> Scalar {
        if path.is_empty() {
            return Scalar::Null;
        }
        let mut current = match self.variables.get(&path[0]) {
            Some(s) => s.clone(),
            None => return Scalar::Null,
        };
        for key in &path[1..] {
            current = match current {
                Scalar::Object(map) => map.get(key).cloned().unwrap_or(Scalar::Null),
                // Bracket / numeric access into an array: `$a[0]` reaches
                // here as the path segment "0".
                Scalar::Array(arr) => key
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| arr.get(i).cloned())
                    .unwrap_or(Scalar::Null),
                _ => return Scalar::Null,
            };
        }
        current
    }
}
