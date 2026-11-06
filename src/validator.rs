use crate::ast::Node;
use crate::types::*;

pub fn validate(node: &Node, config: &Config) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    validate_node(node, config, &mut errors);
    errors
}

fn validate_node(node: &Node, config: &Config, errors: &mut Vec<ValidationError>) {
    // Add existing node errors
    errors.extend(node.errors.clone());

    // Find schema
    let schema = find_schema(node, config);

    // Validate attributes
    if let Some(schema) = &schema {
        validate_attributes(node, schema, errors);
        validate_children(node, schema, errors);
    }

    // Recursively validate children
    for child in &node.children {
        validate_node(child, config, errors);
    }

    // Validate slots
    for slot in node.slots.values() {
        validate_node(slot, config, errors);
    }
}

fn find_schema(node: &Node, config: &Config) -> Option<Schema> {
    if let Some(tag) = &node.tag
        && let Some(schema) = config.tags.get(tag)
    {
        return Some(schema.clone());
    }

    let node_key = node.node_type.to_string();
    config.nodes.get(&node_key).cloned()
}

fn validate_attributes(node: &Node, schema: &Schema, errors: &mut Vec<ValidationError>) {
    if let Some(schema_attrs) = &schema.attributes {
        // Check required attributes
        for (attr_name, attr_schema) in schema_attrs {
            if attr_schema.required && !node.attributes.contains_key(attr_name) {
                errors.push(ValidationError {
                    id: "missing-attribute".to_string(),
                    level: ValidationLevel::Error,
                    message: format!(
                        "Required attribute '{}' is missing from '{}'",
                        attr_name,
                        node.tag.as_deref().unwrap_or(&node.node_type.to_string())
                    ),
                    location: node.location.clone(),
                });
            }
        }

        // An attribute the schema does not define.
        //
        // Every renderer ignores one silently, which is the whole reason this
        // check exists: `{% table colomn_weights="1 2" %}` is a typo for
        // `column_weights`, it parses, it validates, it renders, and the
        // table comes out with the automatic column widths the author was
        // trying to override. Thirty-six pages of one corpus carried exactly
        // that before anything looked.
        //
        // `primary` is exempt because it is not a named attribute at all: it
        // is where the parser puts `{% tag "value" %}`, and most schemas have
        // no reason to declare it. `frontmatter` likewise is put on the
        // document by the parser rather than written by an author.
        for attr_name in node.attributes.keys() {
            if attr_name == "primary" || attr_name == "frontmatter" {
                continue;
            }
            if schema_attrs.contains_key(attr_name) {
                continue;
            }
            let mut message = format!(
                "Attribute '{}' is not defined for '{}'",
                attr_name,
                node.tag.as_deref().unwrap_or(&node.node_type.to_string())
            );
            // A near-miss is almost always a typo, and saying which one turns
            // a puzzle into a fix.
            if let Some(near) = nearest(attr_name, schema_attrs.keys()) {
                message.push_str(&format!(" (did you mean '{near}'?)"));
            }
            errors.push(ValidationError {
                id: "undefined-attribute".to_string(),
                level: ValidationLevel::Error,
                message,
                location: node.location.clone(),
            });
        }

        // Validate attribute types
        for (attr_name, attr_value) in &node.attributes {
            if let Some(attr_schema) = schema_attrs.get(attr_name)
                && let Some(valid_types) = &attr_schema.attr_type
                && !validate_type(attr_value, valid_types)
            {
                errors.push(ValidationError {
                    id: "invalid-attribute-type".to_string(),
                    level: ValidationLevel::Error,
                    message: format!(
                        "Attribute '{}' has invalid type for '{}'",
                        attr_name,
                        node.tag.as_deref().unwrap_or(&node.node_type.to_string())
                    ),
                    location: node.location.clone(),
                });
            }
        }
    }
}

/// The closest defined name, when it is close enough to be a typo.
///
/// Edit distance over a handful of short names, so the cost is nothing and
/// the gain is that `colomn_weights` names `column_weights` instead of
/// leaving the author to spot a transposition by eye. The threshold scales
/// with the name's length: two edits in `id` is a different word, two edits
/// in `column_weights` is a slip.
fn nearest<'a>(name: &str, candidates: impl Iterator<Item = &'a String>) -> Option<&'a str> {
    let limit = (name.len() / 4).clamp(1, 3);
    candidates
        .map(|c| (edit_distance(name, c), c.as_str()))
        .filter(|(d, _)| *d <= limit)
        .min_by_key(|(d, c)| (*d, c.len()))
        .map(|(_, c)| c)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn validate_children(node: &Node, schema: &Schema, errors: &mut Vec<ValidationError>) {
    if let Some(allowed_children) = &schema.children {
        for child in &node.children {
            let child_type_string = child.node_type.to_string();
            let child_type = child.tag.as_deref().unwrap_or(&child_type_string);
            if !allowed_children.contains(&child_type.to_string()) {
                errors.push(ValidationError {
                    id: "invalid-child".to_string(),
                    level: ValidationLevel::Warning,
                    message: format!(
                        "Node '{}' is not allowed as child of '{}'",
                        child_type,
                        node.tag.as_deref().unwrap_or(&node.node_type.to_string())
                    ),
                    location: child.location.clone(),
                });
            }
        }
    }
}

fn validate_type(value: &Scalar, valid_types: &[ValidationType]) -> bool {
    for valid_type in valid_types {
        let matches = matches!(
            (value, valid_type),
            (Scalar::String(_), ValidationType::String)
                | (Scalar::Number(_), ValidationType::Number)
                | (Scalar::Boolean(_), ValidationType::Boolean)
                | (Scalar::Array(_), ValidationType::Array)
                | (Scalar::Object(_), ValidationType::Object)
                | (Scalar::Null, _)
        );

        if matches {
            return true;
        }
    }

    false
}
