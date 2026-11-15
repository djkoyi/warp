//! Installs the Claude Code meters bridge: copies the bundled script into the Claude config
//! directory and registers it as the `statusLine` command and as lifecycle hooks in
//! `settings.json`. An existing status line is preserved by chaining to it from the bridge.

use std::path::{Path, PathBuf};
use std::{env, fs, io};

use anyhow::{Context as _, bail};
use serde_json::{Map, Value, json};

/// The bridge script, bundled into the binary so installation needs no resource lookup.
pub const BRIDGE_SCRIPT: &str =
    include_str!("../../../resources/claude_meters/warp_claude_meters.py");

const SCRIPT_FILE_NAME: &str = "warp_claude_meters.py";
/// Substring identifying commands that already invoke the bridge.
const SCRIPT_MARKER: &str = "warp_claude_meters.py";
const STATUS_LINE_CHAIN_FILE: &str = "statusline_chain";
const BACKUP_MARKER_FILE: &str = "backup_enabled";
const HOOK_TIMEOUT_SECS: u64 = 10;

/// Hook events the bridge listens to.
const HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "Notification",
    "PostToolUse",
    "Stop",
    "PreCompact",
    "PostCompact",
];

/// What [`install`] did.
#[derive(Debug)]
pub struct InstallReport {
    pub script_path: PathBuf,
    pub settings_path: PathBuf,
    /// Copy of `settings.json` taken before it was modified, if it existed and changed.
    pub settings_backup_path: Option<PathBuf>,
    /// The status line command that was replaced and is now chained from the bridge.
    pub chained_status_line: Option<String>,
}

/// Result of merging the bridge into a Claude Code settings object.
#[derive(Debug, Default, PartialEq)]
pub struct MergeOutcome {
    pub changed: bool,
    pub previous_status_line: Option<String>,
}

fn claude_home_dir() -> io::Result<PathBuf> {
    if let Ok(dir) = env::var("CLAUDE_CONFIG_DIR")
        && !dir.is_empty()
    {
        return Ok(PathBuf::from(dir));
    }
    dirs::home_dir()
        .map(|home| home.join(".claude"))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "could not determine home directory",
            )
        })
}

fn bridge_dir() -> io::Result<PathBuf> {
    claude_home_dir().map(|dir| dir.join("warp"))
}

/// Quotes `value` for a POSIX shell command line.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

fn bridge_command(script_path: &Path, mode: &str) -> String {
    format!(
        "python3 -I {} {mode}",
        shell_quote(&script_path.to_string_lossy())
    )
}

fn invokes_bridge(command: Option<&Value>) -> bool {
    command
        .and_then(Value::as_str)
        .is_some_and(|command| command.contains(SCRIPT_MARKER))
}

/// Points `statusLine` at the bridge and appends a bridge hook for every event in [`HOOK_EVENTS`]
/// that does not already have one. Unrelated settings and hooks are left untouched.
pub fn merge_settings(
    settings: &mut Value,
    status_line_command: &str,
    hook_command: &str,
) -> anyhow::Result<MergeOutcome> {
    let Some(root) = settings.as_object_mut() else {
        bail!("settings.json is not a JSON object");
    };
    let mut outcome = MergeOutcome::default();

    let existing_status_line = root.get("statusLine");
    if !invokes_bridge(existing_status_line.and_then(|line| line.get("command"))) {
        outcome.previous_status_line = existing_status_line
            .and_then(|line| line.get("command"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|command| !command.is_empty())
            .map(str::to_owned);
        let mut status_line = Map::new();
        status_line.insert("type".to_owned(), json!("command"));
        status_line.insert("command".to_owned(), json!(status_line_command));
        for key in ["padding", "refreshInterval"] {
            if let Some(value) = existing_status_line.and_then(|line| line.get(key)) {
                status_line.insert(key.to_owned(), value.clone());
            }
        }
        root.insert("statusLine".to_owned(), Value::Object(status_line));
        outcome.changed = true;
    }

    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(hooks) = hooks.as_object_mut() else {
        bail!("`hooks` in settings.json is not a JSON object");
    };
    for event in HOOK_EVENTS {
        let matchers = hooks
            .entry(*event)
            .or_insert_with(|| Value::Array(Vec::new()));
        let Some(matchers) = matchers.as_array_mut() else {
            bail!("`hooks.{event}` in settings.json is not a JSON array");
        };
        let already_registered = matchers.iter().any(|matcher| {
            matcher
                .get("hooks")
                .and_then(Value::as_array)
                .is_some_and(|hooks| hooks.iter().any(|hook| invokes_bridge(hook.get("command"))))
        });
        if !already_registered {
            matchers.push(json!({
                "hooks": [{
                    "type": "command",
                    "command": hook_command,
                    "timeout": HOOK_TIMEOUT_SECS,
                }]
            }));
            outcome.changed = true;
        }
    }

    Ok(outcome)
}

/// Writes the bridge script and registers it in Claude Code's user `settings.json`. Idempotent.
/// Refuses to touch a `settings.json` it cannot parse.
pub fn install() -> anyhow::Result<InstallReport> {
    let claude_dir = claude_home_dir()?;
    let bridge_dir = claude_dir.join("warp");
    fs::create_dir_all(&bridge_dir)
        .with_context(|| format!("creating {}", bridge_dir.display()))?;

    let script_path = bridge_dir.join(SCRIPT_FILE_NAME);
    fs::write(&script_path, BRIDGE_SCRIPT)
        .with_context(|| format!("writing {}", script_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755))?;
    }

    let settings_path = claude_dir.join("settings.json");
    let original = match fs::read_to_string(&settings_path) {
        Ok(contents) => Some(contents),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => {
            return Err(err).with_context(|| format!("reading {}", settings_path.display()));
        }
    };
    let mut settings = match original.as_deref().map(str::trim) {
        None | Some("") => Value::Object(Map::new()),
        Some(contents) => serde_json::from_str(contents)
            .with_context(|| format!("{} is not valid JSON", settings_path.display()))?,
    };

    let outcome = merge_settings(
        &mut settings,
        &bridge_command(&script_path, "statusline"),
        &bridge_command(&script_path, "hook"),
    )?;

    if let Some(previous) = &outcome.previous_status_line {
        let chain_path = bridge_dir.join(STATUS_LINE_CHAIN_FILE);
        fs::write(&chain_path, format!("{previous}\n"))
            .with_context(|| format!("writing {}", chain_path.display()))?;
    }

    let mut settings_backup_path = None;
    if outcome.changed {
        if let Some(original) = &original {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or_default();
            let backup = claude_dir.join(format!("settings.json.bak-warp-meters-{stamp}"));
            fs::write(&backup, original)
                .with_context(|| format!("writing {}", backup.display()))?;
            settings_backup_path = Some(backup);
        }
        let serialized = serde_json::to_string_pretty(&settings)? + "\n";
        let tmp_path = claude_dir.join("settings.json.warp-tmp");
        fs::write(&tmp_path, serialized)
            .with_context(|| format!("writing {}", tmp_path.display()))?;
        fs::rename(&tmp_path, &settings_path)
            .with_context(|| format!("replacing {}", settings_path.display()))?;
    }

    Ok(InstallReport {
        script_path,
        settings_path,
        settings_backup_path,
        chained_status_line: outcome.previous_status_line,
    })
}

/// Whether the bridge copies the transcript before each compaction.
pub fn is_transcript_backup_enabled() -> bool {
    bridge_dir().is_ok_and(|dir| dir.join(BACKUP_MARKER_FILE).exists())
}

/// Turns transcript backups before compaction on or off for the bridge (a marker file it checks).
pub fn set_transcript_backup_enabled(enabled: bool) -> io::Result<()> {
    let dir = bridge_dir()?;
    let marker = dir.join(BACKUP_MARKER_FILE);
    if enabled {
        fs::create_dir_all(&dir)?;
        fs::write(marker, "")
    } else {
        match fs::remove_file(marker) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }
}

#[cfg(test)]
#[path = "meters_bridge_tests.rs"]
mod tests;
