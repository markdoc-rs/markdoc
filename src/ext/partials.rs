//! Partials: loading the files `{% partial file="..." /%}` names (an extension, off unless
//! called).
//!
//! Upstream Markdoc renders a partial from `config.partials[file]`, a document the caller
//! parsed beforehand; a file missing there transforms to nothing, and only `validate` reports
//! it (`attribute-value-invalid`). This module does the loading:
//!
//! - [`load_partials`] finds every `{% partial %}` of a document, and of each partial it
//!   loads, reads each file once through a [`PartialResolver`], parses it as the document was
//!   parsed (the same [`ParseOptions`], with `file` set to the partial's name), and returns
//!   the map for `config.partials`. A missing file, a `partial` tag without a string
//!   `file`, and a cycle of partials including each other are an `Err` here, at load time,
//!   instead of an empty render later. Upstream's partial tag then renders them, with its
//!   `variables=` scoping and `$$partial:filename`.
//! - [`expand_partials`] splices the partials into the tree instead, for callers whose AST
//!   passes need the expanded document before `transform`. A splice cannot scope variables:
//!   a partial tag's `variables` are dropped, and `$$partial:filename` is not bound.
//!
//! ```
//! use markdoc::ext::partials::{InMemoryPartialResolver, expand_partials, load_partials};
//! use markdoc::ext::{ParseOptions, parse};
//! use markdoc::next::{index, renderers::html, schema::Config};
//!
//! let resolver = InMemoryPartialResolver::new()
//!     .with("note.md", "Hello from {% $who %}")
//!     .with("footer.md", "{% partial file=\"note.md\" /%}");
//! let options = ParseOptions::default();
//! let doc = parse(
//!     "{% partial file=\"footer.md\" variables={who: \"me\"} /%}\n",
//!     &options,
//! )
//! .unwrap();
//!
//! let partials = load_partials(&doc, &resolver, &options).unwrap();
//! assert_eq!(partials.keys().collect::<Vec<_>>(), ["note.md", "footer.md"]);
//! let config = Config {
//!     partials: Some(partials.clone()),
//!     ..Config::default()
//! };
//! let tree = index::transform(&doc, &config).unwrap();
//! assert_eq!(
//!     html::render(&tree).unwrap(),
//!     "<article><p>Hello from me</p></article>"
//! );
//!
//! // The same, spliced into the AST (where nothing binds `$who` any more).
//! let expanded = expand_partials(&doc, &partials).unwrap();
//! assert!(expanded.walk().all(|n| n.tag.as_deref() != Some("partial")));
//! ```

use crate::ast::Node;
use crate::ext::{ParseOptions, parse};
use crate::next::schema::Partial;
use crate::types::{MarkdocError, NodeType, Result, Scalar};
use indexmap::IndexMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

/// Where partial files come from.
///
/// `canonicalize` gives the identity cycles are detected by. The default returns the path
/// unchanged, which suits in-memory resolvers; a filesystem resolver should resolve to an
/// absolute path, so that `./b.md` and `b.md` are recognised as the same file.
pub trait PartialResolver: Send + Sync {
    fn load(&self, path: &Path) -> Result<String>;

    fn canonicalize(&self, path: &Path) -> Result<PathBuf> {
        Ok(path.to_path_buf())
    }
}

/// Files under `root`: every `file=` of a `{% partial %}` is a path relative to it.
pub struct FsPartialResolver {
    pub root: PathBuf,
}

impl FsPartialResolver {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl PartialResolver for FsPartialResolver {
    fn load(&self, path: &Path) -> Result<String> {
        let full = self.root.join(path);
        std::fs::read_to_string(&full).map_err(|e| {
            MarkdocError::TransformError(format!(
                "Failed to read partial {:?}: {e}",
                full.display()
            ))
        })
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf> {
        let full = self.root.join(path);
        std::fs::canonicalize(&full).map_err(|e| {
            MarkdocError::TransformError(format!(
                "Failed to canonicalize partial path {:?}: {e}",
                full.display()
            ))
        })
    }
}

/// Partial sources held in memory (tests, or sources loaded from elsewhere).
#[derive(Debug, Default)]
pub struct InMemoryPartialResolver {
    pub files: HashMap<PathBuf, String>,
}

impl InMemoryPartialResolver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, path: impl Into<PathBuf>, content: impl Into<String>) -> Self {
        self.files.insert(path.into(), content.into());
        self
    }
}

impl PartialResolver for InMemoryPartialResolver {
    fn load(&self, path: &Path) -> Result<String> {
        self.files.get(path).cloned().ok_or_else(|| {
            MarkdocError::TransformError(format!("Partial not found: {}", path.display()))
        })
    }
}

fn error(message: impl Into<String>) -> MarkdocError {
    MarkdocError::TransformError(message.into())
}

fn is_partial(node: &Node) -> bool {
    node.node_type == NodeType::Tag && node.tag.as_deref() == Some("partial")
}

/// A partial tag's `file`, which must be a string written in the document.
fn file_of(node: &Node) -> Result<String> {
    match node.attributes.get("file") {
        Some(Scalar::String(file)) => Ok(file.clone()),
        _ => Err(error("{% partial %} requires a string `file` attribute")),
    }
}

/// The `file` of every partial tag in the tree, in document order (slots, fences and the
/// node itself included, as upstream's transform reaches them all).
fn files_in(node: &Node) -> Result<Vec<String>> {
    std::iter::once(node)
        .chain(node.walk())
        .filter(|n| is_partial(n))
        .map(file_of)
        .collect()
}

fn cycle(chain: impl IntoIterator<Item = String>) -> MarkdocError {
    let chain: Vec<String> = chain.into_iter().collect();
    error(format!(
        "Partial inclusion cycle detected: {}",
        chain.join(" -> ")
    ))
}

/// Every partial `document` includes, directly or through other partials, parsed: the map
/// for `config.partials`, keyed by the `file` as written (upstream looks a partial up by
/// that), in the order each file's loading finishes (a partial after the ones it includes).
///
/// Each file is loaded once, through `resolver` (`file` is the path it gets), and parsed with
/// `options` with `file` set to its name ([`ParseOptions::with_file`]), so that a partial is
/// read exactly as the document it is in. `Err` where rendering could not be right: a `partial` tag without
/// a string `file` (a variable cannot be resolved before transform), a file the resolver
/// cannot load or the tokenizer or parser rejects, and a cycle (a partial including itself,
/// directly or not), which names the chain of files (canonical paths). Any depth of
/// inclusion (no recursion).
pub fn load_partials(
    document: &Node,
    resolver: &dyn PartialResolver,
    options: &ParseOptions,
) -> Result<IndexMap<String, Partial>> {
    let mut loaded = IndexMap::new();
    load_partials_into(document, resolver, options, &mut loaded)?;
    Ok(loaded)
}

/// [`load_partials`] into a map the caller keeps, for a cache across documents: a `file`
/// already in `loaded` is neither read nor parsed again (nor are the partials it includes),
/// and every partial loaded is added. On `Err`, the partials finished before the error stay
/// in `loaded` (each one whole, with the partials it includes).
pub fn load_partials_into(
    document: &Node,
    resolver: &dyn PartialResolver,
    options: &ParseOptions,
    loaded: &mut IndexMap<String, Partial>,
) -> Result<()> {
    /// A document whose partials are being loaded: the partial it is (none for the root),
    /// the files it names, how many of them are done.
    struct Loading {
        partial: Option<(String, Node)>,
        files: Vec<String>,
        next: usize,
    }

    // The canonical paths of the partials being loaded (every `Loading` but the root's).
    let mut chain: Vec<PathBuf> = Vec::new();
    let mut stack = vec![Loading {
        partial: None,
        files: files_in(document)?,
        next: 0,
    }];
    while let Some(top) = stack.last_mut() {
        let Some(file) = top.files.get(top.next).cloned() else {
            let done = stack.pop().expect("the top");
            if let Some((file, node)) = done.partial {
                chain.pop();
                loaded.insert(file, Partial::Node(Arc::new(node)));
            }
            continue;
        };
        top.next += 1;
        if loaded.contains_key(&file) {
            continue;
        }
        let path = PathBuf::from(&file);
        let canonical = resolver.canonicalize(&path)?;
        if chain.contains(&canonical) {
            return Err(cycle(
                chain
                    .iter()
                    .chain([&canonical])
                    .map(|p| p.display().to_string()),
            ));
        }
        let source = resolver.load(&path)?;
        let node = parse(&source, &options.with_file(file.clone()))?;
        let files = files_in(&node)?;
        chain.push(canonical);
        stack.push(Loading {
            partial: Some((file, node)),
            files,
            next: 0,
        });
    }
    Ok(())
}

/// The files being spliced at a point of the tree, innermost last.
struct Chain {
    file: String,
    outer: Option<Rc<Chain>>,
}

impl Drop for Chain {
    /// Unlinks the chain from the inside out, so that dropping a long one never recurses.
    fn drop(&mut self) {
        let mut outer = self.outer.take();
        while let Some(link) = outer {
            match Rc::try_unwrap(link) {
                Ok(mut chain) => outer = chain.outer.take(),
                Err(_) => break,
            }
        }
    }
}

fn chain_has(chain: &Option<Rc<Chain>>, file: &str) -> bool {
    let mut link = chain.as_deref();
    while let Some(c) = link {
        if c.file == file {
            return true;
        }
        link = c.outer.as_deref();
    }
    false
}

fn chain_files(chain: &Option<Rc<Chain>>, last: &str) -> Vec<String> {
    let mut files = vec![last.to_string()];
    let mut link = chain.as_deref();
    while let Some(c) = link {
        files.push(c.file.clone());
        link = c.outer.as_deref();
    }
    files.reverse();
    files
}

/// `node` with every `{% partial file="..." /%}` replaced by the children of the partial's
/// document (its frontmatter, and the tag's `variables`, are dropped), from `partials` (what
/// [`load_partials`] returns, or a config's `partials`); partials inside partials are spliced
/// too. A partial tag at the root makes the result a document holding what it splices.
///
/// `Err` for a partial tag without a string `file`, a file `partials` does not have (or has
/// as a value other than a parsed document), and a cycle. Any depth (no recursion).
pub fn expand_partials(node: &Node, partials: &IndexMap<String, Partial>) -> Result<Node> {
    let mut root = if is_partial(node) {
        Node::new(
            NodeType::Document,
            std::iter::empty(),
            vec![node.clone()],
            None,
        )
    } else {
        node.clone()
    };
    let mut stack: Vec<(&mut Node, Option<Rc<Chain>>)> = vec![(&mut root, None)];
    while let Some((node, chain)) = stack.pop() {
        let mut children = Vec::with_capacity(node.children.len());
        let mut chains = Vec::with_capacity(node.children.len());
        // The children still to place, last first: a partial's are put back here, so that a
        // partial its content starts with is spliced in turn.
        let mut queue: Vec<(Node, Option<Rc<Chain>>)> = std::mem::take(&mut node.children)
            .into_iter()
            .rev()
            .map(|child| (child, chain.clone()))
            .collect();
        while let Some((child, outer)) = queue.pop() {
            if !is_partial(&child) {
                children.push(child);
                chains.push(outer);
                continue;
            }
            let file = file_of(&child)?;
            if chain_has(&outer, &file) {
                return Err(cycle(chain_files(&outer, &file)));
            }
            let parts: Vec<&Node> = match partials.get(&file) {
                Some(Partial::Node(part)) => vec![part.as_ref()],
                Some(Partial::Nodes(parts)) => parts.iter().collect(),
                Some(Partial::Value(_)) => {
                    return Err(error(format!("Partial `{file}` is not a parsed document")));
                }
                None => return Err(error(format!("Partial not found: {file}"))),
            };
            let inner = Some(Rc::new(Chain { file, outer }));
            for part in parts.into_iter().rev() {
                for spliced in part.children.iter().rev() {
                    queue.push((spliced.clone(), inner.clone()));
                }
            }
        }
        node.children = children;
        let Node {
            children, slots, ..
        } = node;
        for slot in slots.values_mut() {
            stack.push((slot, chain.clone()));
        }
        stack.extend(children.iter_mut().zip(chains));
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    //! The old pipeline's tests (`crate::partials`), on `next`, and upstream's own partial tag
    //! with what the loader gives it.
    use super::*;
    use crate::next::index::{parse, transform, validate};
    use crate::next::parser::ParserArgs;
    use crate::next::renderers::html;
    use crate::next::schema::Config;
    use crate::next::tokenizer::TokenizerOptions;

    fn doc(src: &str) -> Node {
        parse(src, &ParserArgs::default()).unwrap()
    }

    fn load(src: &str, resolver: &InMemoryPartialResolver) -> Result<IndexMap<String, Partial>> {
        load_partials(&doc(src), resolver, &ParseOptions::default())
    }

    fn expand(src: &str, resolver: &InMemoryPartialResolver) -> Result<Node> {
        let partials = load(src, resolver)?;
        expand_partials(&doc(src), &partials)
    }

    fn texts(node: &Node) -> Vec<String> {
        node.walk()
            .filter(|n| n.node_type == NodeType::Text)
            .filter_map(|n| match n.attributes.get("content") {
                Some(Scalar::String(s)) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    fn has_text(node: &Node, needle: &str) -> bool {
        texts(node).iter().any(|t| t.contains(needle))
    }

    fn count_text(node: &Node, needle: &str) -> usize {
        texts(node).iter().filter(|t| t.contains(needle)).count()
    }

    fn has_partial(node: &Node) -> bool {
        node.walk().any(is_partial)
    }

    fn render(src: &str, partials: IndexMap<String, Partial>) -> String {
        let config = Config {
            partials: Some(partials),
            ..Config::default()
        };
        html::render(&transform(&doc(src), &config).unwrap()).unwrap()
    }

    fn message(result: Result<impl std::fmt::Debug>) -> String {
        match result.unwrap_err() {
            MarkdocError::TransformError(m) => m,
            other => panic!("expected a TransformError, got {other:?}"),
        }
    }

    #[test]
    fn inlines_simple_partial() {
        let resolver = InMemoryPartialResolver::new().with("b.markdoc", "Hello from partial B");
        let src = "Before\n\n{% partial file=\"b.markdoc\" /%}\n\nAfter";
        let expanded = expand(src, &resolver).unwrap();
        assert!(has_text(&expanded, "Hello from partial B"));
        assert!(has_text(&expanded, "Before"));
        assert!(has_text(&expanded, "After"));
        assert!(!has_partial(&expanded));
        // Upstream's tag renders the loaded partial in the same place.
        assert_eq!(
            render(src, load(src, &resolver).unwrap()),
            "<article><p>Before</p><p>Hello from partial B</p><p>After</p></article>"
        );
    }

    #[test]
    fn detects_cycle_self_reference() {
        let resolver = InMemoryPartialResolver::new()
            .with("a.markdoc", "Recursive: {% partial file=\"a.markdoc\" /%}");
        let msg = message(load("{% partial file=\"a.markdoc\" /%}", &resolver));
        assert_eq!(
            msg,
            "Partial inclusion cycle detected: a.markdoc -> a.markdoc"
        );
    }

    #[test]
    fn detects_cycle_indirect() {
        let resolver = InMemoryPartialResolver::new()
            .with("a.markdoc", "A: {% partial file=\"b.markdoc\" /%}")
            .with("b.markdoc", "B: {% partial file=\"a.markdoc\" /%}");
        let msg = message(load("{% partial file=\"a.markdoc\" /%}", &resolver));
        assert_eq!(
            msg,
            "Partial inclusion cycle detected: a.markdoc -> b.markdoc -> a.markdoc"
        );
    }

    #[test]
    fn a_cycle_given_to_expand_is_an_error_too() {
        // A map made by hand can hold a cycle `load_partials` would have refused.
        let mut partials = IndexMap::new();
        partials.insert(
            "a".to_string(),
            Partial::Node(Arc::new(doc("x {% partial file=\"b\" /%}"))),
        );
        partials.insert(
            "b".to_string(),
            Partial::Node(Arc::new(doc("{% partial file=\"a\" /%}"))),
        );
        let msg = message(expand_partials(
            &doc("{% partial file=\"a\" /%}"),
            &partials,
        ));
        assert_eq!(msg, "Partial inclusion cycle detected: a -> b -> a");
    }

    #[test]
    fn missing_partial_errors_with_path() {
        let msg = message(load(
            "{% partial file=\"missing.markdoc\" /%}",
            &InMemoryPartialResolver::new(),
        ));
        assert!(msg.contains("missing.markdoc"), "{msg}");
        // Without the loader, upstream renders nothing and only `validate` reports it.
        let src = "{% partial file=\"missing.markdoc\" /%}";
        assert_eq!(render(src, IndexMap::new()), "<article></article>");
        let errors = validate(&doc(src), &Config::default()).unwrap();
        assert_eq!(errors[0].error.id, "attribute-value-invalid");
    }

    #[test]
    fn missing_file_attr_errors() {
        let resolver = InMemoryPartialResolver::new();
        assert!(message(load("{% partial /%}", &resolver)).contains("file"));
        assert!(message(load("{% partial file=$f /%}", &resolver)).contains("file"));
        assert!(
            message(expand_partials(&doc("{% partial /%}"), &IndexMap::new())).contains("file")
        );
    }

    #[test]
    fn nested_non_cyclic_chain_works() {
        let resolver = InMemoryPartialResolver::new()
            .with("a.markdoc", "A: {% partial file=\"b.markdoc\" /%}")
            .with("b.markdoc", "B: {% partial file=\"c.markdoc\" /%}")
            .with("c.markdoc", "C-content");
        let src = "Top: {% partial file=\"a.markdoc\" /%}";
        let partials = load(src, &resolver).unwrap();
        assert_eq!(
            partials.keys().collect::<Vec<_>>(),
            ["c.markdoc", "b.markdoc", "a.markdoc"]
        );
        let expanded = expand_partials(&doc(src), &partials).unwrap();
        for text in ["Top:", "A:", "B:", "C-content"] {
            assert!(has_text(&expanded, text), "{text}");
        }
        assert!(!has_partial(&expanded));
        assert_eq!(
            render(src, partials),
            "<article><p>Top: <p>A: <p>B: <p>C-content</p></p></p></p></article>"
        );
    }

    #[test]
    fn same_partial_twice_is_loaded_once_and_spliced_twice() {
        let resolver = InMemoryPartialResolver::new().with("b.markdoc", "Repeated content");
        let src =
            "{% partial file=\"b.markdoc\" /%}\n\nMiddle\n\n{% partial file=\"b.markdoc\" /%}";
        assert_eq!(load(src, &resolver).unwrap().len(), 1);
        assert_eq!(
            count_text(&expand(src, &resolver).unwrap(), "Repeated content"),
            2
        );
    }

    #[test]
    fn a_map_kept_across_documents_loads_each_partial_once() {
        /// Counts the loads of each file.
        struct Counting(InMemoryPartialResolver, std::sync::Mutex<Vec<PathBuf>>);
        impl PartialResolver for Counting {
            fn load(&self, path: &Path) -> Result<String> {
                self.1.lock().unwrap().push(path.to_path_buf());
                self.0.load(path)
            }
        }
        let resolver = Counting(
            InMemoryPartialResolver::new()
                .with("a.md", "A {% partial file=\"shared.md\" /%}")
                .with("shared.md", "Shared")
                .with("b.md", "B"),
            Default::default(),
        );
        let options = ParseOptions::default();
        let mut cache = IndexMap::new();
        load_partials_into(
            &doc("{% partial file=\"a.md\" /%}"),
            &resolver,
            &options,
            &mut cache,
        )
        .unwrap();
        load_partials_into(
            &doc("{% partial file=\"shared.md\" /%}\n\n{% partial file=\"b.md\" /%}"),
            &resolver,
            &options,
            &mut cache,
        )
        .unwrap();
        assert_eq!(
            cache.keys().collect::<Vec<_>>(),
            ["shared.md", "a.md", "b.md"]
        );
        let loads = resolver.1.lock().unwrap().clone();
        assert_eq!(
            loads,
            [PathBuf::from("a.md"), "shared.md".into(), "b.md".into()]
        );
        // A missing file is an `Err`, and what was loaded before stays.
        let missing = load_partials_into(
            &doc("{% partial file=\"gone.md\" /%}"),
            &resolver,
            &options,
            &mut cache,
        );
        assert!(missing.is_err());
        assert_eq!(cache.len(), 3);
    }

    #[test]
    fn partial_frontmatter_is_dropped() {
        let resolver = InMemoryPartialResolver::new().with(
            "with_fm.markdoc",
            "---\ntitle: Inner Title\n---\n\nBody from inner.",
        );
        let src = "---\ntitle: Outer Title\n---\n\n{% partial file=\"with_fm.markdoc\" /%}";
        let expanded = expand(src, &resolver).unwrap();
        assert_eq!(
            expanded.attributes.get("frontmatter"),
            Some(&Scalar::String("title: Outer Title".into()))
        );
        assert!(has_text(&expanded, "Body from inner."));
        assert!(
            expanded
                .children
                .iter()
                .all(|c| c.node_type != NodeType::Document)
        );
    }

    #[test]
    fn partials_are_parsed_as_the_document_is_with_their_file_set() {
        // The old loader parsed every partial with the defaults and no file.
        let options = ParseOptions {
            tokenizer: TokenizerOptions {
                allow_indentation: true,
                allow_comments: true,
                ..TokenizerOptions::default()
            },
            strip_bom: true,
            ..ParseOptions::default()
        };
        let resolver = InMemoryPartialResolver::new().with(
            "p.md",
            "\u{FEFF}# Title <!-- hidden -->\n\n    - indented item\n",
        );
        let document = crate::ext::parse("{% partial file=\"p.md\" /%}", &options).unwrap();
        let partials = load_partials(&document, &resolver, &options).unwrap();
        let Some(Partial::Node(part)) = partials.get("p.md") else {
            panic!("a parsed partial")
        };
        let types: Vec<&str> = part.children.iter().map(|c| c.node_type.as_str()).collect();
        assert_eq!(types, ["heading", "list"]);
        assert!(part.walk().any(|n| n.node_type == NodeType::Comment));
        assert_eq!(
            part.children[0].location.as_ref().unwrap().file.as_deref(),
            Some("p.md")
        );
    }

    #[test]
    fn partials_in_slots_fences_and_lists_of_documents_are_expanded() {
        let options = ParseOptions {
            args: ParserArgs {
                slots: true,
                ..ParserArgs::default()
            },
            ..ParseOptions::default()
        };
        let resolver = InMemoryPartialResolver::new().with("s.md", "slotted");
        let src = "{% box %}\n{% slot \"title\" %}\n{% partial file=\"s.md\" /%}\n{% /slot %}\n{% /box %}\n";
        let document = crate::ext::parse(src, &options).unwrap();
        let partials = load_partials(&document, &resolver, &options).unwrap();
        let expanded = expand_partials(&document, &partials).unwrap();
        assert!(has_text(&expanded, "slotted"));
        assert!(!has_partial(&expanded));

        let mut partials = IndexMap::new();
        partials.insert(
            "two".to_string(),
            Partial::Nodes(Arc::new(vec![doc("one"), doc("two")])),
        );
        let expanded = expand_partials(&doc("{% partial file=\"two\" /%}"), &partials).unwrap();
        assert_eq!(texts(&expanded), ["one", "two"]);
    }

    #[test]
    fn variables_reach_the_partial_through_upstreams_tag_only() {
        let resolver = InMemoryPartialResolver::new().with("v.md", "Hi {% $who %}");
        let src = "{% partial file=\"v.md\" variables={who: \"there\"} /%}";
        let partials = load(src, &resolver).unwrap();
        assert_eq!(
            render(src, partials.clone()),
            "<article><p>Hi there</p></article>"
        );
        // The splice keeps the variable, which nothing binds any more.
        let expanded = expand_partials(&doc(src), &partials).unwrap();
        let config = Config::default();
        assert_eq!(
            html::render(&transform(&expanded, &config).unwrap()).unwrap(),
            "<article><p>Hi </p></article>"
        );
    }

    #[test]
    fn a_deep_chain_of_partials_loads_without_recursion() {
        let mut resolver = InMemoryPartialResolver::new();
        for i in 0..3000 {
            resolver = resolver.with(
                format!("p{i}"),
                format!("{{% partial file=\"p{}\" /%}}", i + 1),
            );
        }
        resolver = resolver.with("p3000", "end");
        let handle = std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(move || {
                let partials = load("{% partial file=\"p0\" /%}", &resolver).unwrap();
                let expanded =
                    expand_partials(&doc("{% partial file=\"p0\" /%}"), &partials).unwrap();
                (partials.len(), texts(&expanded))
            })
            .unwrap();
        assert_eq!(handle.join().unwrap(), (3001, vec!["end".to_string()]));
    }
}
