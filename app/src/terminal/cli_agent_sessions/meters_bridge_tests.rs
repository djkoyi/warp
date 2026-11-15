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
    let outcome = merge_settings(&mut settings, STATUS, HOOK, true).unwrap();

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
    merge_settings(&mut settings, STATUS, HOOK, true).unwrap();
    let after_first = settings.clone();

    let outcome = merge_settings(&mut settings, STATUS, HOOK, true).unwrap();
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
    let outcome = merge_settings(&mut settings, STATUS, HOOK, true).unwrap();

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
    assert!(merge_settings(&mut json!([]), STATUS, HOOK, true).is_err());
    assert!(merge_settings(&mut json!({"hooks": []}), STATUS, HOOK, true).is_err());
    assert!(merge_settings(&mut json!({"hooks": {"Stop": {}}}), STATUS, HOOK, true).is_err());
}

#[test]
fn post_compact_is_only_added_when_supported() {
    let mut settings = json!({});
    merge_settings(&mut settings, STATUS, HOOK, false).unwrap();
    assert_eq!(bridge_hook_count(&settings, "PostCompact"), 0);
    assert_eq!(bridge_hook_count(&settings, "PreCompact"), 1);

    let outcome = merge_settings(&mut settings, STATUS, HOOK, true).unwrap();
    assert!(outcome.changed);
    assert_eq!(bridge_hook_count(&settings, "PostCompact"), 1);
}

#[test]
fn parses_claude_version_output() {
    assert_eq!(
        parse_claude_version("2.1.76 (Claude Code)\n"),
        Some((2, 1, 76))
    );
    assert_eq!(parse_claude_version("garbage"), None);
    assert!(parse_claude_version("2.1.75 (Claude Code)").unwrap() < POST_COMPACT_MIN_VERSION);
    assert!(parse_claude_version("2.2.0").unwrap() >= POST_COMPACT_MIN_VERSION);
}

#[test]
fn write_atomically_follows_through_to_the_target() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("settings.json");
    fs::write(&target, "old").unwrap();
    write_atomically(&target, b"new", None).unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), "new");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1, "no temp file left");
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
