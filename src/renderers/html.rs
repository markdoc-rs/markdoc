use crate::types::*;

pub fn render(node: &RenderableTreeNode) -> String {
    match node {
        RenderableTreeNode::Tag(tag) => render_tag(tag),
        RenderableTreeNode::Scalar(scalar) => render_scalar(scalar),
    }
}

fn render_tag(tag: &Tag) -> String {
    let mut html = String::new();

    // Opening tag
    html.push('<');
    html.push_str(&tag.name);

    // Attributes, in sorted order.
    //
    // `Tag::attributes` is a HashMap and Rust randomises its iteration order
    // per process, so the same tree rendered twice produced different bytes --
    // observed in one file: `<image id=… size=…>` and `<image size=… id=…>`.
    // Any consumer that hashes the output (the support portal content-hashes
    // every page for its ETag table) would then see every page change on every
    // build. Sorting is the smallest fix that makes a render reproducible;
    // preserving *source* order would mean an IndexMap and a public API
    // change, which is worth doing separately.
    let mut names: Vec<&String> = tag.attributes.keys().collect();
    names.sort();
    for key in names {
        html.push(' ');
        html.push_str(key);
        html.push_str("=\"");
        html.push_str(&escape_html(&scalar_to_string(&tag.attributes[key])));
        html.push('"');
    }

    // Self-closing or with content
    if tag.children.is_empty() && is_void_element(&tag.name) {
        html.push_str(" />");
    } else {
        html.push('>');

        // Children
        for child in &tag.children {
            html.push_str(&render(child));
        }

        // Closing tag
        html.push_str("</");
        html.push_str(&tag.name);
        html.push('>');
    }

    html
}

fn render_scalar(scalar: &Scalar) -> String {
    match scalar {
        Scalar::String(s) => escape_html(s),
        Scalar::Number(n) => n.to_string(),
        Scalar::Boolean(b) => b.to_string(),
        Scalar::Null => String::new(),
        Scalar::Array(arr) => arr.iter().map(render_scalar).collect::<Vec<_>>().join(""),
        Scalar::Object(_) => String::new(), // Objects don't render directly
    }
}

fn scalar_to_string(scalar: &Scalar) -> String {
    match scalar {
        Scalar::String(s) => s.clone(),
        Scalar::Number(n) => n.to_string(),
        Scalar::Boolean(b) => b.to_string(),
        Scalar::Null => String::new(),
        Scalar::Array(_) | Scalar::Object(_) => String::new(),
    }
}

fn escape_html(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            '\'' => "&#x27;".to_string(),
            _ => c.to_string(),
        })
        .collect()
}

fn is_void_element(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}
