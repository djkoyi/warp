//! Crash-safe writes for Claude Code configuration files: every change is preceded by a
//! timestamped backup and lands through a temp file renamed over the real target.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Map, Value};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Older backups of the same file beyond this count are deleted after each new backup.
const MAX_BACKUPS_PER_FILE: usize = 20;
/// How many times a read-modify-write is retried when the file changes underneath it.
const MAX_RMW_ATTEMPTS: usize = 3;

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

/// Atomically replaces `target` with `contents`, keeping the original permissions (new files are
/// private to the user). Returns the path that was written (the symlink target, if `target` is a
/// link).
pub fn atomic_write(target: &Path, contents: &[u8]) -> io::Result<PathBuf> {
    atomic_write_checked(target, contents, || Ok(true))
}

/// Like [`atomic_write`], but calls `still_valid` after the temp file is complete and right
/// before the rename; when it returns `false` the target is left untouched and the error has
/// kind [`io::ErrorKind::WouldBlock`].
fn atomic_write_checked(
    target: &Path,
    contents: &[u8],
    still_valid: impl FnOnce() -> io::Result<bool>,
) -> io::Result<PathBuf> {
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
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            // Created private so secrets are never readable by others, not even briefly.
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        if let Ok(metadata) = fs::metadata(&real) {
            file.set_permissions(metadata.permissions())?;
        }
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        if !still_valid()? {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "el archivo cambió mientras se escribía",
            ));
        }
        fs::rename(&temp, &real)?;
        sync_parent_dir(&real)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map(|()| real)
}

/// Makes a completed rename durable across a crash.
#[cfg(unix)]
fn sync_parent_dir(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

/// Directories cannot be opened for syncing on Windows; the rename itself is atomic there.
#[cfg(not(unix))]
fn sync_parent_dir(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Copies `target` into `backups_dir` under a timestamped name. Returns `None` when there is
/// nothing to back up. Symlinked directories are recorded by their link target, not copied.
pub fn backup(backups_dir: &Path, target: &Path) -> io::Result<Option<PathBuf>> {
    let Ok(metadata) = fs::symlink_metadata(target) else {
        return Ok(None);
    };
    create_private_dir(backups_dir)?;
    let destination = unique_backup_path(backups_dir, target);
    let result = backup_into(target, &metadata, &destination);
    prune_backups(backups_dir, target);
    result
}

fn backup_into(
    target: &Path,
    metadata: &fs::Metadata,
    destination: &Path,
) -> io::Result<Option<PathBuf>> {
    if metadata.file_type().is_symlink() {
        let link = fs::read_link(target)?;
        let note = symlink_note_path(destination);
        fs::write(
            &note,
            format!("{} -> {}\n", target.display(), link.display()),
        )?;
        if fs::metadata(target).map(|m| m.is_file()).unwrap_or(false) {
            fs::copy(target, destination)?;
        }
        return Ok(Some(note));
    }
    if metadata.is_dir() {
        copy_dir(target, destination)?;
    } else {
        fs::copy(target, destination)?;
    }
    Ok(Some(destination.to_path_buf()))
}

fn symlink_note_path(destination: &Path) -> PathBuf {
    let mut name = destination.as_os_str().to_owned();
    name.push(".symlink.txt");
    PathBuf::from(name)
}

/// Deletes all but the [`MAX_BACKUPS_PER_FILE`] most recent backups of `target`. Best-effort.
fn prune_backups(backups_dir: &Path, target: &Path) {
    let key = sanitized_backup_key(target);
    let Ok(entries) = fs::read_dir(backups_dir) else {
        return;
    };
    let mut groups: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some((timestamp, rest)) = name.split_once('_') else {
            continue;
        };
        let Some(rest) = rest.strip_prefix(key.as_str()) else {
            continue;
        };
        // `<key>`, `<key>.<n>` (same-millisecond counter), plus an optional `.symlink.txt`.
        let rest = rest.strip_suffix(".symlink.txt").unwrap_or(rest);
        let counter = match rest.strip_prefix('.') {
            None if rest.is_empty() => "",
            Some(digits) if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) => {
                digits
            }
            None | Some(_) => continue,
        };
        groups
            .entry(format!("{timestamp}.{counter:0>6}"))
            .or_default()
            .push(entry.path());
    }
    let excess = groups.len().saturating_sub(MAX_BACKUPS_PER_FILE);
    for paths in groups.into_values().take(excess) {
        for path in paths {
            let removed = if fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_file(&path)
            };
            if let Err(err) = removed {
                log::warn!("No se pudo borrar una copia antigua de Claude Code: {err}");
            }
        }
    }
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
    let bytes = read_optional_bytes(path)?;
    match bytes {
        None => Ok(Map::new()),
        Some(bytes) if bytes.trim_ascii().is_empty() => Ok(Map::new()),
        Some(bytes) => parse_json_object(path, &bytes),
    }
}

fn read_optional_bytes(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("No se pudo leer {}: {err}", path.display())),
    }
}

fn parse_json_object(path: &Path, bytes: &[u8]) -> Result<Map<String, Value>, String> {
    match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(format!("{} no contiene un objeto JSON", path.display())),
        Err(err) => Err(format!("JSON inválido en {}: {err}", path.display())),
    }
}

/// Read-modify-write of a JSON file: `update` runs on a fresh read, and the file is compared
/// with that read again right before the rename, so a concurrent write by Claude Code is never
/// lost. On a conflict the whole read-modify-write is retried, up to [`MAX_RMW_ATTEMPTS`] times.
///
/// An existing but empty file is refused instead of treated as `{}`: Claude Code truncates and
/// rewrites `.claude.json`, and replacing it mid-write would drop all of its state.
pub fn update_json_file(
    backups_dir: &Path,
    path: &Path,
    mut update: impl FnMut(&mut Map<String, Value>) -> Result<(), String>,
) -> Result<(), String> {
    for _ in 0..MAX_RMW_ATTEMPTS {
        let original = read_optional_bytes(path)?;
        let mut root = match &original {
            None => Map::new(),
            Some(bytes) if bytes.trim_ascii().is_empty() => {
                return Err(format!(
                    "{} está vacío (quizá otro proceso lo está escribiendo); inténtalo de nuevo \
                     o corrígelo con \"Abrir archivo\"",
                    path.display()
                ));
            }
            Some(bytes) => parse_json_object(path, bytes)?,
        };
        update(&mut root)?;
        let mut text = serde_json::to_string_pretty(&Value::Object(root))
            .map_err(|err| format!("No se pudo serializar JSON: {err}"))?;
        text.push('\n');
        if original.is_some() {
            let real = fs::canonicalize(path)
                .map_err(|err| format!("No se pudo resolver {}: {err}", path.display()))?;
            backup(backups_dir, &real)
                .map_err(|err| format!("No se pudo copiar {}: {err}", path.display()))?;
        }
        let unchanged = || -> io::Result<bool> {
            match fs::read(path) {
                Ok(current) => Ok(original.as_deref() == Some(current.as_slice())),
                Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(original.is_none()),
                Err(err) => Err(err),
            }
        };
        match atomic_write_checked(path, text.as_bytes(), unchanged) {
            Ok(_) => return Ok(()),
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => continue,
            Err(err) => return Err(format!("No se pudo escribir {}: {err}", path.display())),
        }
    }
    Err(format!(
        "{} cambió mientras se guardaba ({MAX_RMW_ATTEMPTS} intentos); recarga e inténtalo de nuevo",
        path.display()
    ))
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

fn sanitized_backup_key(target: &Path) -> String {
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
    sanitized.trim_start_matches('_').to_owned()
}

fn unique_backup_path(backups_dir: &Path, target: &Path) -> PathBuf {
    // The timestamp contains no '_', so the first '_' separates it from the file key.
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S%.3f");
    let sanitized = sanitized_backup_key(target);
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
