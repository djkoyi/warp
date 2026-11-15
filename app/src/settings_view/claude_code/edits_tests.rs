use std::fs;

use serde_json::json;
use tempfile::TempDir;

use super::*;
use crate::settings_view::claude_code::snapshot::{McpState, load_snapshot};

struct Fixture {
    _home: TempDir,
    repo: PathBuf,
    paths: ClaudePaths,
}

fn fixture() -> Fixture {
    let home = TempDir::new().unwrap();
    let repo = home.path().join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let paths = ClaudePaths::resolve(
        |_| None,
        Some(home.path().to_path_buf()),
        Some(repo.clone()),
    )
    .unwrap();
    fs::create_dir_all(&paths.config_dir).unwrap();
    Fixture {
        _home: home,
        repo,
        paths,
    }
}

fn write_json(path: &Path, value: Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn backup_count(paths: &ClaudePaths) -> usize {
    fs::read_dir(paths.backups_dir())
        .map(|dir| dir.count())
        .unwrap_or(0)
}

fn obj(value: Value) -> Map<String, Value> {
    value.as_object().unwrap().clone()
}

#[test]
fn create_edit_and_delete_skill() {
    let fx = fixture();
    let create = ClaudeEdit::SaveMarkdown {
        kind: MdKind::Skill,
        project: false,
        original_path: None,
        name: "deploy".into(),
        fields: vec![("description".into(), "Ship it".into())],
        body: "Steps\n".into(),
    };
    apply_edit(&fx.paths, create.clone()).unwrap();
    let path = fx.paths.config_dir.join("skills/deploy/SKILL.md");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "---\nname: deploy\ndescription: Ship it\n---\n\nSteps\n"
    );
    assert!(apply_edit(&fx.paths, create).is_err(), "duplicate create");

    fs::write(
        &path,
        "---\nname: deploy\nallowed-tools: Bash(git:*)\ndescription: Ship it\n---\nSteps\n",
    )
    .unwrap();
    apply_edit(
        &fx.paths,
        ClaudeEdit::SaveMarkdown {
            kind: MdKind::Skill,
            project: false,
            original_path: Some(path.clone()),
            name: "deploy".into(),
            fields: vec![("description".into(), "Ship faster".into())],
            body: "New steps\n".into(),
        },
    )
    .unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("allowed-tools: Bash(git:*)"));
    assert!(text.contains("description: Ship faster"));
    assert!(text.ends_with("New steps\n"));
    assert_eq!(backup_count(&fx.paths), 1);

    apply_edit(
        &fx.paths,
        ClaudeEdit::DeleteItem {
            path: path.parent().unwrap().to_path_buf(),
        },
    )
    .unwrap();
    assert!(!path.exists());
    assert_eq!(backup_count(&fx.paths), 2);
}

#[test]
fn skill_requires_valid_name_and_description() {
    let fx = fixture();
    let bad_name = ClaudeEdit::SaveMarkdown {
        kind: MdKind::Skill,
        project: false,
        original_path: None,
        name: "../escape".into(),
        fields: vec![("description".into(), "x".into())],
        body: String::new(),
    };
    assert!(apply_edit(&fx.paths, bad_name).is_err());
    let no_description = ClaudeEdit::SaveMarkdown {
        kind: MdKind::Skill,
        project: false,
        original_path: None,
        name: "ok".into(),
        fields: vec![],
        body: String::new(),
    };
    assert!(apply_edit(&fx.paths, no_description).is_err());
}

#[test]
fn project_command_and_agent() {
    let fx = fixture();
    apply_edit(
        &fx.paths,
        ClaudeEdit::SaveMarkdown {
            kind: MdKind::Command,
            project: true,
            original_path: None,
            name: "review".into(),
            fields: vec![
                ("description".into(), "Review PR".into()),
                ("argument-hint".into(), "[pr]".into()),
            ],
            body: "Review $ARGUMENTS\n".into(),
        },
    )
    .unwrap();
    let command = fx.repo.join(".claude/commands/review.md");
    let text = fs::read_to_string(&command).unwrap();
    assert!(!text.contains("name:"));
    assert!(text.contains("argument-hint: \"[pr]\"") || text.contains("argument-hint: '[pr]'"));

    apply_edit(
        &fx.paths,
        ClaudeEdit::SaveMarkdown {
            kind: MdKind::Agent,
            project: false,
            original_path: None,
            name: "reviewer".into(),
            fields: vec![("description".into(), "Reviews".into())],
            body: "You review.\n".into(),
        },
    )
    .unwrap();
    assert!(fx.paths.config_dir.join("agents/reviewer.md").exists());
}

#[test]
fn edits_outside_claude_dirs_are_rejected() {
    let fx = fixture();
    let outside = fx.repo.join("notes.md");
    fs::write(&outside, "x").unwrap();
    assert!(
        apply_edit(
            &fx.paths,
            ClaudeEdit::DeleteItem {
                path: outside.clone()
            }
        )
        .is_err()
    );
    assert!(outside.exists());
}

#[test]
fn mcp_upsert_rename_and_delete_preserve_claude_json() {
    let fx = fixture();
    write_json(
        &fx.paths.claude_json,
        json!({"numStartups": 3, "oauthAccount": {"x": 1}, "mcpServers": {"old": {"command": "a", "env": {"K": "secret"}}}}),
    );

    let original = obj(json!({"command": "a", "env": {"K": "secret"}, "timeout": 5}));
    let entry = build_mcp_entry(
        Some(&original),
        "stdio",
        "npx",
        "-y\n@scope/server\n",
        "",
        obj(json!({"K": "secret"})),
        Map::new(),
    )
    .unwrap();
    apply_edit(
        &fx.paths,
        ClaudeEdit::UpsertMcpServer {
            scope: McpScope::User,
            original_name: Some("old".into()),
            name: "renamed".into(),
            entry,
        },
    )
    .unwrap();
    let value = read_json(&fx.paths.claude_json);
    assert_eq!(value["numStartups"], json!(3));
    assert_eq!(value["oauthAccount"], json!({"x": 1}));
    assert!(value["mcpServers"].get("old").is_none());
    assert_eq!(
        value["mcpServers"]["renamed"]["args"],
        json!(["-y", "@scope/server"])
    );
    assert_eq!(value["mcpServers"]["renamed"]["timeout"], json!(5));
    assert_eq!(value["mcpServers"]["renamed"]["env"]["K"], json!("secret"));

    apply_edit(
        &fx.paths,
        ClaudeEdit::DeleteMcpServer {
            scope: McpScope::User,
            name: "renamed".into(),
        },
    )
    .unwrap();
    assert!(
        read_json(&fx.paths.claude_json)["mcpServers"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    assert!(
        apply_edit(
            &fx.paths,
            ClaudeEdit::DeleteMcpServer {
                scope: McpScope::User,
                name: "renamed".into(),
            },
        )
        .is_err()
    );
}

#[test]
fn mcp_local_and_project_scopes() {
    let fx = fixture();
    let key = fx.repo.to_string_lossy().into_owned();
    let http = build_mcp_entry(
        None,
        "http",
        "",
        "",
        "https://mcp.example.com",
        Map::new(),
        obj(json!({"Authorization": "Bearer x"})),
    )
    .unwrap();
    apply_edit(
        &fx.paths,
        ClaudeEdit::UpsertMcpServer {
            scope: McpScope::Local(key.clone()),
            original_name: None,
            name: "remote".into(),
            entry: http.clone(),
        },
    )
    .unwrap();
    apply_edit(
        &fx.paths,
        ClaudeEdit::UpsertMcpServer {
            scope: McpScope::Project,
            original_name: None,
            name: "shared".into(),
            entry: http,
        },
    )
    .unwrap();
    let claude_json = read_json(&fx.paths.claude_json);
    assert_eq!(
        claude_json["projects"][key.as_str()]["mcpServers"]["remote"]["type"],
        json!("http")
    );
    let mcp_json = read_json(&fx.repo.join(".mcp.json"));
    assert_eq!(
        mcp_json["mcpServers"]["shared"]["url"],
        json!("https://mcp.example.com")
    );

    let duplicate = ClaudeEdit::UpsertMcpServer {
        scope: McpScope::Project,
        original_name: None,
        name: "shared".into(),
        entry: Map::new(),
    };
    assert!(apply_edit(&fx.paths, duplicate).is_err());
}

#[test]
fn build_mcp_entry_validates() {
    assert!(build_mcp_entry(None, "stdio", " ", "", "", Map::new(), Map::new()).is_err());
    assert!(build_mcp_entry(None, "http", "", "", "ftp://x", Map::new(), Map::new()).is_err());
    assert!(build_mcp_entry(None, "ws", "", "", "", Map::new(), Map::new()).is_err());
    let switched = build_mcp_entry(
        Some(&obj(
            json!({"command": "a", "args": ["b"], "env": {"K": "v"}}),
        )),
        "sse",
        "",
        "",
        "https://x",
        Map::new(),
        Map::new(),
    )
    .unwrap();
    assert!(switched.get("command").is_none());
    assert!(switched.get("env").is_none());
    assert_eq!(switched["type"], json!("sse"));
}

#[test]
fn mcpjson_approval_and_project_disable() {
    let fx = fixture();
    let key = fx.repo.to_string_lossy().into_owned();
    write_json(
        &fx.repo.join(".mcp.json"),
        json!({"mcpServers": {"tool": {"command": "t"}}}),
    );
    write_json(
        &fx.paths.config_dir.join("settings.json"),
        json!({"model": "opus", "disabledMcpjsonServers": ["tool"]}),
    );
    apply_edit(
        &fx.paths,
        ClaudeEdit::SetMcpjsonServerEnabled {
            name: "tool".into(),
            enabled: true,
        },
    )
    .unwrap();
    let user = read_json(&fx.paths.config_dir.join("settings.json"));
    assert_eq!(user["model"], json!("opus"));
    assert_eq!(user["disabledMcpjsonServers"], json!([]));
    let snapshot = load_snapshot(&fx.paths);
    assert_eq!(snapshot.mcp_servers[0].state, McpState::Enabled);

    apply_edit(
        &fx.paths,
        ClaudeEdit::SetMcpjsonServerEnabled {
            name: "tool".into(),
            enabled: false,
        },
    )
    .unwrap();
    let local = read_json(&fx.repo.join(".claude/settings.local.json"));
    assert_eq!(local["enabledMcpjsonServers"], json!([]));
    assert_eq!(local["disabledMcpjsonServers"], json!(["tool"]));

    apply_edit(
        &fx.paths,
        ClaudeEdit::SetMcpServerDisabled {
            project_key: key.clone(),
            name: "github".into(),
            disabled: true,
        },
    )
    .unwrap();
    let claude_json = read_json(&fx.paths.claude_json);
    assert_eq!(
        claude_json["projects"][key.as_str()]["disabledMcpServers"],
        json!(["github"])
    );
}

#[test]
fn plugin_and_skill_toggles() {
    let fx = fixture();
    write_json(
        &fx.paths.config_dir.join("settings.json"),
        json!({"enabledPlugins": {"a@m": true}, "theme": "dark"}),
    );
    apply_edit(
        &fx.paths,
        ClaudeEdit::SetPluginEnabled {
            scope: SettingsScope::User,
            id: "a@m".into(),
            enabled: false,
        },
    )
    .unwrap();
    apply_edit(
        &fx.paths,
        ClaudeEdit::SetSkillOverride {
            name: "deploy".into(),
            off: true,
            default_scope: SettingsScope::User,
        },
    )
    .unwrap();
    let user = read_json(&fx.paths.config_dir.join("settings.json"));
    assert_eq!(user["enabledPlugins"]["a@m"], json!(false));
    assert_eq!(user["skillOverrides"]["deploy"], json!("off"));
    assert_eq!(user["theme"], json!("dark"));

    apply_edit(
        &fx.paths,
        ClaudeEdit::SetSkillOverride {
            name: "deploy".into(),
            off: false,
            default_scope: SettingsScope::User,
        },
    )
    .unwrap();
    let user = read_json(&fx.paths.config_dir.join("settings.json"));
    assert!(user.get("skillOverrides").is_none());

    assert!(
        apply_edit(
            &fx.paths,
            ClaudeEdit::SetPluginEnabled {
                scope: SettingsScope::Managed,
                id: "a@m".into(),
                enabled: true,
            },
        )
        .is_err()
    );
}

#[test]
fn plugin_toggle_scope_prefers_local_when_project_mentions_it() {
    let fx = fixture();
    assert_eq!(plugin_toggle_scope(&fx.paths, &[]), SettingsScope::User);
    assert_eq!(
        plugin_toggle_scope(&fx.paths, &[SettingsScope::User, SettingsScope::Project]),
        SettingsScope::Local
    );
}

#[test]
fn hooks_add_edit_move_and_remove() {
    let fx = fixture();
    let settings = fx.paths.config_dir.join("settings.json");
    write_json(&settings, json!({"model": "opus"}));

    let add = |command: &str, matcher: Option<&str>| ClaudeEdit::UpsertHook {
        scope: SettingsScope::User,
        original: None,
        hook: HookSpec {
            event: "PreToolUse".into(),
            matcher: matcher.map(str::to_owned),
            command: command.into(),
            timeout: None,
        },
    };
    apply_edit(&fx.paths, add("one", Some("Bash"))).unwrap();
    apply_edit(&fx.paths, add("two", Some("Bash"))).unwrap();
    apply_edit(&fx.paths, add("three", None)).unwrap();
    let value = read_json(&settings);
    assert_eq!(value["model"], json!("opus"));
    let groups = value["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0]["hooks"].as_array().unwrap().len(), 2);
    assert!(groups[1].get("matcher").is_none());

    apply_edit(
        &fx.paths,
        ClaudeEdit::UpsertHook {
            scope: SettingsScope::User,
            original: Some(HookLocation {
                event: "PreToolUse".into(),
                group_index: 0,
                hook_index: 1,
                expected_command: Some("two".into()),
            }),
            hook: HookSpec {
                event: "PreToolUse".into(),
                matcher: Some("Bash".into()),
                command: "two-edited".into(),
                timeout: Some(10),
            },
        },
    )
    .unwrap();
    let value = read_json(&settings);
    assert_eq!(
        value["hooks"]["PreToolUse"][0]["hooks"][1],
        json!({"type": "command", "command": "two-edited", "timeout": 10})
    );

    let stale = ClaudeEdit::RemoveHook {
        scope: SettingsScope::User,
        location: HookLocation {
            event: "PreToolUse".into(),
            group_index: 0,
            hook_index: 0,
            expected_command: Some("not-there".into()),
        },
    };
    assert!(apply_edit(&fx.paths, stale).is_err());

    apply_edit(
        &fx.paths,
        ClaudeEdit::UpsertHook {
            scope: SettingsScope::User,
            original: Some(HookLocation {
                event: "PreToolUse".into(),
                group_index: 1,
                hook_index: 0,
                expected_command: Some("three".into()),
            }),
            hook: HookSpec {
                event: "Stop".into(),
                matcher: None,
                command: "three".into(),
                timeout: None,
            },
        },
    )
    .unwrap();
    let value = read_json(&settings);
    assert_eq!(value["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
    assert_eq!(
        value["hooks"]["Stop"][0]["hooks"][0]["command"],
        json!("three")
    );

    for (event, command, hook_index) in [
        ("Stop", "three", 0),
        ("PreToolUse", "one", 0),
        ("PreToolUse", "two-edited", 0),
    ] {
        apply_edit(
            &fx.paths,
            ClaudeEdit::RemoveHook {
                scope: SettingsScope::User,
                location: HookLocation {
                    event: event.into(),
                    group_index: 0,
                    hook_index,
                    expected_command: Some(command.into()),
                },
            },
        )
        .unwrap();
    }
    let value = read_json(&settings);
    assert!(value.get("hooks").is_none());
    assert_eq!(value["model"], json!("opus"));
}

#[test]
fn common_settings_round_trip() {
    let fx = fixture();
    let settings = fx.paths.config_dir.join("settings.json");
    write_json(
        &settings,
        json!({
            "model": "sonnet",
            "statusLine": {"type": "command", "command": "old", "padding": 1},
            "permissions": {"allow": ["Bash(ls)"], "additionalDirectories": ["/x"]},
            "cleanupPeriodDays": 9
        }),
    );
    apply_edit(
        &fx.paths,
        ClaudeEdit::SetCommonSettings {
            scope: SettingsScope::User,
            update: CommonSettingsUpdate {
                model: Some("opus".into()),
                default_mode: Some("plan".into()),
                output_style: Some(String::new()),
                status_line_command: Some("~/.claude/status.sh".into()),
                allow: ListDiff {
                    add: vec![],
                    remove: vec!["Bash(ls)".into()],
                },
                deny: ListDiff {
                    add: vec!["Read(.env)".into()],
                    remove: vec![],
                },
                ask: ListDiff::default(),
                env_set: obj(json!({"FOO": "bar"})),
                env_remove: vec![],
            },
        },
    )
    .unwrap();
    let value = read_json(&settings);
    assert_eq!(value["model"], json!("opus"));
    assert_eq!(value["cleanupPeriodDays"], json!(9));
    assert_eq!(value["statusLine"]["padding"], json!(1));
    assert_eq!(value["statusLine"]["command"], json!("~/.claude/status.sh"));
    assert_eq!(value["permissions"]["defaultMode"], json!("plan"));
    assert_eq!(value["permissions"]["deny"], json!(["Read(.env)"]));
    assert!(value["permissions"].get("allow").is_none());
    assert_eq!(value["permissions"]["additionalDirectories"], json!(["/x"]));
    assert_eq!(value["env"]["FOO"], json!("bar"));

    let bad = ClaudeEdit::SetCommonSettings {
        scope: SettingsScope::User,
        update: CommonSettingsUpdate {
            default_mode: Some("rm -rf".into()),
            ..Default::default()
        },
    };
    assert!(apply_edit(&fx.paths, bad).is_err());
}

#[test]
fn needs_reload_only_for_startup_loaded_sources() {
    assert!(
        ClaudeEdit::DeleteMcpServer {
            scope: McpScope::User,
            name: "x".into()
        }
        .needs_claude_reload()
    );
    assert!(
        !ClaudeEdit::SetCommonSettings {
            scope: SettingsScope::User,
            update: CommonSettingsUpdate::default(),
        }
        .needs_claude_reload()
    );
}

#[test]
fn delete_rejects_parent_dir_and_wrong_shapes() {
    let fx = fixture();
    let config = fx.paths.config_dir.clone();
    fs::create_dir_all(config.join("skills/deploy")).unwrap();
    fs::write(
        config.join("skills/deploy/SKILL.md"),
        "---\nname: deploy\n---\n",
    )
    .unwrap();
    fs::create_dir_all(config.join("agents")).unwrap();
    fs::write(config.join("settings.json"), "{}").unwrap();

    for path in [
        config.join("skills/.."),
        config.join("skills/deploy/.."),
        config.join("skills/../settings.json"),
        config.join("agents/../settings.json"),
        config.join("skills"),
        config.join("skills/deploy/SKILL.md"),
        config.join("agents"),
        config.join("agents/notes.txt"),
        fx.repo.join(".claude/skills/.."),
    ] {
        let result = apply_edit(&fx.paths, ClaudeEdit::DeleteItem { path: path.clone() });
        assert!(result.is_err(), "{} should be rejected", path.display());
    }
    assert!(config.join("settings.json").exists());
    assert!(config.join("skills/deploy/SKILL.md").exists());

    // Editing only accepts `<root>/<name>/SKILL.md` for skills.
    fs::write(config.join("skills/deploy/notes.md"), "---\nname: x\n---\n").unwrap();
    let edit_other = ClaudeEdit::SaveMarkdown {
        kind: MdKind::Skill,
        project: false,
        original_path: Some(config.join("skills/deploy/notes.md")),
        name: "deploy".into(),
        fields: vec![("description".into(), "d".into())],
        body: String::new(),
    };
    assert!(apply_edit(&fx.paths, edit_other).is_err());

    apply_edit(
        &fx.paths,
        ClaudeEdit::DeleteItem {
            path: config.join("skills/deploy"),
        },
    )
    .unwrap();
    assert!(!config.join("skills/deploy").exists());
    assert!(config.exists());
}

#[test]
fn nested_commands_can_be_deleted() {
    let fx = fixture();
    let nested = fx.paths.config_dir.join("commands/frontend/review.md");
    fs::create_dir_all(nested.parent().unwrap()).unwrap();
    fs::write(&nested, "Review\n").unwrap();
    apply_edit(
        &fx.paths,
        ClaudeEdit::DeleteItem {
            path: nested.clone(),
        },
    )
    .unwrap();
    assert!(!nested.exists());
}

#[cfg(unix)]
#[test]
fn symlinked_agent_dirs_cannot_reach_outside_the_root() {
    use std::os::unix::fs::symlink;

    let fx = fixture();
    let outside = fx.repo.join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("victim.md"), "---\nname: victim\n---\nkeep\n").unwrap();
    let agents = fx.paths.config_dir.join("agents");
    fs::create_dir_all(&agents).unwrap();
    symlink(&outside, agents.join("linked")).unwrap();
    symlink(outside.join("victim.md"), agents.join("file-link.md")).unwrap();

    let through_dir = agents.join("linked/victim.md");
    assert!(
        apply_edit(
            &fx.paths,
            ClaudeEdit::DeleteItem {
                path: through_dir.clone()
            }
        )
        .is_err()
    );
    let edit = |path: PathBuf| ClaudeEdit::SaveMarkdown {
        kind: MdKind::Agent,
        project: false,
        original_path: Some(path),
        name: "victim".into(),
        fields: vec![],
        body: "overwritten\n".into(),
    };
    assert!(apply_edit(&fx.paths, edit(through_dir)).is_err());
    assert!(apply_edit(&fx.paths, edit(agents.join("file-link.md"))).is_err());
    assert_eq!(
        fs::read_to_string(outside.join("victim.md")).unwrap(),
        "---\nname: victim\n---\nkeep\n"
    );

    // Deleting a symlinked file removes only the link.
    apply_edit(
        &fx.paths,
        ClaudeEdit::DeleteItem {
            path: agents.join("file-link.md"),
        },
    )
    .unwrap();
    assert!(outside.join("victim.md").exists());
}

#[cfg(unix)]
#[test]
fn symlinked_skill_dir_is_editable_and_delete_removes_only_the_link() {
    use std::os::unix::fs::symlink;

    let fx = fixture();
    let shared = fx.repo.join("shared/deploy");
    fs::create_dir_all(&shared).unwrap();
    fs::write(
        shared.join("SKILL.md"),
        "---\nname: deploy\ndescription: d\n---\nold\n",
    )
    .unwrap();
    let link = fx.paths.config_dir.join("skills/deploy");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    symlink(&shared, &link).unwrap();

    apply_edit(
        &fx.paths,
        ClaudeEdit::SaveMarkdown {
            kind: MdKind::Skill,
            project: false,
            original_path: Some(link.join("SKILL.md")),
            name: "deploy".into(),
            fields: vec![],
            body: "new\n".into(),
        },
    )
    .unwrap();
    assert!(
        fs::read_to_string(shared.join("SKILL.md"))
            .unwrap()
            .ends_with("new\n")
    );
    apply_edit(&fx.paths, ClaudeEdit::DeleteItem { path: link.clone() }).unwrap();
    assert!(fs::symlink_metadata(&link).is_err());
    assert!(shared.join("SKILL.md").exists());
}

#[test]
fn common_settings_only_write_changed_fields_onto_fresh_values() {
    let fx = fixture();
    let settings = fx.paths.config_dir.join("settings.json");
    // The form was opened when allow was ["Bash(ls)"]; since then Claude Code added a rule, a
    // non-string entry exists, and the model changed.
    write_json(
        &settings,
        json!({
            "model": "sonnet",
            "permissions": {"allow": ["Bash(ls)", "Bash(git status)", {"weird": true}], "deny": ["Read(.env)"]},
            "env": {"KEEP": "1", "DROP": "2"}
        }),
    );
    apply_edit(
        &fx.paths,
        ClaudeEdit::SetCommonSettings {
            scope: SettingsScope::User,
            update: CommonSettingsUpdate {
                allow: ListDiff::between(
                    &["Bash(ls)".to_owned()],
                    &["Bash(ls)".to_owned(), "Bash(cargo test)".to_owned()],
                ),
                env_set: obj(json!({"NEW": "3"})),
                env_remove: vec!["DROP".into()],
                ..Default::default()
            },
        },
    )
    .unwrap();
    let value = read_json(&settings);
    assert_eq!(value["model"], json!("sonnet"));
    assert_eq!(
        value["permissions"]["allow"],
        json!(["Bash(ls)", "Bash(git status)", {"weird": true}, "Bash(cargo test)"])
    );
    assert_eq!(value["permissions"]["deny"], json!(["Read(.env)"]));
    assert_eq!(value["env"], json!({"KEEP": "1", "NEW": "3"}));

    let before = fs::read_to_string(&settings).unwrap();
    let message = apply_edit(
        &fx.paths,
        ClaudeEdit::SetCommonSettings {
            scope: SettingsScope::User,
            update: CommonSettingsUpdate::default(),
        },
    )
    .unwrap();
    assert_eq!(message, "Sin cambios");
    assert_eq!(fs::read_to_string(&settings).unwrap(), before);
}

#[test]
fn mcp_entry_keeps_type_and_rejects_unknown_types() {
    let ws = obj(json!({"type": "ws", "url": "wss://x"}));
    assert_eq!(mcp_form_transport(&ws), None);
    assert!(
        build_mcp_entry(
            Some(&ws),
            "http",
            "",
            "",
            "https://x",
            Map::new(),
            Map::new()
        )
        .is_err()
    );

    let streamable = obj(json!({"type": "streamable-http", "url": "https://x", "oauth": {"a": 1}}));
    assert_eq!(mcp_form_transport(&streamable), Some("http"));
    let entry = build_mcp_entry(
        Some(&streamable),
        "http",
        "",
        "",
        "https://y",
        Map::new(),
        Map::new(),
    )
    .unwrap();
    assert_eq!(entry["type"], json!("streamable-http"));
    assert_eq!(entry["oauth"], json!({"a": 1}));

    let untyped = obj(json!({"command": "srv"}));
    let entry = build_mcp_entry(
        Some(&untyped),
        "stdio",
        "srv2",
        "",
        "",
        Map::new(),
        Map::new(),
    )
    .unwrap();
    assert!(entry.get("type").is_none());
    assert_eq!(entry["command"], json!("srv2"));

    let entry = build_mcp_entry(None, "stdio", "srv", "", "", Map::new(), Map::new()).unwrap();
    assert_eq!(entry["type"], json!("stdio"));
}

#[test]
fn mcp_entry_keeps_masked_args_and_url() {
    let original = obj(json!({
        "command": "srv",
        "args": ["--api-key", "k-123", "--port", "80"],
    }));
    let masked = "--api-key\n••••••\n--port\n81";
    let entry = build_mcp_entry(
        Some(&original),
        "stdio",
        "srv",
        masked,
        "",
        Map::new(),
        Map::new(),
    )
    .unwrap();
    assert_eq!(entry["args"], json!(["--api-key", "k-123", "--port", "81"]));
    assert!(
        build_mcp_entry(
            Some(&original),
            "stdio",
            "srv",
            "--x\n••••••x",
            "",
            Map::new(),
            Map::new()
        )
        .is_err()
    );

    let remote = obj(json!({"type": "http", "url": "https://u:p@h/x?token=t"}));
    let entry = build_mcp_entry(
        Some(&remote),
        "http",
        "",
        "",
        "https://••••••@h/x?token=••••••",
        Map::new(),
        Map::new(),
    )
    .unwrap();
    assert_eq!(entry["url"], json!("https://u:p@h/x?token=t"));
}

#[test]
fn skill_toggle_uses_the_files_that_define_the_override() {
    let fx = fixture();
    let user = fx.paths.config_dir.join("settings.json");
    let project = fx.repo.join(".claude/settings.json");
    let local = fx.repo.join(".claude/settings.local.json");
    write_json(&user, json!({"skillOverrides": {"deploy": "off"}}));
    write_json(
        &project,
        json!({"skillOverrides": {"deploy": "name-only", "other": "off"}}),
    );

    // Disabling goes to the highest-precedence writable file that already has an override.
    apply_edit(
        &fx.paths,
        ClaudeEdit::SetSkillOverride {
            name: "deploy".into(),
            off: true,
            default_scope: SettingsScope::Local,
        },
    )
    .unwrap();
    assert_eq!(
        read_json(&project)["skillOverrides"]["deploy"],
        json!("off")
    );
    assert!(!local.exists());

    // Enabling removes the override everywhere.
    apply_edit(
        &fx.paths,
        ClaudeEdit::SetSkillOverride {
            name: "deploy".into(),
            off: false,
            default_scope: SettingsScope::Local,
        },
    )
    .unwrap();
    assert!(read_json(&user).get("skillOverrides").is_none());
    assert_eq!(
        read_json(&project)["skillOverrides"],
        json!({"other": "off"})
    );

    // With no override anywhere, disabling uses the default scope.
    apply_edit(
        &fx.paths,
        ClaudeEdit::SetSkillOverride {
            name: "deploy".into(),
            off: true,
            default_scope: SettingsScope::Local,
        },
    )
    .unwrap();
    assert_eq!(read_json(&local)["skillOverrides"]["deploy"], json!("off"));
}

#[test]
fn moving_a_hook_keeps_its_other_fields() {
    let fx = fixture();
    let settings = fx.paths.config_dir.join("settings.json");
    write_json(
        &settings,
        json!({"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{
            "type": "command",
            "command": "check.sh",
            "if": "Bash(git *)",
            "async": true,
            "shell": "bash",
            "statusMessage": "Checking",
            "timeout": 5
        }]}]}}),
    );
    apply_edit(
        &fx.paths,
        ClaudeEdit::UpsertHook {
            scope: SettingsScope::User,
            original: Some(HookLocation {
                event: "PreToolUse".into(),
                group_index: 0,
                hook_index: 0,
                expected_command: Some("check.sh".into()),
            }),
            hook: HookSpec {
                event: "PostToolUse".into(),
                matcher: Some("Edit".into()),
                command: "check2.sh".into(),
                timeout: None,
            },
        },
    )
    .unwrap();
    let value = read_json(&settings);
    assert!(value["hooks"].get("PreToolUse").is_none());
    assert_eq!(value["hooks"]["PostToolUse"][0]["matcher"], json!("Edit"));
    assert_eq!(
        value["hooks"]["PostToolUse"][0]["hooks"][0],
        json!({
            "type": "command",
            "command": "check2.sh",
            "if": "Bash(git *)",
            "async": true,
            "shell": "bash",
            "statusMessage": "Checking"
        })
    );
}

#[test]
fn debug_output_redacts_secrets() {
    let edits = [
        ClaudeEdit::UpsertMcpServer {
            scope: McpScope::User,
            original_name: None,
            name: "srv".into(),
            entry: obj(json!({"env": {"TOKEN": "mcp-secret"}})),
        },
        ClaudeEdit::SetCommonSettings {
            scope: SettingsScope::User,
            update: CommonSettingsUpdate {
                env_set: obj(json!({"API_KEY": "env-secret"})),
                ..Default::default()
            },
        },
    ];
    for edit in edits {
        let debug = format!("{edit:?}");
        assert!(
            !debug.contains("mcp-secret") && !debug.contains("env-secret"),
            "{debug}"
        );
    }
}
