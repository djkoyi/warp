//! Markdown files with a YAML frontmatter block (`SKILL.md`, subagents, slash commands).
//!
//! Unknown frontmatter keys are kept, in their original order, when a document is edited and
//! rendered again. YAML comments inside the frontmatter are not preserved.

use serde_yaml::{Mapping, Value};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MarkdownDoc {
    pub frontmatter: Mapping,
    pub body: String,
}

impl MarkdownDoc {
    /// A frontmatter value rendered as text, if it is a scalar.
    pub fn get(&self, key: &str) -> Option<String> {
        match self.frontmatter.get(&Value::String(key.to_owned()))? {
            Value::String(value) => Some(value.clone()),
            Value::Bool(value) => Some(value.to_string()),
            Value::Number(value) => Some(value.to_string()),
            Value::Sequence(values) => Some(
                values
                    .iter()
                    .filter_map(|value| match value {
                        Value::String(value) => Some(value.clone()),
                        Value::Number(value) => Some(value.to_string()),
                        Value::Bool(value) => Some(value.to_string()),
                        Value::Null | Value::Sequence(_) | Value::Mapping(_) => None,
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            Value::Null | Value::Mapping(_) => None,
        }
    }

    /// Sets a string frontmatter value, or removes the key when `value` is blank. Keys that
    /// already exist keep their position.
    pub fn set_string(&mut self, key: &str, value: &str) {
        let key = Value::String(key.to_owned());
        let value = value.trim();
        if value.is_empty() {
            // `Mapping::remove` swap-removes, which would reorder the remaining keys.
            self.frontmatter = self
                .frontmatter
                .iter()
                .filter(|(existing, _)| **existing != key)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
        } else {
            self.frontmatter.insert(key, Value::String(value.to_owned()));
        }
    }

    /// Like [`Self::set_string`], but keeps a non-string existing value (for example a YAML
    /// list of tools) untouched when its textual rendering did not change.
    pub fn set_string_if_changed(&mut self, key: &str, value: &str) {
        if self.get(key).as_deref().map(str::trim) == Some(value.trim()) {
            return;
        }
        self.set_string(key, value);
    }
}

/// Parses a markdown document. A missing frontmatter block yields an empty mapping and the
/// whole content as the body; a malformed one is an error.
pub fn parse_markdown(content: &str) -> Result<MarkdownDoc, String> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let Some((yaml, body)) = split_frontmatter(content) else {
        return Ok(MarkdownDoc {
            frontmatter: Mapping::new(),
            body: content.to_owned(),
        });
    };
    let frontmatter = if yaml.trim().is_empty() {
        Mapping::new()
    } else {
        match serde_yaml::from_str::<Value>(yaml) {
            Ok(Value::Mapping(mapping)) => mapping,
            Ok(Value::Null) => Mapping::new(),
            Ok(_) => return Err("El frontmatter YAML no es un mapa clave: valor".to_owned()),
            Err(err) => return Err(format!("Frontmatter YAML inválido: {err}")),
        }
    };
    Ok(MarkdownDoc {
        frontmatter,
        body: body.to_owned(),
    })
}

/// Renders a document back to text, always with a frontmatter block.
pub fn render_markdown(doc: &MarkdownDoc) -> Result<String, String> {
    let mut yaml = if doc.frontmatter.is_empty() {
        String::new()
    } else {
        serde_yaml::to_string(&doc.frontmatter)
            .map_err(|err| format!("No se pudo serializar el frontmatter: {err}"))?
    };
    // serde_yaml 0.8 prefixes documents with a `---` marker; 0.9 does not.
    if let Some(stripped) = yaml.strip_prefix("---\n") {
        yaml = stripped.to_owned();
    } else if yaml.trim() == "---" {
        yaml.clear();
    }
    if !yaml.is_empty() && !yaml.ends_with('\n') {
        yaml.push('\n');
    }
    let body = doc.body.trim_start_matches(['\r', '\n']);
    let mut out = format!("---\n{yaml}---\n\n{body}");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

/// Returns `(yaml, body)` when `content` opens with a `---` line closed by another `---` line.
fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let first_end = content.find('\n')?;
    if content[..first_end].trim_end() != "---" {
        return None;
    }
    let rest = &content[first_end + 1..];
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            let yaml = &rest[..offset];
            let body = &rest[offset + line.len()..];
            return Some((yaml, body));
        }
        offset += line.len();
    }
    None
}

#[cfg(test)]
#[path = "frontmatter_tests.rs"]
mod tests;
