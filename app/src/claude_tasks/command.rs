//! Builds the shell command lines typed into new panes.
//!
//! Panes run an interactive shell, so the command has to be a single line of shell text. Every
//! piece of user data goes through `quote` as its own argument and is never spliced into the
//! command unquoted. The quoted form means the same in zsh, bash and fish.

use std::path::{Path, PathBuf};

pub const CLAUDE_BINARY: &str = "claude";
pub const SKIP_PERMISSIONS_FLAG: &str = "--dangerously-skip-permissions";

/// Expands a leading `~` in a configured directory.
pub fn expand_dir(dir: &str) -> PathBuf {
    PathBuf::from(shellexpand::tilde(dir.trim()).into_owned())
}

/// Prompts are sent as one line: a raw newline would be submitted by the shell's line editor
/// before the closing quote in some shells.
pub fn single_line_prompt(prompt: &str) -> String {
    prompt
        .split(['\r', '\n'])
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Quotes `word` as one literal argument for zsh, bash and fish.
///
/// `shell_words::quote` is not enough: fish also interprets `\\` inside single quotes. Quotes and
/// backslashes are therefore emitted outside the single-quoted runs as `\'` and `\\`, which all
/// three shells read the same way.
fn quote(word: &str) -> String {
    let mut quoted = String::with_capacity(word.len() + 2);
    quoted.push('\'');
    for c in word.chars() {
        match c {
            '\'' => quoted.push_str(r"'\''"),
            '\\' => quoted.push_str(r"'\\'"),
            c => quoted.push(c),
        }
    }
    quoted.push('\'');
    quoted
}

fn cd_prefix(dir: &Path) -> String {
    format!("cd -- {}", quote(&dir.to_string_lossy()))
}

/// `cd -- <dir> && claude --dangerously-skip-permissions -p -- <prompt>`. The `cd` guard keeps
/// the command from running in the wrong directory if the pane started elsewhere, and `--` keeps
/// a prompt that starts with `-` from being read as an option.
pub fn claude_task_command(dir: &Path, prompt: &str) -> String {
    format!(
        "{} && {CLAUDE_BINARY} {SKIP_PERMISSIONS_FLAG} -p -- {}",
        cd_prefix(dir),
        quote(&single_line_prompt(prompt))
    )
}

/// `cd -- <dir> && <command>`. The preview command is the user's own shell code, so it is kept
/// verbatim; only the directory is quoted.
pub fn preview_command(dir: &Path, command: &str) -> String {
    format!("{} && {}", cd_prefix(dir), command.trim())
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod tests;
