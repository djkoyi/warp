//! Index of the user's recent Claude Code conversations, read from the transcripts Claude Code
//! keeps under `<config dir>/projects/<encoded cwd>/<session id>.jsonl`.
//!
//! Transcripts can grow to tens of megabytes, so only a bounded prefix (for the working directory
//! and the first user message) and a bounded suffix (for the latest title) of each file are read.
//! Every function here touches the filesystem: call them off the UI thread.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;

use super::claude_code_resume::is_valid_session_id;
use crate::settings_view::claude_code::paths::ClaudePaths;

/// How many conversations the picker lists.
pub const DEFAULT_LIMIT: usize = 30;
/// Maximum length, in characters, of a title built from the first user message.
pub const MAX_TITLE_CHARS: usize = 60;

/// Bytes read from the start of a transcript looking for the first user message. Early lines are
/// often large attachments (memory files, skill listings), so this is generous.
const HEAD_BYTES: u64 = 1024 * 1024;
/// Bytes read from the end of a transcript looking for the latest title.
const TAIL_BYTES: u64 = 256 * 1024;
/// Transcripts examined per listed entry before giving up, so a directory full of empty or
/// unreadable transcripts cannot turn one refresh into thousands of reads.
const MAX_FILES_PER_ENTRY: usize = 4;

/// One resumable Claude Code conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaudeSessionEntry {
    pub session_id: String,
    pub title: String,
    /// Directory Claude Code ran in; `--resume` must run from it.
    pub cwd: String,
    pub modified: SystemTime,
}

/// `<config dir>/projects`, honoring `$CLAUDE_CONFIG_DIR`.
pub fn projects_dir() -> Option<PathBuf> {
    ClaudePaths::from_env(None).map(|paths| paths.config_dir.join("projects"))
}

/// The `limit` most recently modified conversations under `projects_dir`, newest first.
/// Transcripts without a user message or a working directory are skipped.
pub fn recent_sessions(projects_dir: &Path, limit: usize) -> Vec<ClaudeSessionEntry> {
    let mut candidates = transcript_candidates(projects_dir);
    candidates.sort_by(|a, b| b.1.cmp(&a.1));

    candidates
        .into_iter()
        .take(limit.saturating_mul(MAX_FILES_PER_ENTRY))
        .filter_map(|(path, modified)| read_session(&path, modified))
        .take(limit)
        .collect()
}

/// Top-level `<session id>.jsonl` files in each project directory, with their modification time.
fn transcript_candidates(projects_dir: &Path) -> Vec<(PathBuf, SystemTime)> {
    let Ok(projects) = std::fs::read_dir(projects_dir) else {
        return Vec::new();
    };
    projects
        .flatten()
        .filter(|project| project.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|project| std::fs::read_dir(project.path()).ok())
        .flat_map(|entries| entries.flatten())
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "jsonl") {
                return None;
            }
            let stem = path.file_stem()?.to_str()?;
            if !is_valid_session_id(stem) || stem.starts_with("agent-") {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            if !metadata.is_file() || metadata.len() == 0 {
                return None;
            }
            Some((path, metadata.modified().ok()?))
        })
        .collect()
}

/// Reads the summary of one transcript, or `None` if it has no usable conversation.
pub fn read_session(path: &Path, modified: SystemTime) -> Option<ClaudeSessionEntry> {
    let session_id = path.file_stem()?.to_str()?.to_string();
    if !is_valid_session_id(&session_id) {
        return None;
    }
    let mut file = File::open(path).ok()?;

    let head = scan_head(&mut file);
    let tail = scan_tail(&mut file);

    // A transcript with neither a user message nor a title never had a real conversation.
    let title = tail
        .title
        .or_else(|| head.first_prompt.as_deref().map(title_from_prompt))
        .filter(|title| !title.is_empty())?;
    let cwd = head.cwd.or(tail.cwd)?;

    Some(ClaudeSessionEntry {
        session_id,
        title,
        cwd,
        modified,
    })
}

#[derive(Default)]
struct HeadScan {
    cwd: Option<String>,
    first_prompt: Option<String>,
}

fn scan_head(file: &mut File) -> HeadScan {
    let mut scan = HeadScan::default();
    let reader = BufReader::new(Read::by_ref(file).take(HEAD_BYTES));
    for line in reader.split(b'\n').map_while(Result::ok) {
        // Cheap prefilter: only user lines carry the prompt, and they also carry the cwd.
        if !contains(&line, b"\"user\"") {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("user") {
            continue;
        }
        if scan.cwd.is_none() {
            scan.cwd = value_cwd(&value);
        }
        if let Some(prompt) = user_prompt(&value) {
            if scan.cwd.is_none() {
                scan.cwd = value_cwd(&value);
            }
            scan.first_prompt = Some(prompt);
            break;
        }
    }
    scan
}

#[derive(Default)]
struct TailScan {
    title: Option<String>,
    cwd: Option<String>,
}

fn scan_tail(file: &mut File) -> TailScan {
    let mut scan = TailScan::default();
    let Ok(len) = file.metadata().map(|metadata| metadata.len()) else {
        return scan;
    };
    let start = len.saturating_sub(TAIL_BYTES);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return scan;
    }
    let mut buffer = Vec::new();
    if Read::by_ref(file).take(TAIL_BYTES).read_to_end(&mut buffer).is_err() {
        return scan;
    }
    let mut lines: Vec<&[u8]> = buffer.split(|byte| *byte == b'\n').collect();
    // The first line is cut in the middle unless the window starts at the beginning of the file.
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }

    let mut custom_title = None;
    let mut generated_title = None;
    for line in lines.iter().rev() {
        let has_title = contains(line, b"itle\"") || contains(line, b"\"summary\"");
        let wants_cwd = scan.cwd.is_none() && contains(line, b"\"cwd\"");
        if !has_title && !wants_cwd {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        if scan.cwd.is_none() {
            scan.cwd = value_cwd(&value);
        }
        match value.get("type").and_then(Value::as_str) {
            Some("custom-title") if custom_title.is_none() => {
                custom_title = string_field(&value, "customTitle");
            }
            Some("ai-title") if generated_title.is_none() => {
                generated_title = string_field(&value, "aiTitle");
            }
            Some("summary") if generated_title.is_none() => {
                generated_title = string_field(&value, "summary");
            }
            _ => {}
        }
        if custom_title.is_some() && scan.cwd.is_some() {
            break;
        }
    }
    // A title the user set with /rename wins over one Claude generated.
    scan.title = custom_title
        .or(generated_title)
        .map(|title| truncate_chars(&collapse_whitespace(&title), MAX_TITLE_CHARS));
    scan
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn value_cwd(value: &Value) -> Option<String> {
    string_field(value, "cwd").filter(|cwd| Path::new(cwd).is_absolute())
}

/// The text the user typed in a user transcript line, skipping tool results, sub-agent turns and
/// the synthetic messages Claude Code injects (slash command echoes, reminders, caveats).
fn user_prompt(value: &Value) -> Option<String> {
    if value.get("isMeta").and_then(Value::as_bool) == Some(true)
        || value.get("isSidechain").and_then(Value::as_bool) == Some(true)
    {
        return None;
    }
    let content = value.get("message")?.get("content")?;
    let text = match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => return None,
    };
    let text = text.trim();
    if text.is_empty() || text.starts_with('<') || text.starts_with("Caveat:") {
        return None;
    }
    Some(text.to_string())
}

/// A single-line title of at most [`MAX_TITLE_CHARS`] characters.
pub fn title_from_prompt(prompt: &str) -> String {
    truncate_chars(&collapse_whitespace(prompt), MAX_TITLE_CHARS)
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut truncated: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    truncated.truncate(truncated.trim_end().len());
    truncated.push('…');
    truncated
}

/// `cwd` with the home directory shown as `~`.
pub fn display_cwd(cwd: &str, home: Option<&Path>) -> String {
    let path = Path::new(cwd);
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(relative) if relative.as_os_str().is_empty() => "~".to_string(),
        Some(relative) => format!("~/{}", relative.display()),
        None => cwd.to_string(),
    }
}

/// How long ago `then` was, in Spanish ("hace 5 min", "ayer").
pub fn relative_time_es(now: SystemTime, then: SystemTime) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    const WEEK: u64 = 7 * DAY;
    const MONTH: u64 = 30 * DAY;

    let seconds = now
        .duration_since(then)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    match seconds {
        s if s < MINUTE => "ahora mismo".to_string(),
        s if s < HOUR => format!("hace {} min", s / MINUTE),
        s if s < DAY => format!("hace {} h", s / HOUR),
        s if s < 2 * DAY => "ayer".to_string(),
        s if s < WEEK => format!("hace {} días", s / DAY),
        s if s < MONTH => plural(s / WEEK, "semana", "semanas"),
        s => plural((s / MONTH).max(1), "mes", "meses"),
    }
}

fn plural(count: u64, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("hace 1 {singular}")
    } else {
        format!("hace {count} {plural}")
    }
}

#[cfg(test)]
#[path = "claude_sessions_index_tests.rs"]
mod tests;
