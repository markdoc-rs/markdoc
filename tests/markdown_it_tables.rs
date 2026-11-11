//! The generated tables of the markdown-it port (src/markdown_it/common/{entities,ucmicro}.rs)
//! are exactly what tools/gen_mdit_tables.cjs produces from the npm packages.
//!
//! Needs node and a node_modules holding `entities` and `uc.micro`: `MDIT_NODE_MODULES`, or
//! `node_modules` in the crate (`npm install entities uc.micro`). Without them it says it skipped.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn generated_tables_match_their_generator() {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let node_modules = std::env::var_os("MDIT_NODE_MODULES")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate_dir.join("node_modules"));
    if !node_modules.join("entities").is_dir() || !node_modules.join("uc.micro").is_dir() {
        eprintln!(
            "SKIPPED: no entities and uc.micro under {} (set MDIT_NODE_MODULES)",
            node_modules.display()
        );
        return;
    }
    let status = match Command::new("node")
        .arg(crate_dir.join("tools/gen_mdit_tables.cjs"))
        .arg(&node_modules)
        .arg("--check")
        .status()
    {
        Ok(status) => status,
        Err(e) => {
            eprintln!("SKIPPED: cannot run node: {e}");
            return;
        }
    };
    assert!(
        status.success(),
        "src/markdown_it/common/{{entities,ucmicro}}.rs differ from what tools/gen_mdit_tables.cjs generates"
    );
}
