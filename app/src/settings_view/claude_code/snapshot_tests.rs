use std::fs;
use std::path::Path;

use serde_json::json;
use tempfile::TempDir;

use super::*;

struct Fixture {
    _home: TempDir,
    repo: PathBuf,
    paths: ClaudePaths,
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn write_json(path: &Path, value: serde_json::Value) {
    write(path, &serde_json::to_string_pretty(&value).unwrap());
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
    Fixture {
        _home: home,
        repo,
        paths,
    }
}

#[test]
fn empty_setup_loads_without_errors() {
    let fx = fixture();
    let snapshot = load_snapshot(&fx.paths);
    assert!(snapshot.skills.is_empty());
    assert!(snapshot.mcp_servers.is_empty());
    assert!(snapshot.warnings.is_empty());
    assert!(snapshot.claude_json_error.is_none());
    assert!(
        snapshot
            .settings
            .iter()
            .all(|file| !file.exists || file.scope == SettingsScope::Managed)
    );
    assert_eq!(snapshot.memory_files.len(), 4);
}

#[test]
fn skills_from_user_project_and_plugins() {
    let fx = fixture();
    let config = &fx.paths.config_dir;
    write(
        &config.join("skills/deploy/SKILL.md"),
        "---\nname: deploy\ndescription: Ship it\n---\nSteps\n",
    );
    write(
        &config.join("skills/broken/SKILL.md"),
        "---\nname: [x\n---\n",
    );
    write(&config.join("skills/no-skill-file/README.md"), "ignored");
    write(
        &fx.repo.join(".claude/skills/lint/SKILL.md"),
        "---\ndescription: Lint\n---\n",
    );
    let plugin_root = fx.paths.plugins_dir.join("cache/mkt/tools/1.0.0");
    write(
        &plugin_root.join("skills/fmt/SKILL.md"),
        "---\nname: fmt\ndescription: Format\n---\n",
    );
    write_json(
        &fx.paths.plugins_dir.join("installed_plugins.json"),
        json!({"version": 2, "plugins": {"tools@mkt": [{
            "scope": "user",
            "installPath": plugin_root.to_string_lossy(),
            "version": "1.0.0"
        }]}}),
    );
    write_json(
        &config.join("settings.json"),
        json!({"skillOverrides": {"deploy": "off"}, "enabledPlugins": {"tools@mkt": true}}),
    );

    let snapshot = load_snapshot(&fx.paths);
    let names: Vec<_> = snapshot.skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["broken", "deploy", "lint", "fmt"]);

    let deploy = &snapshot.skills[1];
    assert_eq!(deploy.origin, ItemOrigin::User);
    assert_eq!(deploy.description().as_deref(), Some("Ship it"));
    assert_eq!(deploy.override_state.as_deref(), Some("off"));
    assert_eq!(deploy.removal_path(), config.join("skills/deploy"));

    assert!(snapshot.skills[0].error.is_some());
    assert_eq!(snapshot.skills[2].origin, ItemOrigin::Project);
    assert_eq!(
        snapshot.skills[3].origin,
        ItemOrigin::Plugin("tools@mkt".to_owned())
    );
    assert!(snapshot.skills[3].origin.is_read_only());

    let plugin = &snapshot.plugins[0];
    assert!(plugin.installed);
    assert_eq!(plugin.enabled, Some(true));
    assert_eq!(plugin.version.as_deref(), Some("1.0.0"));
}

#[test]
fn agents_and_nested_commands() {
    let fx = fixture();
    let config = &fx.paths.config_dir;
    write(
        &config.join("agents/reviewer.md"),
        "---\nname: code-reviewer\ndescription: Reviews\ntools: Read, Grep\n---\nYou review.\n",
    );
    write(
        &config.join("commands/frontend/component.md"),
        "Make a component\n",
    );
    write(&config.join("commands/notes.txt"), "ignored");

    let snapshot = load_snapshot(&fx.paths);
    assert_eq!(snapshot.agents.len(), 1);
    assert_eq!(snapshot.agents[0].name, "code-reviewer");
    assert_eq!(snapshot.commands.len(), 1);
    assert_eq!(snapshot.commands[0].name, "frontend:component");
    assert!(snapshot.commands[0].error.is_none());
}

#[test]
fn mcp_servers_from_every_scope_with_state() {
    let fx = fixture();
    let repo_key = fx.repo.to_string_lossy().into_owned();
    let mut projects = Map::new();
    projects.insert(
        repo_key.clone(),
        json!({
            "mcpServers": {"db": {"command": "npx", "args": ["-y", "db"], "env": {"TOKEN": "t"}}},
            "disabledMcpServers": ["github"]
        }),
    );
    write_json(
        &fx.paths.claude_json,
        json!({
            "mcpServers": {
                "github": {"type": "http", "url": "https://api", "headers": {"Authorization": "Bearer s"}}
            },
            "projects": projects,
        }),
    );
    write_json(
        &fx.repo.join(".mcp.json"),
        json!({"mcpServers": {
            "approved": {"command": "a"},
            "rejected": {"command": "b"},
            "pending": {"type": "sse", "url": "https://sse"}
        }}),
    );
    write_json(
        &fx.repo.join(".claude/settings.local.json"),
        json!({"enabledMcpjsonServers": ["approved"], "disabledMcpjsonServers": ["rejected"]}),
    );

    let snapshot = load_snapshot(&fx.paths);
    let find = |name: &str| {
        snapshot
            .mcp_servers
            .iter()
            .find(|server| server.name == name)
            .unwrap()
    };
    let github = find("github");
    assert_eq!(github.scope, McpScope::User);
    assert_eq!(github.transport, "http");
    assert_eq!(github.header_keys, vec!["Authorization".to_owned()]);
    assert_eq!(github.state, McpState::Disabled);

    let db = find("db");
    assert_eq!(db.scope, McpScope::Local(repo_key));
    assert_eq!(db.transport, "stdio");
    assert_eq!(db.summary(), "npx -y db");
    assert_eq!(db.env_keys, vec!["TOKEN".to_owned()]);
    assert_eq!(db.state, McpState::Enabled);

    assert_eq!(find("approved").state, McpState::Enabled);
    assert_eq!(find("rejected").state, McpState::Disabled);
    assert_eq!(find("pending").state, McpState::PendingApproval);
    assert_eq!(find("pending").transport, "sse");
}

#[test]
fn mcp_summary_and_debug_hide_secrets() {
    let fx = fixture();
    write_json(
        &fx.paths.claude_json,
        json!({"mcpServers": {
            "remote": {"type": "http", "url": "https://me:hunter2@api.example/mcp?apikey=s3cr3t&v=1"},
            "local": {"command": "srv", "args": ["--token", "tok123", "--verbose"], "env": {"K": "envsecret"}}
        }}),
    );
    write_json(
        &fx.paths.config_dir.join("settings.json"),
        json!({"env": {"ANTHROPIC_API_KEY": "settingsecret"}}),
    );
    let snapshot = load_snapshot(&fx.paths);
    let find = |name: &str| {
        snapshot
            .mcp_servers
            .iter()
            .find(|server| server.name == name)
            .unwrap()
    };
    assert_eq!(
        find("remote").summary(),
        "https://••••••@api.example/mcp?apikey=••••••&v=••••••"
    );
    assert_eq!(find("local").summary(), "srv --token •••••• --verbose");

    let debug = format!("{snapshot:?}");
    for secret in ["hunter2", "s3cr3t", "tok123", "envsecret", "settingsecret"] {
        assert!(!debug.contains(secret), "{secret} leaked into Debug output");
    }
}

#[test]
fn invalid_files_are_reported_not_fatal() {
    let fx = fixture();
    write(&fx.paths.claude_json, "{ nope");
    write(&fx.paths.config_dir.join("settings.json"), "[1]");
    write(&fx.repo.join(".mcp.json"), "{");

    let snapshot = load_snapshot(&fx.paths);
    assert!(snapshot.claude_json_error.is_some());
    let user = snapshot.settings_file(SettingsScope::User).unwrap();
    assert!(user.exists);
    assert!(user.error.is_some());
    assert!(snapshot.warnings.iter().any(|w| w.contains(".mcp.json")));
}

#[test]
fn hooks_attribution_and_permissions() {
    let fx = fixture();
    write_json(
        &fx.paths.config_dir.join("settings.json"),
        json!({
            "model": "sonnet",
            "env": {"SECRET": "value"},
            "permissions": {"allow": ["Bash(ls:*)"], "defaultMode": "plan"},
            "hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [
                {"type": "command", "command": "echo hi", "timeout": 30}
            ]}]}
        }),
    );
    write_json(
        &fx.repo.join(".claude/settings.json"),
        json!({"model": "opus", "permissions": {"deny": ["Read(.env)"]}}),
    );

    let snapshot = load_snapshot(&fx.paths);
    assert_eq!(snapshot.hooks.len(), 1);
    let hook = &snapshot.hooks[0];
    assert_eq!(hook.event, "PreToolUse");
    assert_eq!(hook.matcher.as_deref(), Some("Bash"));
    assert_eq!(hook.command.as_deref(), Some("echo hi"));
    assert_eq!(hook.timeout, Some(30));
    assert_eq!(hook.source, HookSource::Settings(SettingsScope::User));

    let model = snapshot
        .attributions
        .iter()
        .find(|a| a.key == "model")
        .unwrap();
    assert_eq!(model.effective, SettingsScope::Project);
    assert_eq!(model.entries.len(), 2);

    let env = snapshot
        .attributions
        .iter()
        .find(|a| a.key == "env")
        .unwrap();
    assert!(
        env.entries
            .iter()
            .all(|(_, summary)| !summary.contains("value"))
    );

    assert_eq!(snapshot.permission_rules.len(), 2);
    assert!(
        snapshot
            .permission_rules
            .iter()
            .any(|r| r.kind == "deny" && r.scope == SettingsScope::Project)
    );
}

#[test]
fn plugins_enabled_but_not_installed_are_listed() {
    let fx = fixture();
    write_json(
        &fx.repo.join(".claude/settings.json"),
        json!({"enabledPlugins": {"ghost@mkt": false}}),
    );
    let snapshot = load_snapshot(&fx.paths);
    assert_eq!(snapshot.plugins.len(), 1);
    assert!(!snapshot.plugins[0].installed);
    assert_eq!(snapshot.plugins[0].enabled, Some(false));
    assert_eq!(
        snapshot.plugins[0].enabled_source,
        Some(SettingsScope::Project)
    );
}
