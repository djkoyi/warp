//! Reloading live Claude Code sessions in place (`/exit` + `claude --resume`), so a newly added
//! MCP server, skill or plugin is picked up without losing the conversation or the pane.

use std::collections::BTreeMap;
use std::time::Duration;

use super::CLIAgent;
use super::claude_code_resume::ClaudeCodeResume;
use super::cli_agent_sessions::{BlockedSource, CLIAgentSession, CLIAgentSessionStatus};

/// Interrupts a turn, dismisses a prompt, or clears a draft so `/exit` lands on an empty input.
pub const CTRL_C: &[u8] = &[0x03];
pub const EXIT_COMMAND: &[u8] = b"/exit";
/// Sent separately from [`EXIT_COMMAND`]: Claude Code treats a burst of input as a paste, in
/// which a carriage return is a newline rather than a submit.
pub const SUBMIT: &[u8] = b"\r";

/// Time for an interrupted turn or dismissed prompt to settle before the input is cleared.
pub const FORCE_INTERRUPT_SETTLE: Duration = Duration::from_millis(1000);
/// Gap between the keystrokes of the exit sequence.
pub const KEYSTROKE_GAP: Duration = Duration::from_millis(250);
/// How long Claude Code gets to exit after `/exit` before the Ctrl-C fallback.
pub const EXIT_TIMEOUT: Duration = Duration::from_secs(4);
/// Three presses: the first clears any text left in the input, the next two are Claude Code's
/// "press Ctrl-C again to exit".
pub const FALLBACK_CTRL_C_PRESSES: usize = 3;
/// How long the Ctrl-C fallback gets before the reload is abandoned.
pub const FALLBACK_TIMEOUT: Duration = Duration::from_secs(4);

/// What a Claude Code session is doing, as far as reloading it is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaudeCodeActivity {
    /// At its prompt, waiting for the next message.
    Idle,
    Working,
    /// Showing a permission prompt or a question; typed keys would answer it.
    AwaitingUser,
}

pub fn claude_code_activity(
    session: &CLIAgentSession,
    has_seen_prompt_submit: bool,
) -> ClaudeCodeActivity {
    match &session.status {
        // Sessions start out `InProgress` before any turn has run.
        CLIAgentSessionStatus::InProgress if !has_seen_prompt_submit => ClaudeCodeActivity::Idle,
        CLIAgentSessionStatus::InProgress => ClaudeCodeActivity::Working,
        CLIAgentSessionStatus::Success
        | CLIAgentSessionStatus::Failed { .. }
        | CLIAgentSessionStatus::Cancelled => ClaudeCodeActivity::Idle,
        CLIAgentSessionStatus::Blocked { source, .. } => match source {
            BlockedSource::PermissionRequest
            | BlockedSource::QuestionAsked
            | BlockedSource::NeedsInput => ClaudeCodeActivity::AwaitingUser,
        },
    }
}

/// A reload a pane has already started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaudeCodeReloadProgress {
    /// Waiting for the session to go idle.
    Deferred,
    /// The exit sequence has been typed; waiting for the Claude Code block to finish.
    Exiting,
}

/// Everything about one terminal pane that decides whether its Claude Code session can reload.
pub struct ClaudeCodeReloadPane<'a> {
    /// The pane's active long-running block was detected as a Claude Code command.
    pub claude_running: bool,
    pub is_shared_session_viewer: bool,
    /// The active block runs in the pane's own local login shell (not SSH, not a subshell).
    pub is_local_login_shell: bool,
    pub session: Option<&'a CLIAgentSession>,
    pub has_seen_prompt_submit: bool,
    pub progress: Option<ClaudeCodeReloadProgress>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ClaudeCodeReloadSkipReason {
    SharedSession,
    RemoteOrSubshell,
    /// A plugin reported a Claude session but the pane is not running a Claude command, so the
    /// report is not trusted.
    NotVerified,
    NoSessionId,
    TranscriptMissing,
}

impl ClaudeCodeReloadSkipReason {
    pub fn label(self) -> &'static str {
        match self {
            ClaudeCodeReloadSkipReason::SharedSession => "sesión compartida",
            ClaudeCodeReloadSkipReason::RemoteOrSubshell => "SSH/subshell",
            ClaudeCodeReloadSkipReason::NotVerified => "Claude no verificado",
            ClaudeCodeReloadSkipReason::NoSessionId => "sin session_id",
            ClaudeCodeReloadSkipReason::TranscriptMissing => "sin transcript",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClaudeCodeReloadDecision {
    /// The pane has no Claude Code session; it is left out of the summary.
    NotApplicable,
    AlreadyExiting,
    ReloadNow(ClaudeCodeResume),
    ReloadWhenIdle(ClaudeCodeResume),
    Skip(ClaudeCodeReloadSkipReason),
}

/// Decides what to do with one pane. `force` reloads busy sessions immediately.
/// `transcript_exists` touches the filesystem, so it only runs for otherwise reloadable panes.
pub fn decide_claude_code_reload(
    pane: &ClaudeCodeReloadPane,
    force: bool,
    transcript_exists: impl FnOnce(&ClaudeCodeResume) -> bool,
) -> ClaudeCodeReloadDecision {
    if pane.progress == Some(ClaudeCodeReloadProgress::Exiting) {
        return ClaudeCodeReloadDecision::AlreadyExiting;
    }
    let claude_session = pane
        .session
        .filter(|session| session.agent == CLIAgent::Claude);
    if !pane.claude_running && claude_session.is_none() {
        return ClaudeCodeReloadDecision::NotApplicable;
    }
    if pane.is_shared_session_viewer {
        return ClaudeCodeReloadDecision::Skip(ClaudeCodeReloadSkipReason::SharedSession);
    }
    if !pane.is_local_login_shell || claude_session.is_some_and(CLIAgentSession::is_remote) {
        return ClaudeCodeReloadDecision::Skip(ClaudeCodeReloadSkipReason::RemoteOrSubshell);
    }
    if !pane.claude_running {
        return ClaudeCodeReloadDecision::Skip(ClaudeCodeReloadSkipReason::NotVerified);
    }
    let Some((session, resume)) = claude_session.and_then(|session| {
        ClaudeCodeResume::from_live_session(session).map(|resume| (session, resume))
    }) else {
        return ClaudeCodeReloadDecision::Skip(ClaudeCodeReloadSkipReason::NoSessionId);
    };
    if !transcript_exists(&resume) {
        return ClaudeCodeReloadDecision::Skip(ClaudeCodeReloadSkipReason::TranscriptMissing);
    }
    match claude_code_activity(session, pane.has_seen_prompt_submit) {
        ClaudeCodeActivity::Idle => ClaudeCodeReloadDecision::ReloadNow(resume),
        ClaudeCodeActivity::Working | ClaudeCodeActivity::AwaitingUser if force => {
            ClaudeCodeReloadDecision::ReloadNow(resume)
        }
        ClaudeCodeActivity::Working | ClaudeCodeActivity::AwaitingUser => {
            ClaudeCodeReloadDecision::ReloadWhenIdle(resume)
        }
    }
}

/// Tally of one reload request across panes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClaudeCodeReloadSummary {
    pub reloaded: usize,
    pub pending: usize,
    pub skipped: BTreeMap<ClaudeCodeReloadSkipReason, usize>,
}

impl ClaudeCodeReloadSummary {
    pub fn record(&mut self, decision: &ClaudeCodeReloadDecision) {
        match decision {
            ClaudeCodeReloadDecision::NotApplicable => {}
            ClaudeCodeReloadDecision::AlreadyExiting | ClaudeCodeReloadDecision::ReloadNow(_) => {
                self.reloaded += 1
            }
            ClaudeCodeReloadDecision::ReloadWhenIdle(_) => self.pending += 1,
            ClaudeCodeReloadDecision::Skip(reason) => {
                *self.skipped.entry(*reason).or_default() += 1
            }
        }
    }

    pub fn skipped_count(&self) -> usize {
        self.skipped.values().sum()
    }

    pub fn is_empty(&self) -> bool {
        self.reloaded == 0 && self.pending == 0 && self.skipped.is_empty()
    }

    pub fn message(&self) -> String {
        if self.is_empty() {
            return "No hay sesiones de Claude Code en ejecución.".to_owned();
        }
        let mut message = format!(
            "Recargadas {} · pendientes {} · omitidas {}",
            self.reloaded,
            self.pending,
            self.skipped_count()
        );
        if !self.skipped.is_empty() {
            let reasons: Vec<String> = self
                .skipped
                .iter()
                .map(|(reason, count)| format!("{count} {}", reason.label()))
                .collect();
            message.push_str(&format!(" ({})", reasons.join(", ")));
        }
        message
    }
}

#[cfg(test)]
#[path = "claude_code_reload_tests.rs"]
mod tests;
