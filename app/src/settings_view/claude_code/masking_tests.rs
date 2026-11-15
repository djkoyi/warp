use serde_json::json;

use super::*;

fn env() -> Map<String, Value> {
    json!({"API_KEY": "sk-secret", "REGION": "us"})
        .as_object()
        .unwrap()
        .clone()
}

#[test]
fn masked_text_never_contains_values() {
    let text = masked_kv_text(Some(&env()));
    assert!(!text.contains("sk-secret"));
    assert!(text.contains(&format!("API_KEY={MASK}")));
    assert!(text.contains(&format!("REGION={MASK}")));
}

#[test]
fn merge_keeps_masked_values_and_applies_edits() {
    let original = env();
    let edited = format!("API_KEY={MASK}\nREGION=eu\n\n# comment\nNEW=1");
    let merged = merge_masked_kv(Some(&original), &edited).unwrap();
    assert_eq!(merged["API_KEY"], json!("sk-secret"));
    assert_eq!(merged["REGION"], json!("eu"));
    assert_eq!(merged["NEW"], json!("1"));
    assert_eq!(merged.len(), 3);
}

#[test]
fn merge_drops_removed_lines() {
    let merged = merge_masked_kv(Some(&env()), &format!("REGION={MASK}")).unwrap();
    assert!(!merged.contains_key("API_KEY"));
}

#[test]
fn merge_rejects_bad_lines_and_unknown_masks() {
    assert!(merge_masked_kv(None, "novalue").is_err());
    assert!(merge_masked_kv(None, "BAD KEY=1").is_err());
    assert!(merge_masked_kv(None, &format!("X={MASK}")).is_err());
}

#[test]
fn summaries_hide_secrets() {
    assert_eq!(summarize_value("env", &json!({"A": "b"})), MASK);
    assert_eq!(summarize_value("apiKeyHelper", &json!("x")), MASK);
    assert_eq!(summarize_value("model", &json!("opus")), "opus");
    assert_eq!(summarize_value("permissions", &json!({"allow": []})), "{allow}");
}
