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
    assert_eq!(
        summarize_value("permissions", &json!({"allow": []})),
        "{allow}"
    );
}

#[test]
fn urls_hide_userinfo_and_query_values() {
    assert_eq!(
        mask_url("https://user:pw@host.io:8080/p/a?x=1&flag&k=#frag"),
        "https://••••••@host.io:8080/p/a?x=••••••&flag&k=#frag"
    );
    assert_eq!(mask_url("https://host.io/mcp"), "https://host.io/mcp");
    assert_eq!(mask_url("not a url"), "not a url");
}

#[test]
fn args_hide_values_of_secret_flags() {
    let args: Vec<String> = [
        "-y",
        "--API-KEY",
        "abc",
        "--auth-token=def",
        "--verbose",
        "--password",
        "--next-flag",
        "GITHUB_TOKEN=ghp",
        "https://u:p@h",
        "plain",
    ]
    .map(str::to_owned)
    .to_vec();
    assert_eq!(
        mask_args(&args),
        vec![
            "-y",
            "--API-KEY",
            MASK,
            "--auth-token=••••••",
            "--verbose",
            "--password",
            "--next-flag",
            "GITHUB_TOKEN=••••••",
            "https://••••••@h",
            "plain",
        ]
    );
}

#[test]
fn merging_masked_args_and_urls_keeps_stored_values() {
    let original: Vec<String> = ["--token", "t1", "--key=k2"].map(str::to_owned).to_vec();
    let edited = mask_args(&original).join("\n") + "\n--extra";
    assert_eq!(
        merge_masked_args(&original, &edited).unwrap(),
        vec!["--token", "t1", "--key=k2", "--extra"]
    );
    // Reordered lines still resolve to their own stored values.
    assert_eq!(
        merge_masked_args(&original, "--key=••••••\n--token\n••••••").unwrap(),
        vec!["--key=k2", "--token", "t1"]
    );
    assert!(merge_masked_args(&original, "--other=••••••").is_err());

    let url = "https://u:p@h/x?q=1";
    assert_eq!(merge_masked_url(Some(url), &mask_url(url)).unwrap(), url);
    assert_eq!(
        merge_masked_url(Some(url), " https://new ").unwrap(),
        "https://new"
    );
    assert!(merge_masked_url(Some(url), "https://••••••@other").is_err());
    assert!(merge_masked_url(None, "https://••••••@h").is_err());
}
