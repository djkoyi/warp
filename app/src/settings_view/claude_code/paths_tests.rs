use std::fs;

use tempfile::TempDir;

use super::*;

fn no_env(_: &str) -> Option<String> {
    None
}

#[test]
fn defaults_to_home_dot_claude() {
    let home = TempDir::new().unwrap();
    let paths = ClaudePaths::resolve(no_env, Some(home.path().to_path_buf()), None).unwrap();
    assert_eq!(paths.config_dir, home.path().join(".claude"));
    assert_eq!(paths.claude_json, home.path().join(".claude.json"));
    assert_eq!(paths.plugins_dir, home.path().join(".claude").join("plugins"));
    assert_eq!(
        paths.backups_dir(),
        home.path().join(".claude").join("warp").join("backups")
    );
    assert!(paths.project.is_none());
}

#[test]
fn respects_claude_config_dir() {
    let home = TempDir::new().unwrap();
    let custom = home.path().join("custom");
    let custom_str = custom.to_string_lossy().into_owned();
    let paths = ClaudePaths::resolve(
        |key| (key == "CLAUDE_CONFIG_DIR").then(|| custom_str.clone()),
        Some(home.path().to_path_buf()),
        None,
    )
    .unwrap();
    assert_eq!(paths.config_dir, custom);
    assert_eq!(paths.claude_json, custom.join(".claude.json"));
    assert_eq!(
        paths.settings_path(SettingsScope::User),
        Some(custom.join("settings.json"))
    );
}

#[test]
fn project_root_is_the_enclosing_repository() {
    let home = TempDir::new().unwrap();
    let repo = home.path().join("repo");
    let nested = repo.join("src").join("deep");
    fs::create_dir_all(&nested).unwrap();
    fs::create_dir_all(repo.join(".git")).unwrap();

    let paths =
        ClaudePaths::resolve(no_env, Some(home.path().to_path_buf()), Some(nested.clone()))
            .unwrap();
    let project = paths.project.clone().unwrap();
    assert_eq!(project.root, repo);
    assert_eq!(project.cwd, nested);
    assert_eq!(
        paths.settings_path(SettingsScope::Local),
        Some(repo.join(".claude").join("settings.local.json"))
    );
    assert_eq!(paths.project_mcp_json(), Some(repo.join(".mcp.json")));
    assert_eq!(
        paths.project_keys(),
        vec![
            repo.to_string_lossy().into_owned(),
            nested.to_string_lossy().into_owned()
        ]
    );
}

#[test]
fn home_directory_is_not_treated_as_a_project() {
    let home = TempDir::new().unwrap();
    let paths = ClaudePaths::resolve(
        no_env,
        Some(home.path().to_path_buf()),
        Some(home.path().to_path_buf()),
    )
    .unwrap();
    assert!(paths.project.is_none());
    assert!(paths.settings_path(SettingsScope::Project).is_none());
}
