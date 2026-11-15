//! Crash-safe writes for Claude Code configuration files: every change is preceded by a
//! timestamped backup and lands through a temp file renamed over the real target.

use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Map, Value};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Resolves the file that should actually be replaced: symlinks are followed so the link
/// itself survives, and a missing file resolves through its (created) parent directory.
pub fn resolve_write_target(target: &Path) -> io::Result<PathBuf> {
    if fs::symlink_metadata(target).is_ok() {
        return fs::canonicalize(target);
    }
    let parent = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "ruta sin directorio"))?;
    fs::create_dir_all(parent)?;
    let file_name = target
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "ruta sin nombre"))?;
    Ok(fs::canonicalize(parent)?.join(file_name))
}

/// Atomically replaces `target` with `contents`, keeping the original permissions. Returns the
/// path that was written (the symlink target, if `target` is a link).
pub fn atomic_write(target: &Path, contents: &[u8]) -> io::Result<PathBuf> {
    let real = resolve_write_target(target)?;
    let file_name = real
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = real.with_file_name(format!(
        ".{file_name}.warp-tmp-{}-{unique}",
        std::process::id()
    ));

    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        if let Ok(metadata) = fs::metadata(&real) {
            fs::set_permissions(&temp, metadata.permissions())?;
        }
        fs::rename(&temp, &real)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map(|()| real)
}

/// Copies `target` into `backups_dir` under a timestamped name. Returns `None` when there is
/// nothing to back up. Symlinked directories are recorded by their link target, not copied.
pub fn backup(backups_dir: &Path, target: &Path) -> io::Result<Option<PathBuf>> {
    let Ok(metadata) = fs::symlink_metadata(target) else {
        return Ok(None);
    };
    create_private_dir(backups_dir)?;
    let destination = unique_backup_path(backups_dir, target);
    if metadata.file_type().is_symlink() {
        let link = fs::read_link(target)?;
        let note = destination.with_extension("symlink.txt");
        fs::write(
            &note,
            format!("{} -> {}\n", target.display(), link.display()),
        )?;
        if fs::metadata(target).map(|m| m.is_file()).unwrap_or(false) {
            fs::copy(target, &destination)?;
        }
        return Ok(Some(note));
    }
    if metadata.is_dir() {
        copy_dir(target, &destination)?;
    } else {
        fs::copy(target, &destination)?;
    }
    Ok(Some(destination))
}

/// Backs up and then deletes a file, a directory, or a symlink (only the link is removed).
pub fn backup_and_remove(backups_dir: &Path, target: &Path) -> io::Result<Option<PathBuf>> {
    let backup_path = backup(backups_dir, target)?;
    let metadata = fs::symlink_metadata(target)?;
    if metadata.is_dir() {
        fs::remove_dir_all(target)?;
    } else {
        fs::remove_file(target)?;
    }
    Ok(backup_path)
}

/// Backs up `target` (if it exists) and atomically writes `contents` to it.
pub fn write_with_backup(backups_dir: &Path, target: &Path, contents: &[u8]) -> io::Result<()> {
    if fs::symlink_metadata(target).is_ok() {
        let real = fs::canonicalize(target)?;
        backup(backups_dir, &real)?;
    }
    atomic_write(target, contents)?;
    Ok(())
}

/// Reads a JSON object from disk. A missing or empty file is an empty object; anything that is
/// not a JSON object is an error so it is never overwritten by a write.
pub fn read_json_object(path: &Path) -> Result<Map<String, Value>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(err) => return Err(format!("No se pudo leer {}: {err}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(format!("{} no contiene un objeto JSON", path.display())),
        Err(err) => Err(format!("JSON inválido en {}: {err}", path.display())),
    }
}

/// Read-modify-write of a JSON file: the file is re-read immediately before `update` runs, so
/// keys written concurrently by Claude Code outside the edited subtree are kept.
pub fn update_json_file(
    backups_dir: &Path,
    path: &Path,
    update: impl FnOnce(&mut Map<String, Value>) -> Result<(), String>,
) -> Result<(), String> {
    let mut root = read_json_object(path)?;
    update(&mut root)?;
    let mut text = serde_json::to_string_pretty(&Value::Object(root))
        .map_err(|err| format!("No se pudo serializar JSON: {err}"))?;
    text.push('\n');
    write_with_backup(backups_dir, path, text.as_bytes())
        .map_err(|err| format!("No se pudo escribir {}: {err}", path.display()))
}

/// Returns the object at `key` inside `parent`, creating it (or failing, if a non-object value
/// is there) as needed.
pub fn object_entry<'a>(
    parent: &'a mut Map<String, Value>,
    key: &str,
) -> Result<&'a mut Map<String, Value>, String> {
    let entry = parent
        .entry(key.to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    match entry {
        Value::Object(map) => Ok(map),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_) => {
            Err(format!("La clave \"{key}\" no es un objeto JSON"))
        }
    }
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        // Backups may contain MCP credentials and env values.
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn unique_backup_path(backups_dir: &Path, target: &Path) -> PathBuf {
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S%.3f");
    let sanitized: String = target
        .to_string_lossy()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let sanitized = sanitized.trim_start_matches('_');
    let base = format!("{timestamp}_{sanitized}");
    let mut candidate = backups_dir.join(&base);
    let mut counter = 1;
    while fs::symlink_metadata(&candidate).is_ok() {
        candidate = backups_dir.join(format!("{base}.{counter}"));
        counter += 1;
    }
    candidate
}

fn copy_dir(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let to = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), &to)?;
        } else if file_type.is_symlink() {
            let link = fs::read_link(entry.path())?;
            fs::write(
                to.with_extension("symlink.txt"),
                format!("{}\n", link.display()),
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "safe_write_tests.rs"]
mod tests;
