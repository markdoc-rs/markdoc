//! A corpus's own schema files through the bundle loader, and its documents through
//! `validate` with them: the overlay (`.tooling/schemas/tags.overlay.toml`) and then
//! `markdoc.toml` (`[parse]`, `[render]`, `[validation]`, `[functions.*]`), both with
//! `Bundle::overlay`, as every tool that reads the corpus loads them.
//!
//! The corpus is not part of this crate, so the test runs only when told where one is:
//!
//! ```text
//! MARKDOC_CORPUS=/path/to/corpus cargo test --test corpus_bundle -- --ignored
//! ```

use markdoc::ext::bundle::Bundle;
use markdoc::ext::functions::{include, includes, positional};
use markdoc::types::Scalar;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const OVERLAY: &str = ".tooling/schemas/tags.overlay.toml";
const CORPUS_TOML: &str = "markdoc.toml";

/// The corpus's documents: every `.md` file but those under `translations/`, `node_modules/`
/// and hidden directories.
fn documents(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "translations" && name != "node_modules" && !name.starts_with('.') {
                documents(&path, out);
            }
        } else if name.ends_with(".md") {
            out.push(path);
        }
    }
}

#[test]
#[ignore = "needs MARKDOC_CORPUS=<a corpus with markdoc.toml and the tag overlay>"]
fn the_corpus_schema_files_load_and_its_functions_validate() {
    let root = PathBuf::from(
        std::env::var("MARKDOC_CORPUS").expect("MARKDOC_CORPUS names the corpus's root"),
    );
    let read = |rel: &str| {
        std::fs::read_to_string(root.join(rel))
            .unwrap_or_else(|e| panic!("{}: {e}", root.join(rel).display()))
    };
    let mut bundle = Bundle::builtin();
    bundle.overlay(&read(OVERLAY)).expect("the overlay loads");
    bundle
        .overlay(&read(CORPUS_TOML))
        .expect("markdoc.toml loads");

    // `[validation]` and `[functions.*]` reach the config.
    let mut config = bundle.config();
    assert!(config.validation.validate_functions);
    let declared: Vec<&str> = bundle.function_names().collect();
    assert_eq!(declared, ["higher", "lower", "include", "includes"]);
    for name in &declared {
        let function = config.functions[*name].as_ref().unwrap();
        let parameters = function.parameters.as_ref().unwrap();
        assert_eq!(parameters.keys().collect::<Vec<_>>(), ["0", "1"], "{name}");
        assert!(parameters.values().all(|p| p.required), "{name}");
        assert!(function.description.is_some(), "{name}");
    }
    // `[parse]` and `[render]` as before.
    let options = bundle.parse_options();
    assert!(options.tokenizer.allow_indentation && options.tokenizer.allow_comments);
    assert!(
        bundle.render_options().tokenizer.markdown_it["typographer"] == serde_json::json!(true)
    );

    // R3: until the tool registers code for every declared function, it must not start.
    assert_eq!(
        bundle.functions_without_code(&config),
        ["higher", "lower", "include", "includes"]
    );
    config.functions.insert("include".into(), Some(include()));
    config.functions.insert("includes".into(), Some(includes()));
    assert_eq!(bundle.functions_without_code(&config), ["higher", "lower"]);
    for name in ["higher", "lower"] {
        config
            .functions
            .insert(name.into(), Some(positional(|_| Ok(Scalar::Boolean(true)))));
    }
    bundle.apply(&mut config);
    bundle.check_functions(&config).unwrap();
    assert!(
        config.functions["include"]
            .as_ref()
            .unwrap()
            .parameters
            .is_some()
    );

    // Every document validates without a function finding: every call is declared, and every
    // call's arguments fit the declaration.
    let mut files = Vec::new();
    documents(&root, &mut files);
    assert!(files.len() > 1000, "{} documents", files.len());
    let mut by_id: BTreeMap<String, usize> = BTreeMap::new();
    let mut function_findings = Vec::new();
    for file in &files {
        let source = std::fs::read_to_string(file).unwrap();
        let rel = file.strip_prefix(&root).unwrap().display().to_string();
        let doc = markdoc::ext::parse(&source, &options.with_file(rel.clone()))
            .unwrap_or_else(|e| panic!("{rel}: {e}"));
        for error in markdoc::validate(&doc, &config).unwrap() {
            let id = error.error.id.clone();
            if id.starts_with("function-") || id.starts_with("parameter-") {
                function_findings.push(format!("{rel}: {id}: {}", error.error.message));
            }
            *by_id.entry(id).or_default() += 1;
        }
    }
    println!("{} documents; findings by id: {by_id:?}", files.len());
    assert!(function_findings.is_empty(), "{function_findings:#?}");

    // Without markdoc.toml the same calls are undeclared.
    let mut overlay_only = Bundle::builtin();
    overlay_only.overlay(&read(OVERLAY)).unwrap();
    let mut strict = overlay_only.config();
    strict.validation.validate_functions = true;
    let undeclared = files
        .iter()
        .map(|file| {
            let source = std::fs::read_to_string(file).unwrap();
            let doc = markdoc::ext::parse(&source, &options).unwrap();
            markdoc::validate(&doc, &strict)
                .unwrap()
                .iter()
                .filter(|e| e.error.id == "function-undefined")
                .count()
        })
        .sum::<usize>();
    println!("without markdoc.toml: {undeclared} function-undefined");
    assert!(undeclared > 0);
}
