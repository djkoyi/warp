//! Masking of secret-bearing values (MCP `env`/`headers`/`args`/`url`, settings `env`) for
//! display and for editing without ever revealing the stored values.

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

/// Flag or variable names whose value is treated as a secret (`--api-key`, `GITHUB_TOKEN=`…).
fn is_secret_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    ["token", "key", "secret", "password", "auth"]
        .iter()
        .any(|needle| name.contains(needle))
}

/// Masks the userinfo (`user:pass@`) and every query value of a URL.
pub fn mask_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let (rest, fragment) = match rest.split_once('#') {
        Some((rest, fragment)) => (rest, Some(fragment)),
        None => (rest, None),
    };
    let (before_query, query) = match rest.split_once('?') {
        Some((before, query)) => (before, Some(query)),
        None => (rest, None),
    };
    let authority_end = before_query.find('/').unwrap_or(before_query.len());
    let (authority, path) = before_query.split_at(authority_end);
    let authority = match authority.rsplit_once('@') {
        Some((_, host)) => format!("{MASK}@{host}"),
        None => authority.to_owned(),
    };
    let mut masked = format!("{scheme}://{authority}{path}");
    if let Some(query) = query {
        let params = query
            .split('&')
            .map(|param| match param.split_once('=') {
                Some((name, value)) if !value.is_empty() => format!("{name}={MASK}"),
                Some(_) | None => param.to_owned(),
            })
            .collect::<Vec<_>>()
            .join("&");
        masked.push('?');
        masked.push_str(&params);
    }
    if let Some(fragment) = fragment {
        masked.push('#');
        masked.push_str(fragment);
    }
    masked
}

/// Masks command-line arguments that likely carry secrets: the value after a flag whose name
/// matches token/key/secret/password/auth (`--token X` or `--token=X`), `NAME=value` pairs with
/// such a name, and URLs (see [`mask_url`]).
pub fn mask_args(args: &[String]) -> Vec<String> {
    let mut masked = Vec::with_capacity(args.len());
    let mut mask_next = false;
    for arg in args {
        let is_flag = arg.starts_with('-');
        if std::mem::take(&mut mask_next) && !is_flag && !arg.is_empty() {
            masked.push(MASK.to_owned());
            continue;
        }
        let masked_arg = match arg.split_once('=') {
            Some((name, value)) if is_secret_name(name) && !value.is_empty() => {
                format!("{name}={MASK}")
            }
            Some(_) | None if is_flag && !arg.contains('=') && is_secret_name(arg) => {
                mask_next = true;
                arg.clone()
            }
            Some(_) | None if arg.contains("://") => mask_url(arg),
            Some(_) | None => arg.clone(),
        };
        masked.push(masked_arg);
    }
    masked
}

/// Parses the edited arguments (one per line). A line equal to a masked original argument
/// (preferably at the same position) keeps the stored value; any other line still containing
/// [`MASK`] is an error.
pub fn merge_masked_args(original: &[String], edited: &str) -> Result<Vec<String>, String> {
    let masked = mask_args(original);
    let mut used = vec![false; original.len()];
    let mut result = Vec::new();
    for (index, line) in edited
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .enumerate()
    {
        if !line.contains(MASK) {
            result.push(line.to_owned());
            continue;
        }
        let source = if masked.get(index).map(String::as_str) == Some(line) && !used[index] {
            Some(index)
        } else {
            (0..masked.len()).find(|&candidate| !used[candidate] && masked[candidate] == line)
        };
        let Some(source) = source else {
            return Err(format!(
                "Argumento {}: contiene {MASK} pero no corresponde a un valor guardado; escribe \
                 el valor completo",
                index + 1
            ));
        };
        used[source] = true;
        result.push(original[source].clone());
    }
    Ok(result)
}

/// The URL to store for an edited URL field: the stored URL when the field still shows its
/// masked form, the edited text otherwise.
pub fn merge_masked_url(original: Option<&str>, edited: &str) -> Result<String, String> {
    let edited = edited.trim();
    if !edited.contains(MASK) {
        return Ok(edited.to_owned());
    }
    match original {
        Some(original) if mask_url(original) == edited => Ok(original.to_owned()),
        Some(_) | None => Err(format!(
            "La URL contiene {MASK}: escríbela completa (con sus credenciales) para cambiarla"
        )),
    }
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
