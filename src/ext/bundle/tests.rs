//! Tests of the schema bundle: the conversion rules, the built-ins it must not replace (R4),
//! one sample of every bundled tag through parse, validate, transform and HTML (R5), the old
//! loader's answers for the same data, and nothing changing without a bundle.

use super::*;
use crate::ext::{ParseOptions, parse};
use crate::next::index;
use crate::next::parser::ParserArgs;
use crate::next::renderers::html;
use crate::next::schema::Partial;

/// The HTML of `source` under `config`, parsed with `options`.
fn html_with(source: &str, config: &Config, options: &ParseOptions) -> String {
    let doc = parse(source, options).unwrap();
    html::render(&index::transform(&doc, config).unwrap()).unwrap()
}

fn html_of(source: &str, bundle: &Bundle) -> String {
    html_with(source, &bundle.config(), &bundle.parse_options())
}

/// `validate()`'s errors, as `id: message`.
fn errors_of(source: &str, bundle: &Bundle) -> Vec<String> {
    let doc = parse(source, &bundle.parse_options()).unwrap();
    index::validate(&doc, &bundle.config())
        .unwrap()
        .into_iter()
        .map(|e| format!("{}: {}", e.error.id, e.error.message))
        .collect()
}

fn load_err(text: &str) -> String {
    match Bundle::from_toml(text) {
        Ok(_) => panic!("loaded: {text}"),
        Err(e) => e.to_string(),
    }
}

// ---------------------------------------------------------------------------------------
// Without a bundle nothing changes.

#[test]
fn without_a_bundle_the_output_is_upstreams() {
    // Upstream's empty config unwraps an undeclared tag; an empty bundle is that config.
    let source = "{% callout type=\"note\" %}\nText {% $x %}.\n{% /callout %}\n";
    let upstream = html::render(
        &index::transform(
            &index::parse(source, &ParserArgs::default()).unwrap(),
            &Config::default(),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(upstream, "<article><p>Text .</p></article>");
    let empty = Bundle::new();
    assert_eq!(html_of(source, &empty), upstream);
    assert!(empty.config().tags.is_empty() && empty.config().nodes.is_empty());
    assert_eq!(empty.parse_options(), ParseOptions::default());
}

#[test]
fn default_parse_options_are_upstreams_parse() {
    // E1 switched off: the same AST as `Markdoc.parse(content)`, for text each switch would
    // change (a BOM, frontmatter, comments, indentation, typographer quotes).
    for source in [
        "\u{FEFF}# Title\n",
        "---\ntitle: x\n---\n# Doc\n",
        "<!-- note -->\nText \"quoted\" -- (c)\n",
        "    {% callout %}\n    indented\n    {% /callout %}\n",
        "{% if $a %}\nA\n{% /if %}\n",
    ] {
        let ours = parse(source, &ParseOptions::default()).unwrap();
        let upstream = index::parse(source, &ParserArgs::default()).unwrap();
        assert_eq!(
            serde_json::to_value(&ours).unwrap(),
            serde_json::to_value(&upstream).unwrap(),
            "{source:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// The conversion rules.

#[test]
fn a_tag_renders_under_its_own_name_and_an_alias_as_its_canonical_tag() {
    let bundle = Bundle::from_toml(
        "[tags.box]\n[tags.crate]\nalias_of = \"box\"\n\
         [tags.quiet]\nrender = false\n[tags.named]\nrender = \"section\"\n",
    )
    .unwrap();
    let config = bundle.config();
    // (A schema's `render` is a value: its name as a string.)
    let render = |name: &str| {
        config.tags[name]
            .as_ref()
            .unwrap()
            .render
            .as_ref()
            .map(|r| r.as_str().map(str::to_string))
    };
    assert_eq!(render("box"), Some(Some("box".to_string())));
    assert_eq!(render("crate"), Some(Some("box".to_string())));
    assert_eq!(render("quiet"), None);
    assert_eq!(render("named"), Some(Some("section".to_string())));
    assert_eq!(
        html_of(
            "{% crate %}\nA\n{% /crate %}\n\n{% quiet %}\nB\n{% /quiet %}\n",
            &bundle
        ),
        "<article><box><p>A</p></box><p>B</p></article>"
    );
    assert_eq!(bundle.canonical_name("crate"), "box");
    assert_eq!(bundle.canonical_name("box"), "box");
}

#[test]
fn inline_is_honoured_when_given_and_open_when_not() {
    let bundle = Bundle::from_toml(
        "[tags.blk]\ninline = false\n[tags.inl]\ninline = true\n[tags.any]\nself_closing = true\n",
    )
    .unwrap();
    let config = bundle.config();
    let inline = |name: &str| match config.tags[name].as_ref().unwrap().inline {
        Some(Value::Boolean(b)) => Some(b),
        Some(_) => panic!("inline is a boolean here"),
        None => None,
    };
    assert_eq!(
        (inline("blk"), inline("inl"), inline("any")),
        (Some(false), Some(true), None)
    );
    assert!(errors_of("Text {% any /%} here.\n\n{% any /%}\n", &bundle).is_empty());
    assert_eq!(
        errors_of("Text {% blk %}x{% /blk %} here.\n", &bundle),
        ["tag-placement-invalid: 'blk' tag should be block"]
    );
}

#[test]
fn types_defaults_and_attribute_renders_become_values() {
    let bundle = Bundle::from_toml(
        r#"
[tags.x.attributes.a]
type = ["String", "Number"]
default = "dflt"
[tags.x.attributes.b]
type = ["Boolean"]
render = "data-b"
[tags.x.attributes.c]
render = false
required = true
"#,
    )
    .unwrap();
    let schema = bundle.schema("x").unwrap();
    let attrs = schema.attributes.as_ref().unwrap();
    assert!(matches!(&attrs["a"].attr_type, Some(TypeSpec::Union(t)) if t.len() == 2));
    assert!(matches!(
        &attrs["b"].attr_type,
        Some(TypeSpec::Builtin(BuiltinType::Boolean))
    ));
    assert!(attrs["c"].required && attrs["c"].attr_type.is_none());
    assert_eq!(
        html_of("{% x b=true c=1 /%}\n", &bundle),
        "<article><x a=\"dflt\" data-b=\"true\"></x></article>"
    );
    assert_eq!(
        errors_of("{% x a=true /%}\n", &bundle),
        [
            "attribute-type-invalid: Attribute 'a' must be type of 'String | Number'",
            "attribute-missing-required: Missing required attribute: 'c'",
        ]
    );
}

#[test]
fn one_of_and_pattern_alone_are_upstreams_matches() {
    let bundle = Bundle::from_toml(
        "[tags.x.attributes.k]\none_of = [\"a\", \"b\"]\n\
         [tags.x.attributes.p]\npattern = '^\\d+px$'\nerror_level = \"warning\"\n",
    )
    .unwrap();
    assert!(errors_of("{% x k=\"a\" p=\"12px\" /%}\n", &bundle).is_empty());
    assert_eq!(
        errors_of("{% x k=\"c\" p=\"12\" /%}\n", &bundle),
        [
            "attribute-value-invalid: Attribute 'k' must match one of [\"a\",\"b\"]. Got 'c' instead.",
            "attribute-value-invalid: Attribute 'p' must match /^\\d+px$/. Got '12' instead.",
        ]
    );
}

#[test]
fn one_of_with_pattern_accepts_either() {
    // media `size`: a preset or a length (the old validator's OR).
    let bundle = Bundle::builtin();
    for ok in ["small", "icon", "50%", "120px", "1.5em"] {
        let source = format!("{{% media size=\"{ok}\" /%}}\n");
        assert!(errors_of(&source, &bundle).is_empty(), "{ok}");
    }
    assert_eq!(
        errors_of("{% image size=\"huge\" /%}\n", &bundle),
        [
            "attribute-value-invalid: Attribute 'size' must match one of \
          [\"small\",\"medium\",\"large\",\"table\",\"icon\"] or \
          /^\\d+(\\.\\d+)?(px|%|pt|em|rem)$/. Got 'huge' instead."
        ]
    );
}

#[test]
fn children_lists_tag_names() {
    let bundle =
        Bundle::from_toml("[tags.menu]\nchildren = [\"item\"]\n[tags.item]\n[tags.other]\n")
            .unwrap();
    assert!(
        errors_of(
            "{% menu %}\n{% item %}\nA\n{% /item %}\n{% /menu %}\n",
            &bundle
        )
        .is_empty()
    );
    assert_eq!(
        errors_of(
            "{% menu %}\n{% other %}\nA\n{% /other %}\n\nText\n{% /menu %}\n",
            &bundle
        ),
        [
            "child-invalid: Can't nest 'other' in 'menu'",
            "child-invalid: Can't nest 'paragraph' in 'menu'",
        ]
    );
}

#[test]
fn nodes_gain_attributes() {
    let bundle = Bundle::from_toml(
        "[nodes.fence.attributes.align]\none_of = [\"left\", \"center\"]\n\
         [nodes.fence.attributes.language]\ndescription = \"The info string\"\n\
         [nodes.image.attributes.size]\ntype = [\"String\"]\n",
    )
    .unwrap();
    let config = bundle.config();
    let fence = config.nodes["fence"].as_ref().unwrap();
    let attrs = fence.attributes.as_ref().unwrap();
    // Upstream's attributes stay, in their order, with the new one after them.
    let names: Vec<_> = attrs.keys().map(String::as_str).collect();
    assert_eq!(names.last(), Some(&"align"));
    assert!(names.contains(&"content") && names.contains(&"language"));
    assert_eq!(
        attrs["language"].description.as_deref(),
        Some("The info string")
    );
    assert_eq!(fence.render.as_ref().and_then(Value::as_str), Some("pre"));
    assert!(
        config.nodes["image"]
            .as_ref()
            .unwrap()
            .attributes
            .as_ref()
            .unwrap()["size"]
            .attr_type
            .is_some()
    );
    // A fence's info-string annotation reaches the fence node.
    let source = "```js {% align=\"center\" %}\nx\n```\n";
    assert!(
        errors_of(source, &bundle).is_empty(),
        "{:?}",
        errors_of(source, &bundle)
    );
    assert_eq!(
        html_of(source, &bundle),
        "<article><pre data-language=\"js\" align=\"center\">x\n</pre></article>"
    );
    // Without the bundle the attribute is undeclared.
    let doc = parse(source, &ParseOptions::default()).unwrap();
    let ids: Vec<_> = index::validate(&doc, &Config::default())
        .unwrap()
        .into_iter()
        .map(|e| e.error.id)
        .collect();
    assert_eq!(ids, ["attribute-undefined"]);
    // An attribute upstream's node has takes only a description; an unknown node is an error.
    assert!(
        load_err("[nodes.fence.attributes.content]\ntype = [\"Number\"]\n")
            .contains("fence.content")
    );
    assert!(load_err("[nodes.widget.attributes.a]\n").contains("nodes.widget"));
}

#[test]
fn layer_tags_are_left_out_until_their_layer_adds_them() {
    let bundle = Bundle::builtin();
    let config = bundle.config();
    for name in ["switch", "case", "for"] {
        assert!(!config.tags.contains_key(name), "{name}");
        assert_eq!(bundle.layer(name), Some("control_flow"));
        // Still declared for editors.
        assert_eq!(bundle.engines()[name], "conditionals.rs");
    }
    let schemas = bundle.layer_schemas("control_flow");
    assert_eq!(
        schemas.keys().collect::<Vec<_>>(),
        ["switch", "case", "for"]
    );
    let switch = &schemas["switch"];
    assert!(switch.attributes.as_ref().unwrap()["primary"].required);
    assert!(switch.validate.is_some(), "children = [\"case\"]");
    let for_ = &schemas["for"];
    assert!(matches!(
        for_.attributes.as_ref().unwrap()["primary"].attr_type,
        Some(TypeSpec::Builtin(BuiltinType::Array))
    ));
}

#[test]
fn unknown_keys_and_bad_values_fail_the_load() {
    assert!(load_err("[tags.x]\nselfclosing = true\n").contains("selfclosing"));
    assert!(load_err("[tags.x.attributes.a]\ntype = [\"Striiing\"]\n").contains("x.a"));
    assert!(load_err("[tags.x.attributes.a]\npattern = \"(\"\n").contains("x.a: pattern"));
    assert!(load_err("[tags.x.attributes.a]\nerror_level = \"loud\"\n").contains("loud"));
    assert!(load_err("[tags.x]\nalias_of = \"nosuchtag\"\n").contains("nosuchtag"));
    assert!(
        load_err("[tags.a]\n[tags.b]\nalias_of = \"a\"\n[tags.c]\nalias_of = \"b\"\n")
            .contains("itself an alias")
    );
    assert!(
        load_err("[tags.a]\n[tags.b]\nalias_of = \"a\"\nself_closing = true\n").contains("alias")
    );
    assert!(load_err("[parse]\nallow_indentaton = true\n").contains("allow_indentaton"));
    assert!(load_err("[render]\ntypographer = \"yes\"\n").contains("typographer"));
    assert!(load_err("[tags.x]\nbuiltin = true\n").contains("no built-in tag"));
}

// ---------------------------------------------------------------------------------------
// Upstream's built-in tags are extended, never replaced (rule 3, R4).

#[test]
fn an_entry_cannot_replace_a_builtin() {
    for name in ["else", "if", "partial", "slot", "table"] {
        for field in [
            "render = \"x\"",
            "self_closing = true",
            "inline = true",
            "children = [\"x\"]",
            "alias_of = \"callout\"",
            "layer = \"x\"",
            "builtin = false",
        ] {
            let text = format!("[tags.callout]\n[tags.{name}]\n{field}\n");
            assert!(load_err(&text).contains("built-in"), "{name} with {field}");
        }
    }
    // An attribute the built-in has takes only a description.
    assert!(
        load_err("[tags.partial.attributes.file]\nrequired = false\n").contains("partial.file")
    );
    assert!(load_err("[tags.if.attributes.primary]\ntype = [\"String\"]\n").contains("if.primary"));
}

#[test]
fn if_and_else_keep_their_transform() {
    let bundle = Bundle::builtin();
    let config = bundle.config();
    let schema = config.tags["if"].as_ref().unwrap();
    assert!(schema.transform.is_some());
    assert_eq!(
        schema.description.as_deref(),
        Some("Conditional rendering tag")
    );
    assert_eq!(
        schema.attributes.as_ref().unwrap()["primary"]
            .description
            .as_deref(),
        Some("Predicate to evaluate")
    );
    let source = "{% if $on %}\nA\n{% else /%}\nB\n{% /if %}\n";
    assert_eq!(html_of(source, &bundle), "<article><p>B</p></article>");
    assert!(errors_of(source, &bundle).is_empty());
}

#[test]
fn partial_still_renders_its_file() {
    let bundle = Bundle::builtin();
    let mut config = bundle.config();
    let partial = parse(
        "Partial *text*.\n",
        &bundle.parse_options().with_file("p.md"),
    )
    .unwrap();
    config.partials = Some(IndexMap::from([(
        "p.md".to_string(),
        Partial::Node(Arc::new(partial)),
    )]));
    let schema = config.tags["partial"].as_ref().unwrap();
    assert!(schema.transform.is_some());
    assert!(
        schema
            .attributes
            .as_ref()
            .unwrap()
            .contains_key("variables")
    );
    let doc = parse("{% partial file=\"p.md\" /%}\n", &bundle.parse_options()).unwrap();
    assert!(index::validate(&doc, &config).unwrap().is_empty());
    assert_eq!(
        html::render(&index::transform(&doc, &config).unwrap()).unwrap(),
        "<article><p>Partial <em>text</em>.</p></article>"
    );
}

#[test]
fn table_gains_its_styling_attributes_and_nests_no_table() {
    let bundle = Bundle::builtin();
    let config = bundle.config();
    let tag = config.tags["table"].as_ref().unwrap();
    // Upstream's: no render (the tag gives way to the table node it holds), its children.
    assert!(tag.render.is_none());
    assert_eq!(
        tag.children.as_ref().map(|c| c.to_scalar().unwrap()),
        Some(Scalar::Array(vec![Scalar::String("table".into())]))
    );
    assert!(matches!(tag.inline, Some(Value::Boolean(false))));
    assert_eq!(tag.attributes.as_ref().unwrap().len(), 9);
    let node = config.nodes["table"].as_ref().unwrap();
    assert_eq!(node.render.as_ref().and_then(Value::as_str), Some("table"));
    assert_eq!(node.attributes.as_ref().unwrap().len(), 9);
    let source = "{% table borders=\"grid\" column_weights=\"1 2\" %}\n* H1\n* H2\n---\n* a\n* b\n{% /table %}\n";
    assert!(
        errors_of(source, &bundle).is_empty(),
        "{:?}",
        errors_of(source, &bundle)
    );
    let html = html_of(source, &bundle);
    assert_eq!(html.matches("<table").count(), 1, "{html}");
    assert!(
        html.starts_with("<article><table borders=\"grid\" column_weights=\"1 2\">"),
        "{html}"
    );
}

#[test]
fn slot_is_upstreams() {
    let bundle = Bundle::builtin();
    assert!(!bundle.config().tags.contains_key("slot"));
    assert!(bundle.schema("slot").is_none());
}

// ---------------------------------------------------------------------------------------
// Every bundled tag through the full layer (R5).

/// A value of an attribute's type, as the tag grammar writes it.
fn sample_value(attribute: &SchemaAttribute) -> &'static str {
    match &attribute.attr_type {
        Some(TypeSpec::Builtin(BuiltinType::Number)) => "1",
        Some(TypeSpec::Builtin(BuiltinType::Boolean)) => "true",
        _ => "\"v\"",
    }
}

/// A document that uses tag `name` as its schema allows, with its required attributes and a
/// primary value where it takes one.
fn sample(name: &str, schema: &Schema) -> String {
    let mut attributes = String::new();
    for (key, attribute) in schema.attributes.iter().flatten() {
        if key == "primary" {
            attributes.insert_str(0, " \"anchor\"");
        } else if attribute.required {
            attributes.push_str(&format!(" {key}={}", sample_value(attribute)));
        }
    }
    let tag = if schema.self_closing {
        format!("{{% {name}{attributes} /%}}")
    } else {
        format!("{{% {name}{attributes} %}}body{{% /{name} %}}")
    };
    match schema.inline {
        Some(Value::Boolean(true)) => format!("Before {tag} after.\n"),
        _ if schema.self_closing => format!("{tag}\n"),
        _ => format!("{{% {name}{attributes} %}}\nBody.\n{{% /{name} %}}\n"),
    }
}

#[test]
fn every_bundled_tag_renders_through_the_layer() {
    let bundle = Bundle::builtin();
    let config = bundle.config();
    let options = bundle.render_options();
    let mut seen = 0;
    for name in bundle.tag_names() {
        if bundle.layer(name).is_some() || is_builtin(name) {
            continue; // the layers' own tests, and the built-in tests above
        }
        seen += 1;
        let schema = config.tags[name].as_ref().unwrap();
        let source = sample(name, schema);
        let doc = parse(&source, &options).unwrap();
        let errors: Vec<_> = index::validate(&doc, &config)
            .unwrap()
            .into_iter()
            .map(|e| e.error.message)
            .collect();
        assert!(errors.is_empty(), "{source:?}: {errors:?}");
        let html = html::render(&index::transform(&doc, &config).unwrap()).unwrap();
        let element = schema
            .render
            .as_ref()
            .and_then(Value::as_str)
            .expect("every bundled tag renders");
        assert_eq!(element, bundle.canonical_name(name), "{name}");
        assert!(
            html.contains(&format!("<{element}")),
            "{source:?} -> {html}"
        );
        if schema
            .attributes
            .iter()
            .flatten()
            .any(|(k, _)| k == "primary")
        {
            assert!(html.contains("primary=\"anchor\""), "{source:?} -> {html}");
        }
    }
    // The 26 entries of tags.toml: 15 tags and 4 aliases here, 4 built-ins (else, if,
    // partial, table) and the 3 control-flow tags elsewhere.
    assert_eq!(seen, 19);
    assert_eq!(bundle.tag_names().count(), 26);
}

#[test]
fn anchors_keep_their_ids() {
    // tag and tagref take a primary (§4.1), as 2,518 + 2,788 corpus uses write them, as a
    // block or inline.
    let bundle = Bundle::builtin();
    let source = "# Docking {% tag \"docking\" /%}\n\nSee {% tagref \"docking\" /%}.\n\n{% tagref \"docking\" /%}\n";
    assert!(
        errors_of(source, &bundle).is_empty(),
        "{:?}",
        errors_of(source, &bundle)
    );
    assert_eq!(
        html_of(source, &bundle),
        "<article><h1>Docking <tag primary=\"docking\"></tag></h1>\
         <p>See <tagref primary=\"docking\"></tagref>.</p>\
         <tagref primary=\"docking\"></tagref></article>"
    );
}

// ---------------------------------------------------------------------------------------
// What the old loader (removed at the switch-over) answered for the same tags.

#[test]
fn the_bundle_keeps_the_old_aliases_and_engines() {
    let mut new_aliases = aliases().clone();
    new_aliases.sort_keys();
    let old_aliases: IndexMap<String, String> = [
        ("c", "color"),
        ("image", "media"),
        ("img", "media"),
        ("notice", "callout"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    assert_eq!(new_aliases, old_aliases);
    let mut new_engines = engines().clone();
    new_engines.sort_keys();
    let old_engines: IndexMap<String, String> = [
        ("case", "conditionals.rs"),
        ("else", "conditionals.rs"),
        ("for", "conditionals.rs"),
        ("if", "conditionals.rs"),
        ("partial", "partials.rs"),
        ("switch", "conditionals.rs"),
        ("tag", "crossrefs.rs"),
        ("tagref", "crossrefs.rs"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    assert_eq!(new_engines, old_engines);
    assert_eq!(canonical_name("notice"), "callout");
    assert_eq!(canonical_name("image"), "media");
    assert_eq!(canonical_name("callout"), "callout");
}

#[test]
fn a_rendered_callout_matches_the_old_pipeline() {
    // The old pipeline's own render of a bundled tag with a default was this, apart from the
    // root element (a `div` there, upstream's `article` here).
    let source = "{% notice %}\nText.\n{% /notice %}\n";
    assert_eq!(
        html_of(source, &Bundle::builtin()),
        "<article><callout type=\"note\"><p>Text.</p></callout></article>"
    );
}

// ---------------------------------------------------------------------------------------
// `[validation]` and `[functions.*]` (MIGRATION-MAP R3), in the shape of a corpus's
// `markdoc.toml`.

/// A corpus file's `[validation]` and `[functions.*]` tables.
const FUNCTIONS_TOML: &str = r#"
[validation]
validate_functions = true

[functions.higher]
description = "At or above the threshold"
[functions.higher.parameters.0]
type = ["String", "Number"]
required = true
description = "The version"
[functions.higher.parameters.1]
type = ["Array", "String"]
required = true

[functions.includes]
description = "True when the array contains the value"
[functions.includes.parameters.0]
type = ["Array"]
required = true
[functions.includes.parameters.1]
required = true
"#;

#[test]
fn validation_and_function_declarations_reach_the_config() {
    let mut bundle = Bundle::builtin();
    bundle.overlay(FUNCTIONS_TOML).unwrap();
    assert_eq!(
        bundle.function_names().collect::<Vec<_>>(),
        ["higher", "includes"]
    );
    let config = bundle.config();
    assert!(config.validation.validate_functions);
    let higher = config.functions["higher"].as_ref().unwrap();
    assert_eq!(
        higher.description.as_deref(),
        Some("At or above the threshold")
    );
    assert!(higher.transform.is_none() && higher.returns.is_none());
    let parameters = higher.parameters.as_ref().unwrap();
    assert_eq!(parameters.keys().collect::<Vec<_>>(), ["0", "1"]);
    assert!(parameters["0"].required && parameters["1"].required);
    assert_eq!(parameters["0"].description.as_deref(), Some("The version"));

    // Upstream's validator reads the declarations: a wrong argument, a missing one, and a
    // function nobody declared.
    assert_eq!(
        errors_of(
            "{% if higher(\"1.2\", 1.0) %}a{% /if %}\n\n\
             {% if includes($list) %}b{% /if %}\n\n\
             {% if lower(\"1.2\", \"1.0\") %}c{% /if %}\n",
            &bundle
        ),
        [
            "parameter-type-invalid: Parameter '1' of 'higher' must be type of 'Array | String'",
            "parameter-missing-required: Missing required parameter: '1'",
            "function-undefined: Undefined function: 'lower'",
        ]
    );
    // Without the tables, nothing of this is in the config.
    let plain = Bundle::builtin().config();
    assert!(!plain.validation.validate_functions);
    assert!(plain.functions.is_empty());
}

#[test]
fn a_declaration_merges_under_the_code_a_tool_registered() {
    let mut bundle = Bundle::builtin();
    bundle.overlay(FUNCTIONS_TOML).unwrap();
    // A second file states one field: the rest of the declaration stays (D24).
    bundle
        .overlay("[functions.includes.parameters.1]\ndescription = \"The value\"\n")
        .unwrap();
    let mut config = Config::default();
    config
        .functions
        .insert("includes".into(), Some(crate::ext::functions::includes()));
    bundle.apply(&mut config);
    let includes = config.functions["includes"].as_ref().unwrap();
    assert!(includes.transform.is_some(), "the tool's code is kept");
    assert_eq!(
        includes.description.as_deref(),
        Some("True when the array contains the value")
    );
    let parameters = includes.parameters.as_ref().unwrap();
    assert!(parameters["0"].required && parameters["1"].required);
    assert_eq!(parameters["1"].description.as_deref(), Some("The value"));
    // The function runs.
    let doc = parse(
        "{% if includes([\"a\", \"b\"], \"b\") %}yes{% /if %}\n",
        &bundle.parse_options(),
    )
    .unwrap();
    assert_eq!(
        html::render(&index::transform(&doc, &config).unwrap()).unwrap(),
        "<article><p>yes</p></article>"
    );
    // A declaration of one of upstream's functions keeps upstream's code.
    bundle
        .overlay("[functions.equals]\ndescription = \"===\"\n")
        .unwrap();
    let config = bundle.config();
    let equals = config.functions["equals"].as_ref().unwrap();
    assert!(equals.transform.is_some());
    assert_eq!(equals.description.as_deref(), Some("==="));
    // A function the config hides stays hidden.
    let mut hidden = Config::default();
    hidden.functions.insert("higher".into(), None);
    bundle.apply(&mut hidden);
    assert!(hidden.functions["higher"].is_none());
}

#[test]
fn declared_functions_without_code_are_named() {
    let mut bundle = Bundle::builtin();
    bundle.overlay(FUNCTIONS_TOML).unwrap();
    bundle
        .overlay("[functions.equals]\ndescription = \"===\"\n")
        .unwrap();
    let mut config = bundle.config();
    assert_eq!(
        bundle.functions_without_code(&config),
        ["higher", "includes"]
    );
    assert_eq!(
        bundle.check_functions(&config).unwrap_err().to_string(),
        "Parse error: schema: declared functions without code: higher, includes (register a \
         transform for each before starting)"
    );
    // Registered (before or after the bundle is applied), they run.
    config
        .functions
        .insert("includes".into(), Some(crate::ext::functions::includes()));
    let mut higher = crate::ext::functions::positional(|_| Ok(Scalar::Boolean(true)));
    higher.description = Some("registered".into());
    config.functions.insert("higher".into(), Some(higher));
    assert!(bundle.functions_without_code(&config).is_empty());
    bundle.check_functions(&config).unwrap();
    // Hidden counts as missing; a bundle without declarations has nothing to check.
    config.functions.insert("higher".into(), None);
    assert_eq!(bundle.functions_without_code(&config), ["higher"]);
    Bundle::builtin()
        .check_functions(&Config::default())
        .unwrap();
}

#[test]
fn bad_function_and_validation_tables_fail_the_load() {
    assert!(
        load_err("[functions.f.parameters.0]\ntype = [\"Text\"]\n")
            .contains("functions.f.0: unknown type \"Text\"")
    );
    assert!(
        load_err("[functions.f]\nreturns = [\"Text\"]\n")
            .contains("functions.f: unknown type \"Text\" in `returns`")
    );
    assert!(load_err("[functions.f]\ntransform = \"x\"\n").contains("unknown field"));
    assert!(load_err("[validation]\nvalidateFunctions = true\n").contains("unknown field"));
    // On `Err` the bundle is unchanged.
    let mut bundle = Bundle::builtin();
    assert!(
        bundle
            .overlay("[validation]\nvalidate_functions = true\n[functions.f]\nreturns = [1]\n")
            .is_err()
    );
    assert!(!bundle.config().validation.validate_functions);
    assert_eq!(bundle.function_names().count(), 0);
}

// ---------------------------------------------------------------------------------------
// Overlays (D24) and the parse options (D2, D15).

#[test]
fn an_overlay_merges_field_by_field() {
    let mut bundle = Bundle::builtin();
    bundle
        .overlay(
            "[tags.tag.attributes.numbered]\ntype = [\"String\", \"Boolean\"]\n\
             [tags.callout.attributes.type]\none_of = [\"note\", \"tip\"]\n\
             [tags.callout]\ndescription = \"Ours\"\n\
             [tags.lightindicators]\nself_closing = true\n\
             [tags.warning]\nalias_of = \"callout\"\n",
        )
        .unwrap();
    let config = bundle.config();
    // `tag` keeps its other attributes; `numbered` gains a type and keeps its description.
    let tag = config.tags["tag"].as_ref().unwrap();
    let attrs = tag.attributes.as_ref().unwrap();
    assert!(attrs.contains_key("primary") && attrs.contains_key("id"));
    assert!(matches!(
        attrs["numbered"].attr_type,
        Some(TypeSpec::Union(_))
    ));
    assert!(attrs["numbered"].description.is_some());
    assert!(tag.self_closing);
    // `callout.type` keeps its default and type; only the list changed.
    let callout = config.tags["callout"].as_ref().unwrap();
    assert_eq!(callout.description.as_deref(), Some("Ours"));
    let ty = &callout.attributes.as_ref().unwrap()["type"];
    assert_eq!(
        ty.default.as_ref().and_then(|d| d.to_scalar().ok()),
        Some(Scalar::String("note".into()))
    );
    assert!(matches!(&ty.matches, Some(Matches::Array(l)) if l.len() == 2));
    // The overlay's alias sees the merged callout, and so does `notice`.
    assert!(errors_of("{% warning type=\"tip\" %}\nA\n{% /warning %}\n", &bundle).is_empty());
    assert_eq!(bundle.aliases()["warning"], "callout");
    assert_eq!(
        html_of("{% lightindicators /%}\n", &bundle),
        "<article><lightindicators></lightindicators></article>"
    );
}

#[test]
fn a_failed_overlay_changes_nothing() {
    let mut bundle = Bundle::builtin();
    let before = format!("{:?}", bundle.config());
    assert!(
        bundle
            .overlay("[tags.callout]\ndescription = \"x\"\n[tags.bad]\nsize = 1\n")
            .is_err()
    );
    assert_eq!(format!("{:?}", bundle.config()), before);
}

#[test]
fn parse_and_render_tables_give_the_shared_options() {
    let mut bundle = Bundle::builtin();
    assert_eq!(bundle.parse_options(), ParseOptions::default());
    bundle
        .overlay(
            "[parse]\nallow_indentation = true\nallow_comments = true\nstrip_bom = true\n\
             frontmatter = \"yaml\"\n\n[render]\ntypographer = true\n",
        )
        .unwrap();
    let parse_options = bundle.parse_options();
    assert!(parse_options.tokenizer.allow_indentation && parse_options.tokenizer.allow_comments);
    assert!(parse_options.strip_bom);
    assert_eq!(parse_options.frontmatter, crate::ext::Frontmatter::Yaml);
    assert!(
        !parse_options
            .tokenizer
            .markdown_it
            .contains_key("typographer")
    );
    let render_options = bundle.render_options();
    assert_eq!(
        render_options.tokenizer.markdown_it.get("typographer"),
        Some(&serde_json::Value::Bool(true))
    );
    let config = bundle.config();
    let source = "\u{FEFF}# \"Quoted\" <!-- note -->\n";
    assert_eq!(
        html_with(source, &config, &render_options),
        "<article><h1>\u{201C}Quoted\u{201D} </h1></article>"
    );
    assert_eq!(
        html_with(source, &config, &parse_options),
        "<article><h1>&quot;Quoted&quot; </h1></article>"
    );
    // A later overlay overrides one key.
    bundle.overlay("[parse]\nallow_comments = false\n").unwrap();
    let options = bundle.parse_options();
    assert!(options.tokenizer.allow_indentation && !options.tokenizer.allow_comments);
}

#[test]
fn a_corpus_overlay_shaped_as_a_production_one_loads() {
    // The shapes of a production corpus's tags.overlay.toml and markdoc-pdf's own overlay
    // (MIGRATION-MAP §4.2, §4.3).
    let mut bundle = Bundle::builtin();
    bundle
        .overlay(
            r#"
[parse]
allow_indentation = true
allow_comments = true
strip_bom = true
frontmatter = "yaml"
[render]
typographer = true

[tags.ad]
description = "Ad slot"
self_closing = true
inline = false
[tags.ad.attributes.format]
type = ["String"]
one_of = ["leaderboard", "inline", "square"]
[tags.gridimage]
self_closing = false
[tags.gridimage.attributes.id]
type = ["String"]
required = true
[tags.gridimage.attributes.alt]
type = ["String"]
default = ""
[tags.colorBox]
self_closing = true
inline = true
[tags.colorBox.attributes.primary]
type = ["String"]
[tags.vimeo]
self_closing = true
[tags.vimeo.attributes.url]
type = ["String"]
required = true
pattern = '^(https://vimeo\.com/\S+|\d+)$'
[tags.mermaid.attributes.links]
type = ["Boolean"]
default = true

[tags.toc]
self_closing = true
[tags.footnote]
inline = true
[nodes.fence.attributes.align]
type = ["String"]
[nodes.image.attributes.size]
type = ["String"]
[nodes.image.attributes.width]
type = ["String", "Number"]
[nodes.image.attributes.side]
type = ["String"]
"#,
        )
        .unwrap();
    let source = "\u{FEFF}---\ntitle: T\n---\n# T\n\n{% gridimage id=\"a\" %}\nbody\n{% /gridimage %}\n\n\
                  Text {% colorBox \"x\" /%}.\n\n{% vimeo url=\"123\" /%}\n\n{% mermaid %}\n```mermaid\ngraph\n```\n{% /mermaid %}\n";
    assert!(
        errors_of(source, &bundle).is_empty(),
        "{:?}",
        errors_of(source, &bundle)
    );
    let html = html_with(source, &bundle.config(), &bundle.render_options());
    assert!(html.contains("<gridimage id=\"a\" alt=\"\">"), "{html}");
    assert!(html.contains("<colorBox primary=\"x\">"), "{html}");
    assert!(html.contains("<mermaid links=\"true\">"), "{html}");
    assert_eq!(
        errors_of("{% vimeo url=\"x\" /%}\n", &bundle),
        [
            "attribute-value-invalid: Attribute 'url' must match /^(https:\\/\\/vimeo\\.com\\/\\S+|\\d+)$/. Got 'x' instead."
        ]
    );
}

#[test]
fn a_bundle_can_be_printed_and_shared() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<Bundle>();
    // markdoc-lint derives Debug on a struct holding a Config (D21).
    #[derive(Debug)]
    #[allow(dead_code)]
    struct Corpus {
        config: Config,
    }
    let text = format!(
        "{:?}",
        Corpus {
            config: Bundle::builtin().config()
        }
    );
    assert!(text.contains("callout") && text.contains("<function>"));
}
