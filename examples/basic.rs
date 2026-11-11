//! Parse, validate, transform and render one document with upstream's defaults.
//!
//! ```sh
//! cargo run --example basic
//! ```

use markdoc::parser::ParserArgs;
use markdoc::renderers::html;
use markdoc::schema::Config;
use markdoc::{parse, transform, validate};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Example markdown content
    let markdown = r#"
# Hello World

This is a **Markdoc** example written in _Markdown_.

## Features

- Parse Markdown with Markdoc's tokenizer (markdown-it)
- Transform to a renderable tree
- Validate content
- Render to HTML

## Code Example

```rust
fn main() {
    println!("Hello, Markdoc!");
}
```

[Visit Markdoc](https://markdoc.dev)
"#;

    // Parse the markdown
    println!("Parsing markdown...");
    let ast = parse(markdown, &ParserArgs::default())?;

    // Validate and transform with upstream's default config
    let config = Config::default();
    println!("Validating AST...");
    for error in validate(&ast, &config)? {
        println!("  {}: {}", error.error.id, error.error.message);
    }
    println!("Transforming AST...");
    let rendered = transform(&ast, &config)?;

    // Render to HTML
    println!("Rendering to HTML...");
    let html = html::render(&rendered)?;

    println!("\n=== Generated HTML ===\n");
    println!("{html}");

    Ok(())
}
