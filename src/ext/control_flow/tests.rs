//! The old pipeline's tests (`crate::conditionals`), on `next`: `if`/`else` through
//! [`evaluate_conditionals`], `switch` and `for` through both the tags' transforms and
//! [`evaluate_conditionals`]. Where upstream's semantics differ from the old crate's (D11:
//! `""` is truthy; an unset variable is `undefined`), the test says so.
use super::*;
use crate::next::index::{parse, transform, validate};
use crate::next::parser::ParserArgs;
use crate::next::renderers::html;
use serde_json::json;

fn doc(src: &str) -> Node {
    parse(src, &ParserArgs::default()).expect("parses")
}

fn with_variables(variables: serde_json::Value) -> Config {
    Config {
        variables: Some(Variables::Value(Value::from_json(&variables))),
        ..Config::default()
    }
}

/// The variables, and the control-flow tags.
fn config(variables: serde_json::Value) -> Config {
    let mut config = with_variables(variables);
    apply(&mut config);
    config
}

/// Through the tags' transforms.
fn html_with(src: &str, config: &Config) -> String {
    html::render(&transform(&doc(src), config).expect("transforms")).expect("renders")
}

fn html(src: &str, variables: serde_json::Value) -> String {
    html_with(src, &config(variables))
}

/// Through the AST pass.
fn ast(src: &str, variables: serde_json::Value) -> Node {
    evaluate_conditionals(&doc(src), &with_variables(variables)).expect("evaluates")
}

/// The AST pass, then `transform` (which no longer sees any of the tags).
fn ast_html(src: &str, variables: serde_json::Value) -> String {
    let config = with_variables(variables.clone());
    html_with_node(&ast(src, variables), &config)
}

fn html_with_node(node: &Node, config: &Config) -> String {
    html::render(&transform(node, config).expect("transforms")).expect("renders")
}

fn texts(node: &Node) -> Vec<String> {
    std::iter::once(node)
        .chain(node.walk())
        .filter(|n| n.node_type == NodeType::Text)
        .filter_map(|n| match n.attributes.get("content") {
            Some(Scalar::String(s)) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

fn count_text(node: &Node, needle: &str) -> usize {
    texts(node).iter().filter(|t| t.contains(needle)).count()
}

fn has_tag(node: &Node, name: &str) -> bool {
    std::iter::once(node)
        .chain(node.walk())
        .any(|n| is_tag(n, name))
}

fn none() -> serde_json::Value {
    json!({})
}

// ---------------------------------------------------------------------------------------
// if / else on the AST
// ---------------------------------------------------------------------------------------

#[test]
fn truthy_literal_keeps_children() {
    let result = ast("{% if true %}visible content{% /if %}", none());
    assert_eq!(count_text(&result, "visible content"), 1);
    assert!(!has_tag(&result, "if"));
}

#[test]
fn falsy_literal_drops_subtree() {
    let result = ast("before {% if false %}hidden{% /if %} after", none());
    assert_eq!(count_text(&result, "hidden"), 0);
    assert_eq!(count_text(&result, "before"), 1);
    assert_eq!(count_text(&result, "after"), 1);
    assert!(!has_tag(&result, "if"));
}

#[test]
fn variable_predicate_resolves() {
    let src = "{% if $config.show %}content{% /if %}";
    assert_eq!(
        count_text(&ast(src, json!({"config": {"show": true}})), "content"),
        1
    );
    assert_eq!(
        count_text(&ast(src, json!({"config": {"show": false}})), "content"),
        0
    );
}

#[test]
fn missing_variable_treated_as_falsy() {
    let result = ast("{% if $config.absent %}should not appear{% /if %}", none());
    assert_eq!(count_text(&result, "should not appear"), 0);
}

#[test]
fn function_call_predicate_evaluates() {
    let src = "{% if and(equals($a, $b), not($c)) %}kept{% /if %}";
    let result = ast(src, json!({"a": "x", "b": "x", "c": false}));
    assert_eq!(count_text(&result, "kept"), 1);
}

#[test]
fn nested_ifs_resolve_inside_out() {
    let src = "{% if true %}outer kept{% if false %}inner dropped{% /if %}{% /if %}";
    let result = ast(src, none());
    assert_eq!(count_text(&result, "outer kept"), 1);
    assert_eq!(count_text(&result, "inner dropped"), 0);
    assert!(!has_tag(&result, "if"));
}

#[test]
fn if_inside_callout_resolves_correctly() {
    let src = "{% callout type=\"warning\" %}\n{% if $config.has_estop %}E-stop is present.{% /if %}\n{% /callout %}";
    let result = ast(src, json!({"config": {"has_estop": true}}));
    assert!(has_tag(&result, "callout"));
    assert!(!has_tag(&result, "if"));
    assert_eq!(count_text(&result, "E-stop is present."), 1);
}

#[test]
fn if_with_no_predicate_is_falsy() {
    assert_eq!(count_text(&ast("{% if %}body{% /if %}", none()), "body"), 0);
}

#[test]
fn an_empty_string_is_truthy_as_upstream_has_it() {
    // D11: the old crate's truthiness made `""` (and 0) falsy; upstream's makes only false,
    // undefined and null falsy.
    let src = "{% if $config.name %}greet{% /if %}";
    for value in [json!(""), json!(0), json!("Alice"), json!([])] {
        let vars = json!({"config": {"name": value}});
        assert_eq!(count_text(&ast(src, vars.clone()), "greet"), 1, "{value}");
        assert_eq!(
            html_with(src, &with_variables(vars)),
            "<article><p>greet</p></article>"
        );
    }
    for value in [json!(null), json!(false)] {
        let vars = json!({"config": {"name": value}});
        assert_eq!(count_text(&ast(src, vars), "greet"), 0, "{value}");
    }
}

#[test]
fn else_branches() {
    let pick = |src: &str, vars: serde_json::Value| -> Vec<String> {
        texts(&ast(src, vars))
            .into_iter()
            .filter(|t| t.len() == 1)
            .collect()
    };
    assert_eq!(pick("{% if false %}A{% else /%}B{% /if %}", none()), ["B"]);
    assert_eq!(pick("{% if true %}A{% else /%}B{% /if %}", none()), ["A"]);
    let src = "{% if false %}A{% else $b /%}B{% else $c /%}C{% else /%}D{% /if %}";
    assert_eq!(pick(src, json!({"b": false, "c": true})), ["C"]);
    let src = "{% if false %}A{% else $b /%}B{% else /%}C{% /if %}";
    assert_eq!(pick(src, json!({"b": false})), ["C"]);
    let result = ast(
        "before {% if false %}A{% else $b /%}B{% /if %} after",
        none(),
    );
    assert_eq!(count_text(&result, "A") + count_text(&result, "B"), 0);
    assert_eq!(
        count_text(&result, "before") + count_text(&result, "after"),
        2
    );
}

#[test]
fn the_ast_pass_renders_what_upstreams_if_renders() {
    let src = "a\n\n{% if $x %}\nyes {% $x %}\n{% else /%}\nno\n{% /if %}\n\n{% if not($x) %}N{% /if %}\n";
    for vars in [json!({"x": 1}), json!({"x": null}), json!({})] {
        assert_eq!(
            ast_html(src, vars.clone()),
            html_with(src, &with_variables(vars.clone())),
            "{vars}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// switch / case
// ---------------------------------------------------------------------------------------

const REQUESTED: &str = "\
{% switch $item %}

{% case 1 %}
Case 1
{% /case %}

{% case 2 %}
Case 2
{% /case %}

{% /switch %}
";

#[test]
fn the_matching_case_is_the_only_one_kept() {
    assert_eq!(
        html(REQUESTED, json!({"item": 1})),
        "<article><p>Case 1</p></article>"
    );
    assert_eq!(
        html(REQUESTED, json!({"item": 2})),
        "<article><p>Case 2</p></article>"
    );
    let out = ast(REQUESTED, json!({"item": 2}));
    assert_eq!(texts(&out), ["Case 2"]);
    assert!(!has_tag(&out, "switch") && !has_tag(&out, "case"));
}

#[test]
fn nothing_matching_renders_nothing() {
    assert_eq!(html(REQUESTED, json!({"item": 9})), "<article></article>");
    assert!(texts(&ast(REQUESTED, json!({"item": 9}))).is_empty());
}

#[test]
fn a_valueless_case_is_the_default_branch() {
    let src = "{% switch $item %}\n{% case 1 %}one{% /case %}\n{% case %}fallback{% /case %}\n{% /switch %}\n";
    assert_eq!(html(src, json!({"item": 1})), "<article>one</article>");
    assert_eq!(html(src, json!({"item": 7})), "<article>fallback</article>");
    assert_eq!(texts(&ast(src, json!({"item": 7}))), ["fallback"]);
}

#[test]
fn order_decides_so_a_default_placed_first_swallows_the_rest() {
    let src = "{% switch $item %}\n{% case %}fallback{% /case %}\n{% case 1 %}one{% /case %}\n{% /switch %}\n";
    assert_eq!(html(src, json!({"item": 1})), "<article>fallback</article>");
    assert_eq!(texts(&ast(src, json!({"item": 1}))), ["fallback"]);
}

#[test]
fn matching_is_strict_as_upstreams_equals_is() {
    let src = "{% switch $item %}\n{% case 1 %}numeric{% /case %}\n{% /switch %}\n";
    assert_eq!(html(src, json!({"item": 1})), "<article>numeric</article>");
    assert_eq!(html(src, json!({"item": "1"})), "<article></article>");
    assert!(texts(&ast(src, json!({"item": "1"}))).is_empty());
}

#[test]
fn a_case_may_match_against_another_variable_and_strings_match() {
    let src = "{% switch $item %}\n{% case $wanted %}hit{% /case %}\n{% /switch %}\n";
    assert_eq!(
        html(src, json!({"item": "x", "wanted": "x"})),
        "<article>hit</article>"
    );
    let src = "{% switch $item %}\n{% case \"red\" %}warm{% /case %}\n{% case \"blue\" %}cool{% /case %}\n{% /switch %}\n";
    assert_eq!(
        html(src, json!({"item": "blue"})),
        "<article>cool</article>"
    );
    assert_eq!(texts(&ast(src, json!({"item": "blue"}))), ["cool"]);
}

#[test]
fn an_if_inside_a_case_and_a_switch_inside_a_case_resolve() {
    let src =
        "{% switch $item %}\n{% case 1 %}{% if $flag %}deep{% /if %}{% /case %}\n{% /switch %}\n";
    let vars = json!({"item": 1, "flag": true});
    assert_eq!(html(src, vars.clone()), "<article>deep</article>");
    let out = ast(src, vars);
    assert_eq!(texts(&out), ["deep"]);
    assert!(!has_tag(&out, "if"));

    let src = "{% switch $a %}\n{% case 1 %}{% switch $b %}{% case 2 %}both{% /case %}{% /switch %}{% /case %}\n{% /switch %}\n";
    let vars = json!({"a": 1, "b": 2});
    assert_eq!(html(src, vars.clone()), "<article>both</article>");
    let out = ast(src, vars);
    assert_eq!(texts(&out), ["both"]);
    assert!(!has_tag(&out, "switch"));
}

#[test]
fn a_missing_subject_matches_only_a_default() {
    let src = "{% switch $nosuch %}\n{% case 1 %}one{% /case %}\n{% case %}fallback{% /case %}\n{% /switch %}\n";
    assert_eq!(html(src, none()), "<article>fallback</article>");
    assert_eq!(texts(&ast(src, none())), ["fallback"]);
}

#[test]
fn a_case_outside_a_switch_renders_its_children() {
    assert_eq!(
        html("{% case 1 %}\nstray\n{% /case %}\n", none()),
        "<article><p>stray</p></article>"
    );
}

// ---------------------------------------------------------------------------------------
// for
// ---------------------------------------------------------------------------------------

fn robots() -> serde_json::Value {
    json!([{"name": "Bot100", "payload": 100}, {"name": "Bot250", "payload": 250}])
}

#[test]
fn the_body_is_repeated_once_per_element() {
    let src = "{% for $xs %}\nrepeated\n{% /for %}\n";
    let vars = json!({"xs": [1, 2, 3]});
    assert_eq!(
        html(src, vars.clone()),
        "<article><p>repeated</p><p>repeated</p><p>repeated</p></article>"
    );
    let out = ast(src, vars);
    assert_eq!(count_text(&out, "repeated"), 3);
    assert!(!has_tag(&out, "for"));
}

#[test]
fn the_loop_variable_reaches_the_rendered_page() {
    let src = "{% for $robots as=\"robot\" %}\n### {% $robot.name %}\n{% /for %}\n";
    let want = "<article><h3>Bot100</h3><h3>Bot250</h3></article>";
    assert_eq!(html(src, json!({"robots": robots()})), want);
    assert_eq!(ast_html(src, json!({"robots": robots()})), want);
}

#[test]
fn an_attribute_written_as_an_expression_is_bound_too() {
    let src = "{% for $robots as=\"robot\" %}\n{% callout type=$robot.name %}\nbody\n{% /callout %}\n{% /for %}\n";
    let mut config = config(json!({"robots": robots()}));
    let callout = Schema {
        render: Some("callout".into()),
        attributes: Some(IndexMap::from([(
            "type".to_string(),
            SchemaAttribute::default(),
        )])),
        ..Schema::default()
    };
    config.tags.insert("callout".into(), Some(callout));
    let want = "<article><callout type=\"Bot100\"><p>body</p></callout><callout type=\"Bot250\"><p>body</p></callout></article>";
    assert_eq!(html_with(src, &config), want);
    let bound = evaluate_conditionals(&doc(src), &config).unwrap();
    assert_eq!(html_with_node(&bound, &config), want);
}

#[test]
fn the_default_names_are_item_and_index_and_both_can_be_set() {
    let vars = json!({"xs": ["a", "b"]});
    let src = "{% for $xs %}\n{% $index %}:{% $item %}\n{% /for %}\n";
    let want = "<article><p>0:a</p><p>1:b</p></article>";
    assert_eq!(html(src, vars.clone()), want);
    assert_eq!(ast_html(src, vars.clone()), want);
    let src = "{% for $xs as=\"letter\" index=\"n\" %}\n{% $n %}:{% $letter %}\n{% /for %}\n";
    assert_eq!(html(src, vars.clone()), want);
    assert_eq!(ast_html(src, vars), want);
}

#[test]
fn an_empty_array_and_an_unset_variable_render_nothing() {
    let src = "a{% for $xs %}body{% /for %}b";
    assert_eq!(html(src, json!({"xs": []})), "<article><p>ab</p></article>");
    assert_eq!(
        html(src, json!({"xs": null})),
        "<article><p>ab</p></article>"
    );
    assert_eq!(html(src, none()), "<article><p>ab</p></article>");
    assert_eq!(count_text(&ast(src, none()), "body"), 0);
}

#[test]
fn a_subject_of_the_wrong_kind_is_reported() {
    let src = "{% for $robot %}body{% /for %}";
    let vars = json!({"robot": "Bot250"});
    let err = transform(&doc(src), &config(vars.clone())).expect_err("a string is not iterable");
    assert!(format!("{err:?}").contains("a string"), "{err:?}");
    let err = evaluate_conditionals(&doc(src), &with_variables(vars)).expect_err("the same");
    assert!(format!("{err:?}").contains("a string"), "{err:?}");
}

#[test]
fn naming_the_variable_with_an_expression_is_reported() {
    let src = "{% for $xs as=$name %}{% $item %}{% /for %}";
    let vars = json!({"xs": [1], "name": "thing"});
    assert!(transform(&doc(src), &config(vars.clone())).is_err());
    assert!(evaluate_conditionals(&doc(src), &with_variables(vars.clone())).is_err());
    let src = "{% for $xs as=\"\" %}{% $item %}{% /for %}";
    assert!(transform(&doc(src), &config(vars)).is_err());
}

#[test]
fn an_if_and_a_function_call_inside_the_body_see_the_loop_variable() {
    let vars = json!({"robots": robots()});
    let src = "{% for $robots as=\"robot\" %}\n{% if $robot.payload %}{% $robot.name %} carries{% /if %}\n{% /for %}\n";
    let want = "<article><p>Bot100 carries</p><p>Bot250 carries</p></article>";
    assert_eq!(html(src, vars.clone()), want);
    assert_eq!(ast_html(src, vars.clone()), want);
    let src = "{% for $robots as=\"robot\" %}\n{% if equals($robot.name, \"Bot250\") %}chosen{% /if %}\n{% /for %}\n";
    assert_eq!(html(src, vars.clone()).matches("chosen").count(), 1);
    assert_eq!(ast_html(src, vars).matches("chosen").count(), 1);
}

#[test]
fn a_loop_nests_inside_a_loop() {
    let vars = json!({"groups": [
        {"name": "outer1", "parts": ["p1", "p2"]},
        {"name": "outer2", "parts": ["p3"]}
    ]});
    let src = "{% for $groups as=\"group\" %}\n{% $group.name %}\n{% for $group.parts as=\"part\" %}\n- {% $part %} of {% $group.name %}\n{% /for %}\n{% /for %}\n";
    let want = "<article><p>outer1</p><ul><li>p1 of outer1</li><li>p2 of outer1</li></ul><p>outer2</p><ul><li>p3 of outer2</li></ul></article>";
    assert_eq!(html(src, vars.clone()), want);
    assert_eq!(ast_html(src, vars), want);
}

#[test]
fn an_inner_loop_that_reuses_a_name_shadows_the_outer_one() {
    let vars = json!({"outer": ["O"], "inner": ["x", "y"]});
    let src = "{% for $outer %}\n{% for $inner %}[{% $item %}]{% /for %}\n{% /for %}\n";
    let want = "<article><p>[x][y]</p></article>";
    assert_eq!(html(src, vars.clone()), want);
    assert_eq!(ast_html(src, vars), want);
}

#[test]
fn an_expression_that_is_not_the_loops_is_left_for_transform() {
    let src = "{% for $xs %}{% $config.product %}{% /for %}";
    let resolved = ast(src, json!({"xs": [1]}));
    let still_a_variable = resolved.walk().any(|n| {
        matches!(n.attributes.get("content"), Some(Scalar::Variable(v))
            if v.path.first() == Some(&PathSegment::String("config".into())))
    });
    assert!(
        still_a_variable,
        "the loop resolved a variable that was not its own"
    );
    let later = with_variables(json!({"config": {"product": "Bot1200"}}));
    assert_eq!(
        html_with_node(&resolved, &later),
        "<article><p>Bot1200</p></article>"
    );
}

#[test]
fn a_loop_inside_a_case_resolves() {
    let src = "{% switch $kind %}\n{% case \"list\" %}{% for $xs %}[{% $item %}]{% /for %}{% /case %}\n{% /switch %}\n";
    let vars = json!({"kind": "list", "xs": ["a"]});
    assert_eq!(html(src, vars.clone()), "<article>[a]</article>");
    assert_eq!(ast_html(src, vars), "<article>[a]</article>");
}

#[test]
fn a_body_that_is_one_bullet_stays_one_list() {
    let src = "{% for $xs %}\n- item {% $item %}\n{% /for %}\n";
    let vars = json!({"xs": ["a", "b", "c"]});
    let want = "<article><ul><li>item a</li><li>item b</li><li>item c</li></ul></article>";
    assert_eq!(html(src, vars.clone()), want);
    let out = ast(src, vars.clone());
    let lists: Vec<usize> = out
        .walk()
        .filter(|n| n.node_type == NodeType::List)
        .map(|n| n.children.len())
        .collect();
    assert_eq!(lists, [3]);
    assert_eq!(ast_html(src, vars), want);
}

#[test]
fn an_ordered_list_is_not_joined_to_an_unordered_one() {
    let src = "{% for $xs %}\n- bullet\n\n1. numbered\n{% /for %}\n";
    let vars = json!({"xs": [1, 2]});
    let want = "<article><ul><li>bullet</li></ul><ol><li>numbered</li></ol><ul><li>bullet</li></ul><ol><li>numbered</li></ol></article>";
    assert_eq!(html(src, vars.clone()), want);
    assert_eq!(ast_html(src, vars), want);
}

#[test]
fn the_first_node_of_the_first_iteration_survives() {
    let src = "{% for $xs %}\n## {% $item %}\n{% /for %}\n";
    let vars = json!({"xs": ["first", "second"]});
    let want = "<article><h2>first</h2><h2>second</h2></article>";
    assert_eq!(html(src, vars.clone()), want);
    assert_eq!(ast_html(src, vars), want);
}

#[test]
fn a_literal_array_can_be_iterated() {
    assert_eq!(
        html("{% for [1, 2, 3] %}x{% /for %}", none()),
        "<article><p>xxx</p></article>"
    );
    assert_eq!(
        count_text(&ast("{% for [1, 2, 3] %}x{% /for %}", none()), "x"),
        3
    );
}

#[test]
fn variables_given_as_a_function_still_reach_the_body() {
    let mut config = Config {
        variables: Some(Variables::Fn(Arc::new(|path: &[PathSegment]| {
            match path.first() {
                Some(PathSegment::String(s)) if s == "xs" => {
                    Value::from_json(&json!([{"n": "a"}, {"n": "b"}]))
                }
                Some(PathSegment::String(s)) if s == "sep" => Value::from("-"),
                _ => Value::Undefined,
            }
        }))),
        ..Config::default()
    };
    apply(&mut config);
    let src = "{% for $xs %}{% $item.n %}{% $sep %}{% $index %} {% /for %}";
    assert_eq!(
        html_with(src, &config),
        "<article><p>a-0 b-1 </p></article>"
    );
}

// ---------------------------------------------------------------------------------------
// Off by default; apply
// ---------------------------------------------------------------------------------------

#[test]
fn without_the_schemas_the_tags_are_upstreams_undeclared_tags() {
    let src = "{% for $xs %}\nbody\n{% /for %}\n\n{% switch 1 %}{% case 2 %}two{% /case %}{% /switch %}\n";
    let plain = with_variables(json!({"xs": [1, 2]}));
    // Upstream: undeclared tags render their children once.
    assert_eq!(
        html_with(src, &plain),
        "<article><p>body</p><p>two</p></article>"
    );
    let ids: Vec<String> = validate(&doc(src), &plain)
        .unwrap()
        .into_iter()
        .map(|e| e.error.id)
        .collect();
    assert_eq!(ids, ["tag-undefined", "tag-undefined", "tag-undefined"]);
    // With them, nothing else changes: a document without the tags renders the same.
    let other = "# T {% .c %}\n\n{% if $xs %}\n- a\n{% else /%}\nb\n{% /if %}\n\n{% table %}\n- x\n{% /table %}\n";
    assert_eq!(html_with(other, &plain), html(other, json!({"xs": [1, 2]})));
    assert!(
        validate(&doc(src), &config(json!({"xs": [1]})))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn apply_completes_a_schema_the_config_already_has() {
    let mut config = Config::default();
    config.tags.insert(
        "for".into(),
        Some(Schema {
            description: Some("the bundle's".into()),
            attributes: Some(IndexMap::from([(
                "as".to_string(),
                SchemaAttribute {
                    description: Some("the bundle's name".into()),
                    ..SchemaAttribute::default()
                },
            )])),
            ..Schema::default()
        }),
    );
    config.tags.insert("switch".into(), None);
    apply(&mut config);
    let schema = config.tags["for"].as_ref().unwrap();
    assert_eq!(schema.description.as_deref(), Some("the bundle's"));
    assert!(schema.transform.is_some());
    let attributes = schema.attributes.as_ref().unwrap();
    assert_eq!(
        attributes["as"].description.as_deref(),
        Some("the bundle's name")
    );
    assert!(attributes.contains_key("primary") && attributes.contains_key("index"));
    assert!(config.tags["switch"].is_none(), "a hidden tag stays hidden");
    assert!(config.tags["case"].is_some());
}

#[test]
fn the_bundles_schemas_get_the_layers_behaviour() {
    let bundle = Bundle::builtin();
    let mut config = bundle.config();
    assert!(
        !config.tags.contains_key("for"),
        "the bundle leaves its layer's tags out"
    );
    apply_bundle(&bundle, &mut config);
    for name in ["switch", "case", "for"] {
        let schema = config.tags[name].as_ref().unwrap();
        assert_eq!(
            schema.description.as_deref(),
            bundle.description(name),
            "{name} keeps the bundle's description"
        );
    }
    config.variables = Some(Variables::Value(Value::from_json(
        &json!({"item": 2, "robots": robots()}),
    )));
    assert_eq!(
        html_with(REQUESTED, &config),
        "<article><p>Case 2</p></article>"
    );
    let src = "{% for $robots as=\"robot\" %}\n- {% $robot.name %}\n{% /for %}\n";
    assert_eq!(
        html_with(src, &config),
        "<article><ul><li>Bot100</li><li>Bot250</li></ul></article>"
    );
    for src in [REQUESTED, src] {
        let errors = validate(&doc(src), &config).unwrap();
        assert!(
            errors.iter().all(|e| e.error.id == "variable-undefined"),
            "{:?}",
            errors.iter().map(|e| &e.error.id).collect::<Vec<_>>()
        );
    }
    // A hidden tag stays hidden.
    let mut config = bundle.config();
    config.tags.insert("for".into(), None);
    apply_bundle(&bundle, &mut config);
    assert!(config.tags["for"].is_none());
}

#[test]
fn the_shipped_example_renders_through_the_layers() {
    // examples/control_flow.md, as examples/control_flow.rs renders it: E1's YAML
    // frontmatter, E2's bundle, this layer, E7's positional functions and E8's variables.
    // The old pipeline rendered the same content (in a `div`, with `softbreak` tags).
    use crate::ext::functions::positional;
    use crate::ext::variables::frontmatter_variables;
    use crate::ext::{Frontmatter, ParseOptions};
    let options = ParseOptions {
        frontmatter: Frontmatter::Yaml,
        ..ParseOptions::default()
    };
    let document =
        crate::ext::parse(include_str!("../../../examples/control_flow.md"), &options).unwrap();
    let bundle = Bundle::builtin();
    let mut config = bundle.config();
    apply_bundle(&bundle, &mut config);
    let count = positional(|args: &[Scalar]| match args {
        [Scalar::Array(items)] => Ok(Scalar::Number(items.len() as f64)),
        _ => Err(error("count() takes one array")),
    });
    let upper = positional(|args: &[Scalar]| match args {
        [Scalar::String(s)] => Ok(Scalar::String(s.to_uppercase())),
        [other] => Ok(other.clone()),
        _ => Err(error("upper() takes one argument")),
    });
    config.functions.insert("count".into(), Some(count));
    config.functions.insert("upper".into(), Some(upper));
    frontmatter_variables(&document)
        .with_value(
            "fleet",
            Value::from_json(&json!([
                {"name": "Bot100", "status": "ok", "payload": 100, "modules": ["shelf lift", "top module"]},
                {"name": "Bot250", "status": "service", "payload": 250, "modules": ["hook"]},
                {"name": "Bot1200", "status": "retired", "payload": 1200, "modules": []}
            ])),
        )
        .apply(&mut config);
    assert_eq!(
        html_with_node(&document, &config),
        "<article><h1>Fleet readiness</h1><p>3 robots are configured.</p><h2>Bot100</h2>\
         <callout type=\"success\"><p>Ready. Payload 100 kg.</p></callout><ul><li>Slot 0: SHELF \
         LIFT</li><li>Slot 1: TOP MODULE</li></ul><h2>Bot250</h2><callout type=\"warning\"><p>Out \
         for service. Payload 250 kg when it returns.</p></callout><ul><li>Slot 0: HOOK</li></ul>\
         <h2>Bot1200</h2><callout type=\"danger\"><p>Unknown status: retired.</p></callout></article>"
    );
    // The loop variables are unknown to upstream's validator (D10); nothing else is reported.
    let ids: std::collections::BTreeSet<String> = validate(&document, &config)
        .unwrap()
        .into_iter()
        .map(|e| e.error.id)
        .collect();
    assert_eq!(ids.into_iter().collect::<Vec<_>>(), ["variable-undefined"]);
}

#[test]
fn loops_nested_too_deep_in_the_ast_pass_are_an_error() {
    // Made by hand: markdown-it's own `maxNesting` (100) cuts a document's tags off first.
    let nested = |depth: usize| {
        let mut node = Node::new(
            NodeType::Text,
            [("content".to_string(), Scalar::String("x".into()))],
            vec![],
            None,
        );
        for _ in 0..depth {
            let subject = Scalar::Array(vec![Scalar::Number(1.0)]);
            node = Node::new(
                NodeType::Tag,
                [("primary".to_string(), subject)],
                vec![node],
                Some("for".into()),
            );
        }
        Node::new(NodeType::Document, std::iter::empty(), vec![node], None)
    };
    let handle = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || {
            let config = Config::default();
            let err = evaluate_conditionals(&nested(MAX_LOOP_NESTING + 1), &config).unwrap_err();
            let ok = evaluate_conditionals(&nested(MAX_LOOP_NESTING), &config).map(|n| texts(&n));
            (format!("{err:?}"), ok)
        })
        .unwrap();
    let (err, ok) = handle.join().unwrap();
    assert!(err.contains("nested more than"), "{err}");
    assert_eq!(ok.unwrap(), ["x"]);
}

#[test]
fn a_deep_tree_is_no_problem_for_the_ast_pass() {
    let src = "p {% x %} q\n\n".repeat(2000) + "{% if true %}yes{% /if %}";
    let parsed = doc(&src);
    let handle = std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || {
            let out = evaluate_conditionals(&parsed, &Config::default()).unwrap();
            (has_tag(&out, "if"), count_text(&out, "yes"))
        })
        .unwrap();
    assert_eq!(handle.join().unwrap(), (false, 1));
}
