use std::fs;

use serde_json::json;
use tempfile::TempDir;

use super::*;

#[test]
fn atomic_write_creates_parent_and_file() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("a").join("b.json");
    atomic_write(&target, b"{}").unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), "{}");
    let leftovers: Vec<_> = fs::read_dir(target.parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains("warp-tmp"))
        .collect();
    assert!(leftovers.is_empty());
}

#[cfg(unix)]
#[test]
fn atomic_write_keeps_permissions_and_symlinks() {
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    let dir = TempDir::new().unwrap();
    let real = dir.path().join("real.json");
    fs::write(&real, "old").unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
    let link = dir.path().join("link.json");
    symlink(&real, &link).unwrap();

    let written = atomic_write(&link, b"new").unwrap();
    assert_eq!(written, fs::canonicalize(&real).unwrap());
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_to_string(&real).unwrap(), "new");
    assert_eq!(
        fs::metadata(&real).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn write_with_backup_copies_the_previous_contents() {
    let dir = TempDir::new().unwrap();
    let backups = dir.path().join("backups");
    let target = dir.path().join("settings.json");
    fs::write(&target, "before").unwrap();

    write_with_backup(&backups, &target, b"after").unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), "after");
    let backups: Vec<_> = fs::read_dir(&backups)
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(backups.len(), 1);
    assert_eq!(fs::read_to_string(backups[0].path()).unwrap(), "before");
    assert!(
        backups[0]
            .file_name()
            .to_string_lossy()
            .ends_with("settings.json")
    );
}

#[test]
fn write_without_existing_file_makes_no_backup() {
    let dir = TempDir::new().unwrap();
    let backups = dir.path().join("backups");
    write_with_backup(&backups, &dir.path().join("new.json"), b"x").unwrap();
    assert!(!backups.exists());
}

#[test]
fn update_json_file_preserves_other_keys() {
    let dir = TempDir::new().unwrap();
    let backups = dir.path().join("backups");
    let target = dir.path().join(".claude.json");
    fs::write(
        &target,
        serde_json::to_string(&json!({
            "numStartups": 7,
            "projects": {"/p": {"allowedTools": ["Bash"], "mcpServers": {}}},
            "mcpServers": {"old": {"command": "x"}}
        }))
        .unwrap(),
    )
    .unwrap();

    update_json_file(&backups, &target, |root| {
        object_entry(root, "mcpServers")?.insert("new".into(), json!({"type": "http", "url": "u"}));
        Ok(())
    })
    .unwrap();

    let value: Value = serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
    assert_eq!(value["numStartups"], json!(7));
    assert_eq!(value["projects"]["/p"]["allowedTools"], json!(["Bash"]));
    assert_eq!(value["mcpServers"]["old"]["command"], json!("x"));
    assert_eq!(value["mcpServers"]["new"]["url"], json!("u"));
}

#[test]
fn update_json_file_refuses_invalid_json() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("settings.json");
    fs::write(&target, "{ not json").unwrap();
    let result = update_json_file(&dir.path().join("b"), &target, |_| Ok(()));
    assert!(result.is_err());
    assert_eq!(fs::read_to_string(&target).unwrap(), "{ not json");
}

#[test]
fn object_entry_rejects_non_objects() {
    let mut root = Map::new();
    root.insert("mcpServers".into(), json!([1, 2]));
    assert!(object_entry(&mut root, "mcpServers").is_err());
}

#[test]
fn backup_and_remove_directory() {
    let dir = TempDir::new().unwrap();
    let backups = dir.path().join("backups");
    let skill = dir.path().join("skills").join("demo");
    fs::create_dir_all(skill.join("scripts")).unwrap();
    fs::write(skill.join("SKILL.md"), "---\nname: demo\n---\n").unwrap();
    fs::write(skill.join("scripts").join("run.sh"), "echo").unwrap();

    let backup_path = backup_and_remove(&backups, &skill).unwrap().unwrap();
    assert!(!skill.exists());
    assert!(backup_path.join("SKILL.md").exists());
    assert!(backup_path.join("scripts").join("run.sh").exists());
}

#[cfg(unix)]
#[test]
fn backup_and_remove_symlinked_directory_removes_only_the_link() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new().unwrap();
    let backups = dir.path().join("backups");
    let real = dir.path().join("shared-skill");
    fs::create_dir_all(&real).unwrap();
    fs::write(real.join("SKILL.md"), "x").unwrap();
    let link = dir.path().join("skills").join("shared-skill");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    symlink(&real, &link).unwrap();

    backup_and_remove(&backups, &link).unwrap();
    assert!(fs::symlink_metadata(&link).is_err());
    assert!(real.join("SKILL.md").exists());
}

#[cfg(unix)]
#[test]
fn new_files_are_private_and_existing_permissions_are_kept() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = TempDir::new().unwrap();
    let fresh = dir.path().join("fresh.json");
    atomic_write(&fresh, b"{}").unwrap();
    assert_eq!(
        fs::metadata(&fresh).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let shared = dir.path().join("shared.md");
    fs::write(&shared, "old").unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o644)).unwrap();
    atomic_write(&shared, b"new").unwrap();
    assert_eq!(
        fs::metadata(&shared).unwrap().permissions().mode() & 0o777,
        0o644
    );
}

#[test]
fn update_json_file_refuses_an_existing_empty_file() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join(".claude.json");
    fs::write(&target, "  \n").unwrap();
    let result = update_json_file(&dir.path().join("b"), &target, |root| {
        root.insert("x".into(), json!(1));
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(fs::read_to_string(&target).unwrap(), "  \n");
}

#[test]
fn update_json_file_retries_when_the_file_changes_concurrently() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join(".claude.json");
    fs::write(&target, r#"{"a": 1}"#).unwrap();
    let mut calls = 0;
    update_json_file(&dir.path().join("b"), &target, |root| {
        calls += 1;
        if calls == 1 {
            // Another process writes between our read and our rename.
            fs::write(&target, r#"{"a": 1, "fromClaude": true}"#).unwrap();
        }
        root.insert("ours".into(), json!(true));
        Ok(())
    })
    .unwrap();
    assert_eq!(calls, 2);
    let value: Value = serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
    assert_eq!(value, json!({"a": 1, "fromClaude": true, "ours": true}));
}

#[test]
fn update_json_file_gives_up_after_repeated_conflicts() {
    let dir = TempDir::new().unwrap();
    let target = dir.path().join(".claude.json");
    fs::write(&target, r#"{"n": 0}"#).unwrap();
    let mut calls = 0;
    let result = update_json_file(&dir.path().join("b"), &target, |root| {
        calls += 1;
        fs::write(&target, format!(r#"{{"n": {calls}}}"#)).unwrap();
        root.insert("ours".into(), json!(true));
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(calls, 3);
    let value: Value = serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
    assert_eq!(value, json!({"n": 3}));
    let leftovers = fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains("warp-tmp"))
        .count();
    assert_eq!(leftovers, 0);
}

#[test]
fn backups_keep_only_the_most_recent_per_file() {
    let dir = TempDir::new().unwrap();
    let backups = dir.path().join("backups");
    let target = dir.path().join("settings.json");
    let other = dir.path().join("settings.json.bak");
    fs::write(&other, "other").unwrap();
    backup(&backups, &other).unwrap();
    for index in 0..(MAX_BACKUPS_PER_FILE + 5) {
        fs::write(&target, format!("v{index}")).unwrap();
        backup(&backups, &target).unwrap();
    }
    let key = sanitized_backup_key(&target);
    let mut kept: Vec<String> = fs::read_dir(&backups)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| {
            name.split_once('_')
                .is_some_and(|(_, rest)| rest.starts_with(&key))
        })
        .collect();
    kept.sort();
    // The other file's single backup is not part of this file's group.
    assert_eq!(kept.len(), MAX_BACKUPS_PER_FILE + 1);
    let contents: Vec<String> = kept
        .iter()
        .filter(|name| !name.contains(".bak"))
        .map(|name| fs::read_to_string(backups.join(name)).unwrap())
        .collect();
    assert_eq!(contents.len(), MAX_BACKUPS_PER_FILE);
    assert!(!contents.contains(&"v0".to_owned()));
    assert!(contents.contains(&format!("v{}", MAX_BACKUPS_PER_FILE + 4)));
}
