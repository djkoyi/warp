//! Per-pane Claude Code session resume across Warp restarts.
//!
//! Every value that reaches the generated command line comes from a plugin notification or
//! from the app-state database, so it is treated as untrusted: session IDs are validated
//! against a strict charset and paths are quoted for the target shell.

use std::path::{Path, PathBuf};

use super::CLIAgent;
use super::cli_agent_sessions::CLIAgentSession;
use super::shell::ShellType;

const CLAUDE_COMMAND: &str = "claude";
const MAX_SESSION_ID_LEN: usize = 128;

/// The Claude Code conversation that was live in a terminal pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaudeCodeResume {
    pub session_id: String,
    /// Directory Claude Code was launched from. Claude Code keys transcripts by this directory,
    /// so `--resume` must run from it.
    pub cwd: Option<String>,
}

impl ClaudeCodeResume {
    /// Builds a resume record from persisted columns, discarding anything that fails validation.
    pub fn from_persisted(session_id: Option<String>, cwd: Option<String>) -> Option<Self> {
        let session_id = session_id.filter(|id| is_valid_session_id(id))?;
        Some(Self {
            session_id,
            cwd: cwd.filter(|cwd| is_safe_cwd(cwd)),
        })
    }

    /// Returns the resume record for a live, local, plugin-reported Claude Code session.
    pub fn from_live_session(session: &CLIAgentSession) -> Option<Self> {
        if session.agent != CLIAgent::Claude || session.is_remote() {
            return None;
        }
        let context = &session.session_context;
        Self::from_persisted(
            context.session_id.clone(),
            context.resume_cwd.clone().or_else(|| context.cwd.clone()),
        )
    }

    /// Builds the command line that resumes this conversation in `shell_type`.
    ///
    /// `extra_flags` is the user's resume-flags setting. It is split on whitespace and any token
    /// outside a conservative charset is quoted, so the setting can add arguments but can never
    /// inject shell syntax.
    pub fn command(&self, extra_flags: &str, shell_type: ShellType) -> String {
        // Fields are public, so re-check here rather than trusting every constructor.
        let session_id = if is_valid_session_id(&self.session_id) {
            self.session_id.clone()
        } else {
            quote_for_shell(&self.session_id, shell_type)
        };
        let mut claude = format!("{CLAUDE_COMMAND} --resume {session_id}");
        for flag in extra_flags.split_whitespace() {
            claude.push(' ');
            if is_plain_word(flag) {
                claude.push_str(flag);
            } else {
                claude.push_str(&quote_for_shell(flag, shell_type));
            }
        }

        match self.cwd.as_deref().filter(|cwd| is_safe_cwd(cwd)) {
            Some(cwd) => {
                let quoted_cwd = quote_for_shell(cwd, shell_type);
                match shell_type {
                    ShellType::PowerShell => {
                        format!("Set-Location -LiteralPath {quoted_cwd}; if ($?) {{ {claude} }}")
                    }
                    // `builtin` skips user `cd` wrappers (zoxide, autoenv) that could print
                    // or prompt; the path is always absolute, so no `--` is needed.
                    ShellType::Zsh | ShellType::Bash | ShellType::Fish => {
                        format!("builtin cd {quoted_cwd} && {claude}")
                    }
                }
            }
            None => claude,
        }
    }
}

impl ClaudeCodeResume {
    /// Whether Claude Code still has the transcript of this conversation, which `--resume` needs.
    /// Touches the filesystem, so call it once at restore time rather than on every render.
    pub fn transcript_exists(&self) -> bool {
        claude_projects_dir().is_some_and(|projects_dir| self.transcript_exists_in(&projects_dir))
    }

    fn transcript_exists_in(&self, projects_dir: &Path) -> bool {
        // The ID becomes a path component, so it must not be able to name another directory.
        if !is_valid_session_id(&self.session_id) {
            return false;
        }
        let file_name = format!("{}.jsonl", self.session_id);
        if let Some(cwd) = self.cwd.as_deref()
            && projects_dir
                .join(project_dir_name(cwd))
                .join(&file_name)
                .is_file()
        {
            return true;
        }
        // Claude Code shortens the directory name of very long paths, so fall back to looking the
        // session up in every project.
        std::fs::read_dir(projects_dir)
            .into_iter()
            .flatten()
            .flatten()
            .any(|entry| entry.path().join(&file_name).is_file())
    }
}

/// `$CLAUDE_CONFIG_DIR/projects`, or `~/.claude/projects` when the variable is unset.
fn claude_projects_dir() -> Option<PathBuf> {
    let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".claude")))?;
    Some(config_dir.join("projects"))
}

/// Claude Code names a project's transcript directory after its path, with every character other
/// than an ASCII letter or digit replaced by `-`.
fn project_dir_name(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Progress of a Claude Code resume restored with a pane.
#[derive(Clone, Debug)]
pub enum PendingClaudeCodeResume {
    AwaitingBootstrap(ClaudeCodeResume),
    /// Shown in the one-click banner; `command` was built for the bootstrapped shell.
    Offered {
        resume: ClaudeCodeResume,
        command: String,
    },
    /// `command` was submitted and the block running it has not completed yet.
    Submitted {
        resume: ClaudeCodeResume,
        command: String,
    },
}

impl PendingClaudeCodeResume {
    pub fn resume(&self) -> &ClaudeCodeResume {
        match self {
            PendingClaudeCodeResume::AwaitingBootstrap(resume)
            | PendingClaudeCodeResume::Offered { resume, .. }
            | PendingClaudeCodeResume::Submitted { resume, .. } => resume,
        }
    }
}

/// Claude Code session IDs are UUIDs. Accepting only this charset keeps the ID safe to place on a
/// command line unquoted and as a path component in any shell.
pub fn is_valid_session_id(session_id: &str) -> bool {
    !session_id.is_empty()
        && session_id.len() <= MAX_SESSION_ID_LEN
        && !session_id.starts_with('-')
        && session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Control characters (newlines in particular) would split the submitted command line, so such
/// paths are dropped instead of quoted.
fn is_safe_cwd(cwd: &str) -> bool {
    Path::new(cwd).is_absolute() && !cwd.chars().any(char::is_control)
}

fn is_plain_word(word: &str) -> bool {
    // zsh expands a leading `=` into the path of the named command (`=cmd`).
    !word.starts_with('=')
        && word.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '-' | '_' | '=' | '.' | ',' | ':' | '/' | '@' | '+')
        })
}

/// Single-quotes `value` as one literal argument for `shell_type`.
///
/// Differs from `shell_quote_arg` in two cases that matter for arbitrary paths: fish also
/// interprets `\\` inside single quotes, and PowerShell also treats the typographic quotes
/// U+2018..U+201B as single-quote delimiters.
fn quote_for_shell(value: &str, shell_type: ShellType) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for c in value.chars() {
        match shell_type {
            ShellType::Zsh | ShellType::Bash => {
                if c == '\'' {
                    quoted.push_str(r#"'"'"'"#);
                } else {
                    quoted.push(c);
                }
            }
            ShellType::Fish => {
                if c == '\'' || c == '\\' {
                    quoted.push('\\');
                }
                quoted.push(c);
            }
            ShellType::PowerShell => {
                if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
                    quoted.push(c);
                }
                quoted.push(c);
            }
        }
    }
    quoted.push('\'');
    quoted
}

#[cfg(test)]
#[path = "claude_code_resume_tests.rs"]
mod tests;
