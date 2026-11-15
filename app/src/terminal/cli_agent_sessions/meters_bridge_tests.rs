use serde_json::json;

use super::*;

const STATUS: &str = "python3 -I '/h/.claude/warp/warp_claude_meters.py' statusline";
const HOOK: &str = "python3 -I '/h/.claude/warp/warp_claude_meters.py' hook";

fn bridge_hook_count(settings: &Value, event: &str) -> usize {
    settings["hooks"][event]
        .as_array()
        .map(|matchers| {
            matchers
                .iter()
                .filter(|m| m["hooks"][0]["command"].as_str() == Some(HOOK))
                .count()
        })
        .unwrap_or_default()
}

#[test]
fn installs_into_empty_settings() {
    let mut settings = json!({});
    let outcome = merge_settings(&mut settings, STATUS, HOOK).unwrap();

    assert!(outcome.changed);
    assert_eq!(outcome.previous_status_line, None);
    assert_eq!(settings["statusLine"]["type"], "command");
    assert_eq!(settings["statusLine"]["command"], STATUS);
    for event in HOOK_EVENTS {
        assert_eq!(bridge_hook_count(&settings, event), 1, "{event}");
    }
}

#[test]
fn merge_is_idempotent() {
    let mut settings = json!({});
    merge_settings(&mut settings, STATUS, HOOK).unwrap();
    let after_first = settings.clone();

    let outcome = merge_settings(&mut settings, STATUS, HOOK).unwrap();
    assert!(!outcome.changed);
    assert_eq!(settings, after_first);
}

#[test]
fn existing_status_line_is_chained_and_unrelated_settings_kept() {
    let mut settings = json!({
        "model": "opus",
        "statusLine": {"type": "command", "command": "~/.claude/statusline.sh", "padding": 2},
        "hooks": {
            "Stop": [{"hooks": [{"type": "command", "command": "say done"}]}],
            "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "guard"}]}]
        }
    });
    let outcome = merge_settings(&mut settings, STATUS, HOOK).unwrap();

    assert_eq!(
        outcome.previous_status_line.as_deref(),
        Some("~/.claude/statusline.sh")
    );
    assert_eq!(settings["model"], "opus");
    assert_eq!(settings["statusLine"]["command"], STATUS);
    assert_eq!(settings["statusLine"]["padding"], 2);
    assert_eq!(
        settings["hooks"]["Stop"][0]["hooks"][0]["command"],
        "say done"
    );
    assert_eq!(bridge_hook_count(&settings, "Stop"), 1);
    assert_eq!(
        settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
        "guard"
    );
}

#[test]
fn rejects_unexpected_shapes_instead_of_overwriting() {
    assert!(merge_settings(&mut json!([]), STATUS, HOOK).is_err());
    assert!(merge_settings(&mut json!({"hooks": []}), STATUS, HOOK).is_err());
    assert!(merge_settings(&mut json!({"hooks": {"Stop": {}}}), STATUS, HOOK).is_err());
}

#[test]
fn shell_quote_escapes_single_quotes() {
    assert_eq!(shell_quote("/a b/c"), "'/a b/c'");
    assert_eq!(shell_quote("/it's"), r"'/it'\''s'");
}

#[test]
fn bundled_script_has_both_modes() {
    assert!(BRIDGE_SCRIPT.contains("def run_statusline"));
    assert!(BRIDGE_SCRIPT.contains("def run_hook"));
}
