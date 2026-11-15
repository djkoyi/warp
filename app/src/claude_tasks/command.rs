//! Builds the shell command lines typed into new panes.
//!
//! Panes run an interactive shell, so the command has to be a single line of shell text. Every
//! piece of user data goes through `shell_words::quote` as its own argument and is never spliced
//! into the command unquoted. The resulting single-quoted form is valid in zsh, bash and fish.

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

fn cd_prefix(dir: &Path) -> String {
    shell_words::join(["cd", "--", &dir.to_string_lossy()])
}

/// `cd -- <dir> && claude --dangerously-skip-permissions -p <prompt>`. The `cd` guard keeps the
/// command from running in the wrong directory if the pane started elsewhere.
pub fn claude_task_command(dir: &Path, prompt: &str) -> String {
    let claude = shell_words::join([
        CLAUDE_BINARY,
        SKIP_PERMISSIONS_FLAG,
        "-p",
        &single_line_prompt(prompt),
    ]);
    format!("{} && {claude}", cd_prefix(dir))
}

/// `cd -- <dir> && <command>`. The preview command is the user's own shell code, so it is kept
/// verbatim; only the directory is quoted.
pub fn preview_command(dir: &Path, command: &str) -> String {
    format!("{} && {}", cd_prefix(dir), command.trim())
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod tests;
