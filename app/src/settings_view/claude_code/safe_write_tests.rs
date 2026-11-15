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
