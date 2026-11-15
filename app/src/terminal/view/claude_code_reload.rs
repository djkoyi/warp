use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use warpui::r#async::Timer;
use warpui::{SingletonEntity as _, ViewContext};

use super::TerminalView;
use crate::features::FeatureFlag;
use crate::terminal::CLIAgent;
use crate::terminal::claude_code_reload::{
    ClaudeCodeReloadDecision, ClaudeCodeReloadPane, ClaudeCodeReloadProgress,
    ClaudeCodeReloadSkipReason, ExitKeystrokeCheck, ExitStep, KEYSTROKE_GAP,
    check_exit_keystroke, claude_code_activity, decide_claude_code_reload,
    reload_failed_message,
};
use crate::terminal::claude_code_resume::{ClaudeCodeResume, PendingClaudeCodeResume};
use crate::terminal::cli_agent_sessions::CLIAgentSessionsModel;
use crate::terminal::general_settings::GeneralSettings;
use crate::terminal::model::block::BlockId;
use crate::terminal::model::session::SessionType;
use crate::terminal::shell::ShellType;

/// Source of [`ClaudeCodeReload::token`] values; never reused, so a timer left over from an
/// earlier reload can never act on a newer one.
static NEXT_RELOAD_TOKEN: AtomicU64 = AtomicU64::new(0);

/// A reload of this pane's Claude Code session that has not handed over to `--resume` yet.
pub(super) struct ClaudeCodeReload {
    resume: ClaudeCodeResume,
    stage: ClaudeCodeReloadStage,
    token: u64,
}

enum ClaudeCodeReloadStage {
    /// Only entered once the transcript was found, so re-deciding on every status change does
    /// not re-read the transcript directories.
    Deferred,
    Exiting {
        /// Built for the shell Claude Code was verified to run in.
        command: String,
        /// Keys are only typed while this block is still the running Claude Code command.
        claude_block_id: BlockId,
        /// A forced reload types through a busy session; otherwise every keystroke waits for
        /// the session to be idle.
        force: bool,
    },
}

impl ClaudeCodeReload {
    fn progress(&self) -> ClaudeCodeReloadProgress {
        match self.stage {
            ClaudeCodeReloadStage::Deferred => ClaudeCodeReloadProgress::Deferred,
            ClaudeCodeReloadStage::Exiting { .. } => ClaudeCodeReloadProgress::Exiting,
        }
    }

    fn deferred(resume: ClaudeCodeResume) -> Self {
        Self {
            resume,
            stage: ClaudeCodeReloadStage::Deferred,
            token: NEXT_RELOAD_TOKEN.fetch_add(1, Ordering::Relaxed),
        }
    }
}

/// State read from the pane in one pass, so the terminal model lock is held only briefly.
struct ReloadSnapshot {
    claude_running: bool,
    is_shared_session_viewer: bool,
    active_block_id: BlockId,
    /// Set when the active block runs in the pane's own local login shell (not SSH, not a
    /// subshell).
    local_login_shell_type: Option<ShellType>,
}

impl TerminalView {
    /// Reloads this pane's Claude Code session (exit with Ctrl-C, then `claude --resume` in the
    /// same pane) so it picks up newly installed MCP servers, skills and plugins. A busy session
    /// is reloaded once it goes idle, unless `force` is set.
    pub fn reload_claude_code_session(
        &mut self,
        force: bool,
        ctx: &mut ViewContext<Self>,
    ) -> ClaudeCodeReloadDecision {
        if !FeatureFlag::ClaudeCodeSessionResume.is_enabled() {
            return ClaudeCodeReloadDecision::NotApplicable;
        }
        let snapshot = self.claude_code_reload_snapshot(ctx);
        let deferred_resume = match &self.claude_code_reload {
            Some(ClaudeCodeReload {
                resume,
                stage: ClaudeCodeReloadStage::Deferred,
                ..
            }) => Some(resume.clone()),
            _ => None,
        };
        let (mut decision, activity) = {
            let sessions = CLIAgentSessionsModel::as_ref(ctx);
            let session = sessions.session(self.view_id);
            let has_seen_prompt_submit = sessions.has_seen_prompt_submit(self.view_id);
            let pane = ClaudeCodeReloadPane {
                claude_running: snapshot.claude_running,
                is_shared_session_viewer: snapshot.is_shared_session_viewer,
                is_local_login_shell: snapshot.local_login_shell_type.is_some(),
                session,
                has_seen_prompt_submit,
                progress: self
                    .claude_code_reload
                    .as_ref()
                    .map(ClaudeCodeReload::progress),
            };
            let decision = decide_claude_code_reload(&pane, force, |resume| {
                deferred_resume.as_ref() == Some(resume) || resume.transcript_exists()
            });
            let activity =
                session.map(|session| claude_code_activity(session, has_seen_prompt_submit));
            (decision, activity)
        };

        if let ClaudeCodeReloadDecision::ReloadNow(resume) = &decision {
            if let Some(shell_type) = snapshot.local_login_shell_type {
                let command = resume.command(
                    GeneralSettings::as_ref(ctx)
                        .claude_code_resume_flags
                        .value(),
                    shell_type,
                );
                let token = NEXT_RELOAD_TOKEN.fetch_add(1, Ordering::Relaxed);
                self.claude_code_reload = Some(ClaudeCodeReload {
                    resume: resume.clone(),
                    stage: ClaudeCodeReloadStage::Exiting {
                        command,
                        claude_block_id: snapshot.active_block_id,
                        force,
                    },
                    token,
                });
                // Status notifications arrive just before Claude Code redraws its prompt.
                self.schedule_claude_code_exit_step(
                    token,
                    ExitStep::first(activity),
                    KEYSTROKE_GAP,
                    ctx,
                );
            } else {
                decision =
                    ClaudeCodeReloadDecision::Skip(ClaudeCodeReloadSkipReason::RemoteOrSubshell);
            }
        }
        match &decision {
            ClaudeCodeReloadDecision::ReloadNow(_) | ClaudeCodeReloadDecision::AlreadyExiting => {}
            ClaudeCodeReloadDecision::ReloadWhenIdle(resume) => {
                if deferred_resume.as_ref() != Some(resume) {
                    self.claude_code_reload = Some(ClaudeCodeReload::deferred(resume.clone()));
                }
            }
            ClaudeCodeReloadDecision::NotApplicable | ClaudeCodeReloadDecision::Skip(_) => {
                self.claude_code_reload = None;
            }
        }
        self.sync_claude_code_reload_marker(ctx);
        decision
    }

    /// Whether a reload is waiting for this pane's Claude Code session to go idle.
    pub fn is_claude_code_reload_pending(&self) -> bool {
        matches!(
            self.claude_code_reload,
            Some(ClaudeCodeReload {
                stage: ClaudeCodeReloadStage::Deferred,
                ..
            })
        )
    }

    /// The conversation a reload in progress is about to resume, so quitting Warp between the
    /// exit and the hand-off still resumes it.
    pub(super) fn claude_code_reload_resume_to_persist(&self) -> Option<&ClaudeCodeResume> {
        match &self.claude_code_reload {
            Some(ClaudeCodeReload {
                resume,
                stage: ClaudeCodeReloadStage::Exiting { .. },
                ..
            }) => Some(resume),
            _ => None,
        }
    }

    /// A deferred reload is not typed into the pane the user is looking at; it waits until the
    /// pane loses focus (or the user forces it).
    fn retry_deferred_claude_code_reload(&mut self, ctx: &mut ViewContext<Self>) {
        if self.is_claude_code_reload_pending() && !ctx.is_self_or_child_focused() {
            self.reload_claude_code_session(false, ctx);
        }
    }

    pub(super) fn on_claude_code_status_changed_for_reload(&mut self, ctx: &mut ViewContext<Self>) {
        self.retry_deferred_claude_code_reload(ctx);
    }

    pub(super) fn on_blur_for_claude_code_reload(&mut self, ctx: &mut ViewContext<Self>) {
        self.retry_deferred_claude_code_reload(ctx);
    }

    /// A Claude Code session that ended on its own no longer needs reloading.
    pub(super) fn on_claude_code_session_ended_for_reload(&mut self, ctx: &mut ViewContext<Self>) {
        if self.is_claude_code_reload_pending() {
            self.claude_code_reload = None;
            self.sync_claude_code_reload_marker(ctx);
        }
    }

    fn sync_claude_code_reload_marker(&self, ctx: &mut ViewContext<Self>) {
        let pending = self.is_claude_code_reload_pending();
        let terminal_view_id = self.view_id;
        CLIAgentSessionsModel::handle(ctx).update(ctx, |sessions, ctx| {
            sessions.set_reload_pending(terminal_view_id, pending, ctx);
        });
    }

    /// Hands an exited session over to `--resume`. Must run when a user block completes, before
    /// the resume record is saved.
    pub(super) fn on_user_block_completed_for_claude_code_reload(
        &mut self,
        completed_block_id: &BlockId,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(reload) = self.claude_code_reload.take() else {
            return;
        };
        // Some other block (e.g. a command typed ahead) finished; Claude Code is still exiting.
        let is_other_block = matches!(
            &reload.stage,
            ClaudeCodeReloadStage::Exiting { claude_block_id, .. }
                if claude_block_id != completed_block_id
        );
        if is_other_block {
            self.claude_code_reload = Some(reload);
            return;
        }
        let ClaudeCodeReloadStage::Exiting { command, .. } = reload.stage else {
            // A deferred session that exited on its own is not resumed behind the user's back.
            self.sync_claude_code_reload_marker(ctx);
            return;
        };
        if matches!(
            self.pending_claude_code_resume,
            Some(PendingClaudeCodeResume::Offered { .. })
        ) {
            self.claude_code_resume_banner = None;
            ctx.notify();
        }
        self.pending_claude_code_resume = Some(PendingClaudeCodeResume::Submitted {
            resume: reload.resume,
            command: command.clone(),
        });
        // Submitted by the delayed pending-command scheduler once the shell prompt is ready.
        self.set_pending_command(&command, ctx);
    }

    fn claude_code_reload_snapshot(&self, ctx: &ViewContext<Self>) -> ReloadSnapshot {
        let (claude_running, is_shared_session_viewer, active_block_id) = {
            let model = self.model.lock();
            (
                matches!(
                    self.detect_cli_agent_from_model(&model, ctx),
                    Some((CLIAgent::Claude, _))
                ),
                model.is_shared_session_viewer(),
                model.block_list().active_block().id().clone(),
            )
        };
        let local_login_shell_type = self
            .active_block_session_id()
            .and_then(|session_id| self.sessions.as_ref(ctx).get(session_id))
            .filter(|session| {
                matches!(session.session_type(), SessionType::Local)
                    && session.subshell_info().is_none()
            })
            .map(|session| session.shell().shell_type());
        ReloadSnapshot {
            claude_running,
            is_shared_session_viewer,
            active_block_id,
            local_login_shell_type,
        }
    }

    /// Whether `claude_block_id` is still the active block and still runs Claude Code.
    fn is_claude_code_still_running(
        &self,
        claude_block_id: &BlockId,
        ctx: &ViewContext<Self>,
    ) -> bool {
        let model = self.model.lock();
        model.block_list().active_block().id() == claude_block_id
            && !model.is_shared_session_viewer()
            && matches!(
                self.detect_cli_agent_from_model(&model, ctx),
                Some((CLIAgent::Claude, _))
            )
    }

    fn schedule_claude_code_exit_step(
        &self,
        token: u64,
        step: ExitStep,
        delay: Duration,
        ctx: &mut ViewContext<Self>,
    ) {
        let _ = ctx.spawn(
            async move { Timer::after(delay).await },
            move |me, _, ctx| me.run_claude_code_exit_step(token, step, ctx),
        );
    }

    fn run_claude_code_exit_step(
        &mut self,
        token: u64,
        step: ExitStep,
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(ClaudeCodeReload {
            stage:
                ClaudeCodeReloadStage::Exiting {
                    claude_block_id,
                    force,
                    ..
                },
            token: current_token,
            ..
        }) = &self.claude_code_reload
        else {
            return;
        };
        if *current_token != token {
            return;
        }
        let claude_block_id = claude_block_id.clone();
        let force = *force;

        let Some((bytes, next_step, delay)) = step.plan() else {
            log::warn!("Claude Code did not exit for a reload; leaving the session running");
            self.fail_claude_code_reload(ctx);
            return;
        };
        // Re-checked before every keystroke: the user may have quit Claude Code or started
        // something else in the meantime.
        if !self.is_claude_code_still_running(&claude_block_id, ctx) {
            self.claude_code_reload = None;
            return;
        }
        let activity = {
            let sessions = CLIAgentSessionsModel::as_ref(ctx);
            sessions.session(self.view_id).map(|session| {
                claude_code_activity(session, sessions.has_seen_prompt_submit(self.view_id))
            })
        };
        match check_exit_keystroke(force, activity) {
            ExitKeystrokeCheck::Type => {}
            ExitKeystrokeCheck::Defer => {
                if let Some(reload) = self.claude_code_reload.take() {
                    self.claude_code_reload = Some(ClaudeCodeReload::deferred(reload.resume));
                }
                self.sync_claude_code_reload_marker(ctx);
                return;
            }
            ExitKeystrokeCheck::Abandon => {
                self.claude_code_reload = None;
                self.sync_claude_code_reload_marker(ctx);
                return;
            }
        }
        if !self.write_claude_code_reload_bytes(bytes, ctx) {
            // The block is under agent control; typing into it is not ours to do.
            self.fail_claude_code_reload(ctx);
            return;
        }
        self.schedule_claude_code_exit_step(token, next_step, delay, ctx);
    }

    /// Writes the reload's keystrokes straight to this pane's PTY. Unlike user typing, they are
    /// never replicated to other panes when input sync is on.
    fn write_claude_code_reload_bytes(
        &mut self,
        bytes: &'static [u8],
        ctx: &mut ViewContext<Self>,
    ) -> bool {
        let is_agent_in_control = self
            .model
            .lock()
            .block_list()
            .active_block()
            .is_agent_in_control();
        if is_agent_in_control {
            return false;
        }
        self.write_to_pty(bytes, ctx);
        true
    }

    fn fail_claude_code_reload(&mut self, ctx: &mut ViewContext<Self>) {
        self.claude_code_reload = None;
        self.sync_claude_code_reload_marker(ctx);
        let title = {
            let model = self.model.lock();
            model.custom_title().or_else(|| model.terminal_title())
        };
        let pane_label = title
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| "una pestaña".to_owned());
        self.show_error_toast(reload_failed_message(&pane_label), ctx);
    }
}
