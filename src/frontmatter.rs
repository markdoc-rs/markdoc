use crate::types::*;
use std::collections::HashMap;

/// Extract and parse YAML frontmatter from markdown content
/// Split frontmatter from body.
///
/// The third element is the byte offset in `content` where the returned body
/// begins. Everything before it was consumed by the frontmatter, and that one
/// number is the whole source map for this rewrite -- which is why the body is
/// now *sliced* out of `content` rather than rebuilt by joining lines. Joining
/// normalised CRLF to LF and dropped a trailing newline, so offsets into the
/// result did not correspond to bytes in the file. Every writer here is on
/// Windows, so that was a latent wrong-position bug waiting for the first
/// CRLF checkout.
///
/// Returns the parsed frontmatter if there was any, the content with it
/// removed, and how many bytes were removed -- the last so a caller can keep
/// positions pointing into the file the author has open.
pub type Frontmatter = (Option<HashMap<String, Scalar>>, String, usize);

pub fn extract_frontmatter_mapped(content: &str) -> Result<Frontmatter> {
    let lines: Vec<&str> = content.lines().collect();

    // Check if content starts with frontmatter delimiter
    if lines.is_empty() || lines[0].trim() != "---" {
        return Ok((None, content.to_string(), 0));
    }

    // Find closing delimiter
    let mut end_line = None;
    for (i, line) in lines.iter().enumerate().skip(1) {
        if line.trim() == "---" {
            end_line = Some(i);
            break;
        }
    }

    let Some(end) = end_line else {
        // No closing delimiter found, treat as regular content
        return Ok((None, content.to_string(), 0));
    };

    // Extract frontmatter content
    let frontmatter_content = lines[1..end].join("\n");

    // Parse YAML
    let frontmatter: HashMap<String, serde_yaml::Value> =
        serde_yaml::from_str(&frontmatter_content)
            .map_err(|e| MarkdocError::ParseError(format!("Invalid YAML frontmatter: {}", e)))?;

    // Convert to Scalar
    let frontmatter_scalar = frontmatter
        .into_iter()
        .map(|(k, v)| (k, yaml_to_scalar(v)))
        .collect();

    // Where the body starts, as a byte offset into `content`: skip `end + 1`
    // physical lines, counting their real terminators rather than assuming
    // one byte each.
    let offset: usize = content
        .split_inclusive('\n')
        .take(end + 1)
        .map(str::len)
        .sum();
    let remaining_content = content[offset.min(content.len())..].to_string();

    Ok((Some(frontmatter_scalar), remaining_content, offset))
}

/// Frontmatter and body, without the offset. Kept for callers that only want
/// the data.
pub fn extract_frontmatter(content: &str) -> Result<(Option<HashMap<String, Scalar>>, String)> {
    let (fm, body, _) = extract_frontmatter_mapped(content)?;
    Ok((fm, body))
}

fn yaml_to_scalar(value: serde_yaml::Value) -> Scalar {
    match value {
        serde_yaml::Value::Null => Scalar::Null,
        serde_yaml::Value::Bool(b) => Scalar::Boolean(b),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Scalar::Number(i as f64)
            } else if let Some(f) = n.as_f64() {
                Scalar::Number(f)
            } else {
                Scalar::Null
            }
        }
        serde_yaml::Value::String(s) => Scalar::String(s),
        serde_yaml::Value::Sequence(seq) => {
            Scalar::Array(seq.into_iter().map(yaml_to_scalar).collect())
        }
        serde_yaml::Value::Mapping(map) => Scalar::Object(
            map.into_iter()
                .filter_map(|(k, v)| {
                    if let serde_yaml::Value::String(key) = k {
                        Some((key, yaml_to_scalar(v)))
                    } else {
                        None
                    }
                })
                .collect(),
        ),
        _ => Scalar::Null,
    }
}
