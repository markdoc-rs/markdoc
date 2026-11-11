//! The layers together: none of them is on until it is called, and called together on one
//! document (as a renderer and a linter of one corpus call them) they agree.

use super::bundle::Bundle;
use super::partials::{InMemoryPartialResolver, expand_partials, load_partials};
use super::variables::{bind, frontmatter_variables};
use super::{
    OccurrenceKind, ParseOptions, control_flow, crossrefs, frontmatter, functions, occurrences,
    parse, parse_with_occurrences,
};
use crate::next::index;
use crate::next::parser::ParserArgs;
use crate::next::renderers::html;
use crate::next::schema::Config;
use crate::next::schema::value::Value;
use crate::types::{NodeType, Scalar};

/// A document that uses something of every layer: a byte-order mark and YAML frontmatter
/// (E1, E9), a comment and an indented block (E1's tokenizer options), crate tags and an alias
/// (E2), `include()` (E7), `$markdoc.frontmatter` (E8), `for` and `switch` (E3), a partial
/// (E4), `tag`/`tagref` (E5) and tags for E6 to find.
const DOC: &str = "\u{feff}---\ntitle: Hello\nproducts: [Bot250, Bot600]\n---\n\
# {% $markdoc.frontmatter.title %} {% tag \"intro\" /%}\n\n\
<!-- vale off -->\n\n\
{% notice type=\"warning\" %}\n\
{% if include($markdoc.frontmatter.products, \"Bot250\") %}\nBot250 \"inside\"\n{% /if %}\n\
{% /notice %}\n\n\
{% for $markdoc.frontmatter.products as=\"p\" %}\n- {% $p %}\n{% /for %}\n\n\
{% switch $count %}\n{% case 2 %}\ntwo\n{% /case %}\n{% case %}\nother\n{% /case %}\n{% /switch %}\n\n\
{% partial file=\"part.md\" variables={who: \"me\"} /%}\n\n\
See {% tagref \"intro\" /%} and {% tagref \"gone\" /%}.\n\n\
    {% callout %}\n    indented\n    {% /callout %}\n";

/// What upstream's dist gives for [`DOC`] with `new Tokenizer()` and config `{}`
/// (`@markdoc/markdoc` 0.5.11 under node 22.22.1, through `tests/next/oracle.cjs`):
/// `renderers.html(transform(ast))`.
const UPSTREAM_HTML: &str = "<article><h1> </h1><p>&lt;!-- vale off --&gt;</p><ul><li></li></ul>\
<p>two</p><p>other</p><p>See  and .</p><p>indented </p></article>";

/// The same run's `validate(ast)` ids, in order.
const UPSTREAM_ERRORS: [&str; 13] = [
    "tag-undefined",
    "tag-undefined",
    "tag-undefined",
    "tag-undefined",
    "tag-undefined",
    "tag-undefined",
    "attribute-value-invalid",
    "tag-undefined",
    "tag-undefined",
    "missing-closing",
    "tag-undefined",
    "missing-opening",
    "tag-undefined",
];

/// [`DOC`] through the core with its defaults: the HTML, the error ids and the AST's JSON.
fn upstream_run() -> (String, Vec<String>, serde_json::Value) {
    let doc = index::parse(DOC, &ParserArgs::default()).unwrap();
    let config = Config::default();
    let ids = index::validate(&doc, &config)
        .unwrap()
        .into_iter()
        .map(|e| e.error.id)
        .collect();
    let out = html::render(&index::transform(&doc, &config).unwrap()).unwrap();
    (out, ids, serde_json::to_value(&doc).unwrap())
}

/// The corpus-style overlay: D3's parse options and D15's typographer for renderers.
const OVERLAY: &str = "[parse]\nallow_indentation = true\nallow_comments = true\n\
strip_bom = true\nfrontmatter = \"yaml\"\n\n[render]\ntypographer = true\n";

const PART: &str = "\u{feff}Hello from {% $who %}, {% $markdoc.frontmatter.title %}\n";

/// Every layer called on [`DOC`], as a renderer (and, for E6, a linter) of one corpus would
/// call them; returns the HTML and the validation ids.
fn all_layers() -> (String, Vec<String>) {
    // E2: the bundle and the overlay; E1: the options they carry.
    let mut bundle = Bundle::builtin();
    bundle.overlay(OVERLAY).unwrap();
    let options = bundle.render_options();
    let doc = parse(DOC, &options).unwrap();
    assert_eq!(doc.children[0].node_type, NodeType::Heading, "BOM stripped");

    // E9 reads the frontmatter E1 parsed.
    let split = frontmatter::split(DOC).unwrap();
    assert_eq!(split.yaml, "title: Hello\nproducts: [Bot250, Bot600]");
    assert_eq!(
        doc.attributes["frontmatter"],
        Scalar::Object(frontmatter::parse_yaml(split.yaml).unwrap())
    );

    // The config: E2's tags, E3's tags from the bundle, E7, E8, E4.
    let mut config = bundle.config();
    control_flow::apply_bundle(&bundle, &mut config);
    config.functions.extend(functions::all());
    frontmatter_variables(&doc).apply(&mut config);
    bind(&mut config, "count", Value::Number(2.0));
    let resolver = InMemoryPartialResolver::new().with("part.md", PART);
    let partials = load_partials(&doc, &resolver, &options).unwrap();
    config.partials = Some(partials.clone());

    let ids: Vec<String> = index::validate(&doc, &config)
        .unwrap()
        .into_iter()
        .map(|e| e.error.id)
        .collect();
    let out = html::render(&index::transform(&doc, &config).unwrap()).unwrap();

    // E3's AST pass decides as the transform-time tags do.
    let evaluated = control_flow::evaluate_conditionals(&doc, &config).unwrap();
    let again = html::render(&index::transform(&evaluated, &config).unwrap()).unwrap();
    assert_eq!(again, out);

    // E4's splice and E5 over the expanded tree.
    let expanded = expand_partials(&doc, &partials).unwrap();
    assert!(expanded.walk().all(|n| n.tag.as_deref() != Some("partial")));
    let checked = crossrefs::resolve_crossrefs(&expanded);
    let broken: Vec<&str> = checked
        .walk()
        .flat_map(|n| n.errors.iter().map(|e| e.id.as_str()))
        .filter(|id| id.starts_with("tagref/"))
        .collect();
    assert_eq!(broken, ["tagref/unknown-target"]);
    assert!(crossrefs::collect_anchors(&expanded).contains_key("intro"));

    // E6 with the linter's options (no typographer): one tokenizing gives the AST
    // `ext::parse` gives, and the tags.
    let lint = bundle.parse_options();
    let (ast, found) = parse_with_occurrences(DOC, &lint).unwrap();
    assert_eq!(
        serde_json::to_value(&ast).unwrap(),
        serde_json::to_value(parse(DOC, &lint).unwrap()).unwrap()
    );
    let alone = occurrences(DOC, &lint).unwrap();
    assert_eq!(alone.len(), found.len());
    let opens = found
        .iter()
        .filter(|o| matches!(o.kind, OccurrenceKind::Open { .. }))
        .count();
    let closes = found
        .iter()
        .filter(|o| matches!(o.kind, OccurrenceKind::Close { .. }))
        .count();
    // notice, if, for, switch, case x2, callout: each open has its close.
    assert_eq!((opens, closes), (7, 7));
    for (i, o) in found.iter().enumerate() {
        if let Some(p) = o.partner {
            assert_eq!(found[p].partner, Some(i));
        }
        assert!(DOC[o.span.clone()].starts_with("{%"));
    }

    (out, ids)
}

#[test]
fn every_layer_is_off_until_called() {
    let before = upstream_run();
    assert_eq!(before.0, UPSTREAM_HTML);
    assert_eq!(before.1, UPSTREAM_ERRORS);
    // E1's defaults are upstream's parse.
    assert_eq!(
        serde_json::to_value(parse(DOC, &ParseOptions::default()).unwrap()).unwrap(),
        before.2
    );

    // Calling every layer leaves no trace in the core's defaults.
    all_layers();
    assert_eq!(upstream_run(), before);
    let merged = index::merge_config(&Config::default());
    let mut tags: Vec<&str> = merged.tags.keys().map(String::as_str).collect();
    tags.sort_unstable();
    assert_eq!(tags, ["else", "if", "partial", "slot", "table"]);
    let mut names: Vec<&str> = merged.functions.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(names, ["and", "debug", "default", "equals", "not", "or"]);
    assert!(merged.variables.is_none() && merged.partials.is_none());
    let table = merged.nodes["table"].as_ref().unwrap();
    assert!(
        table
            .attributes
            .as_ref()
            .is_none_or(|a| !a.contains_key("borders")),
        "the bundle's [nodes.table] additions stay in the bundle's config"
    );
}

#[test]
fn the_layers_fit_together() {
    let (out, ids) = all_layers();
    assert_eq!(
        out,
        "<article><h1>Hello <tag primary=\"intro\"></tag></h1>\
         <callout type=\"warning\"><p>Bot250 \u{201c}inside\u{201d}</p></callout>\
         <ul><li>Bot250</li><li>Bot600</li></ul><p>two</p>\
         <p>Hello from me, Hello</p>\
         <p>See <tagref primary=\"intro\"></tagref> and <tagref primary=\"gone\"></tagref>.</p>\
         <callout type=\"note\"><p>indented</p></callout></article>"
    );
    // The loop variable is unknown to upstream's validator once `config.variables` is set
    // (D10: accepted for now).
    assert_eq!(ids, ["variable-undefined"]);
}
