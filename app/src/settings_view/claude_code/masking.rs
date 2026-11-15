//! Masking of secret-bearing values (MCP `env`/`headers`, settings `env`) for display and for
//! editing as `KEY=VALUE` text without ever revealing the stored values.

use serde_json::{Map, Value};

/// Placeholder shown instead of a stored value. Leaving it untouched in an edited line keeps the
/// stored value.
pub const MASK: &str = "••••••";

/// Renders a JSON object of string values as `KEY=••••••` lines.
pub fn masked_kv_text(map: Option<&Map<String, Value>>) -> String {
    map.map(|map| {
        map.keys()
            .map(|key| format!("{key}={MASK}"))
            .collect::<Vec<_>>()
            .join("\n")
    })
    .unwrap_or_default()
}

/// Parses edited `KEY=VALUE` lines back into an object. A value equal to [`MASK`] keeps the
/// value stored under the same key in `original`. Blank lines and `#` comments are ignored.
pub fn merge_masked_kv(
    original: Option<&Map<String, Value>>,
    edited: &str,
) -> Result<Map<String, Value>, String> {
    let mut result = Map::new();
    for (index, line) in edited.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("Línea {}: se esperaba CLAVE=VALOR", index + 1));
        };
        let key = key.trim();
        if key.is_empty() || key.chars().any(char::is_whitespace) {
            return Err(format!("Línea {}: clave inválida", index + 1));
        }
        let value = value.trim();
        let value = if value == MASK {
            match original.and_then(|original| original.get(key)) {
                Some(existing) => existing.clone(),
                None => {
                    return Err(format!(
                        "Línea {}: \"{key}\" no tenía valor guardado; escribe el valor",
                        index + 1
                    ));
                }
            }
        } else {
            Value::String(value.to_owned())
        };
        result.insert(key.to_owned(), value);
    }
    Ok(result)
}

/// A short, secret-free summary of a settings value for attribution lists.
pub fn summarize_value(key: &str, value: &Value) -> String {
    if is_secret_key(key) {
        return MASK.to_owned();
    }
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => truncate(value, 80),
        Value::Array(values) => format!("[{} elementos]", values.len()),
        Value::Object(map) => {
            let keys = map.keys().take(6).cloned().collect::<Vec<_>>().join(", ");
            if map.len() > 6 {
                format!("{{{keys}, …}}")
            } else {
                format!("{{{keys}}}")
            }
        }
    }
}

/// Keys whose values must never be displayed.
pub fn is_secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key == "env"
        || key == "headers"
        || ["token", "secret", "password", "apikey", "api_key", "auth"]
            .iter()
            .any(|needle| key.contains(needle))
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_owned()
    } else {
        let mut truncated: String = text.chars().take(max_chars).collect();
        truncated.push('…');
        truncated
    }
}

#[cfg(test)]
#[path = "masking_tests.rs"]
mod tests;
