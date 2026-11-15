//! Reloading live Claude Code sessions in place (exit with Ctrl-C, then `claude --resume`), so a
//! newly added MCP server, skill or plugin is picked up without losing the conversation or the
//! pane.

use std::collections::BTreeMap;
use std::time::Duration;

use super::CLIAgent;
use super::claude_code_resume::ClaudeCodeResume;
use super::cli_agent_sessions::{BlockedSource, CLIAgentSession, CLIAgentSessionStatus};

/// The only key the reload ever types. Printable text is never written: if the session is not
/// where we think it is, text could land in a prompt, a permission dialog or a shell.
pub const CTRL_C: &[u8] = &[0x03];

/// Time for an interrupted turn or dismissed prompt to settle before the input is cleared.
pub const FORCE_INTERRUPT_SETTLE: Duration = Duration::from_millis(1000);
/// Gap between the keystrokes of the exit sequence.
pub const KEYSTROKE_GAP: Duration = Duration::from_millis(250);
/// How long Claude Code gets to exit after the last Ctrl-C before the reload is abandoned.
pub const EXIT_TIMEOUT: Duration = Duration::from_secs(4);

/// One step of the exit sequence. Every step types a single Ctrl-C.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitStep {
    /// Forced reloads only: interrupts a turn or dismisses a prompt.
    Interrupt,
    /// Clears any draft left in the input.
    ClearInput,
    /// Claude Code's "press Ctrl-C again to exit".
    Arm,
    Exit,
    /// Claude Code did not exit in time.
    GiveUp,
}

impl ExitStep {
    pub fn first(activity: Option<ClaudeCodeActivity>) -> Self {
        match activity {
            Some(ClaudeCodeActivity::Idle) => ExitStep::ClearInput,
            Some(
                ClaudeCodeActivity::Working
                | ClaudeCodeActivity::AwaitingUser
                | ClaudeCodeActivity::Unknown,
            )
            | None => ExitStep::Interrupt,
        }
    }

    /// The bytes this step types, the step that follows and the delay before it. `None` for
    /// [`ExitStep::GiveUp`], which types nothing.
    pub fn plan(self) -> Option<(&'static [u8], ExitStep, Duration)> {
        match self {
            ExitStep::Interrupt => Some((CTRL_C, ExitStep::ClearInput, FORCE_INTERRUPT_SETTLE)),
            ExitStep::ClearInput => Some((CTRL_C, ExitStep::Arm, KEYSTROKE_GAP)),
            ExitStep::Arm => Some((CTRL_C, ExitStep::Exit, KEYSTROKE_GAP)),
            ExitStep::Exit => Some((CTRL_C, ExitStep::GiveUp, EXIT_TIMEOUT)),
            ExitStep::GiveUp => None,
        }
    }
}

/// What to do right before typing a keystroke of a non-forced reload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitKeystrokeCheck {
    Type,
    /// The session got busy again; wait for it to go idle.
    Defer,
    /// The session is gone; stop.
    Abandon,
}

/// A non-forced reload only types while the session is idle, re-checked before every keystroke
/// because the user may have started a turn in between.
pub fn check_exit_keystroke(
    force: bool,
    activity: Option<ClaudeCodeActivity>,
) -> ExitKeystrokeCheck {
    match activity {
        _ if force => ExitKeystrokeCheck::Type,
        Some(ClaudeCodeActivity::Idle) => ExitKeystrokeCheck::Type,
        Some(
            ClaudeCodeActivity::Working
            | ClaudeCodeActivity::AwaitingUser
            | ClaudeCodeActivity::Unknown,
        ) => ExitKeystrokeCheck::Defer,
        None => ExitKeystrokeCheck::Abandon,
    }
}

/// Error toast for a pane whose session could not be made to exit.
pub fn reload_failed_message(pane_label: &str) -> String {
    format!("Claude no salió en {pane_label}; sesión sin recargar")
}

/// What a Claude Code session is doing, as far as reloading it is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaudeCodeActivity {
    /// At its prompt, waiting for the next message.
    Idle,
    Working,
    /// Showing a permission prompt or a question; typed keys would answer it.
    AwaitingUser,
    /// The session reports no fine-grained status (no rich plugin notifications), so whether it
    /// is idle cannot be told.
    Unknown,
}

pub fn claude_code_activity(
    session: &CLIAgentSession,
    has_seen_prompt_submit: bool,
) -> ClaudeCodeActivity {
    if !session.supports_rich_status() {
        return ClaudeCodeActivity::Unknown;
    }
    match &session.status {
        // Rich-status sessions start out `InProgress` before any turn has run.
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
    /// No rich status, so the session may be mid-turn; only a forced reload interrupts it.
    UnknownStatus,
}

impl ClaudeCodeReloadSkipReason {
    pub fn label(self) -> &'static str {
        match self {
            ClaudeCodeReloadSkipReason::SharedSession => "sesión compartida",
            ClaudeCodeReloadSkipReason::RemoteOrSubshell => "SSH/subshell",
            ClaudeCodeReloadSkipReason::NotVerified => "Claude no verificado",
            ClaudeCodeReloadSkipReason::NoSessionId => "sin session_id",
            ClaudeCodeReloadSkipReason::TranscriptMissing => "sin transcript",
            ClaudeCodeReloadSkipReason::UnknownStatus => "estado desconocido",
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
        ClaudeCodeActivity::Working
        | ClaudeCodeActivity::AwaitingUser
        | ClaudeCodeActivity::Unknown
            if force =>
        {
            ClaudeCodeReloadDecision::ReloadNow(resume)
        }
        ClaudeCodeActivity::Working | ClaudeCodeActivity::AwaitingUser => {
            ClaudeCodeReloadDecision::ReloadWhenIdle(resume)
        }
        ClaudeCodeActivity::Unknown => {
            ClaudeCodeReloadDecision::Skip(ClaudeCodeReloadSkipReason::UnknownStatus)
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
            "Recargando {} · pendientes {} · omitidas {}",
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
