use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use warpui::r#async::Timer;
use warpui::{SingletonEntity as _, ViewContext};

use super::TerminalView;
use crate::features::FeatureFlag;
use crate::terminal::CLIAgent;
use crate::terminal::claude_code_reload::{
    CTRL_C, ClaudeCodeActivity, ClaudeCodeReloadDecision, ClaudeCodeReloadPane,
    ClaudeCodeReloadProgress, EXIT_COMMAND, EXIT_TIMEOUT, FALLBACK_CTRL_C_PRESSES,
    FALLBACK_TIMEOUT, FORCE_INTERRUPT_SETTLE, KEYSTROKE_GAP, SUBMIT, claude_code_activity,
    decide_claude_code_reload,
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
    Deferred,
    Exiting {
        /// Built for the shell Claude Code was verified to run in.
        command: String,
        /// Keys are only typed while this block is still the running Claude Code command.
        claude_block_id: BlockId,
    },
}

impl ClaudeCodeReload {
    fn progress(&self) -> ClaudeCodeReloadProgress {
        match self.stage {
            ClaudeCodeReloadStage::Deferred => ClaudeCodeReloadProgress::Deferred,
            ClaudeCodeReloadStage::Exiting { .. } => ClaudeCodeReloadProgress::Exiting,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum ExitStep {
    Interrupt,
    ClearInput,
    TypeExit,
    Submit,
    FallbackCtrlC { presses_left: usize },
    GiveUp,
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
    /// Reloads this pane's Claude Code session (`/exit`, then `claude --resume` in the same pane)
    /// so it picks up newly installed MCP servers, skills and plugins. A busy session is reloaded
    /// once it goes idle, unless `force` is set.
    pub fn reload_claude_code_session(
        &mut self,
        force: bool,
        ctx: &mut ViewContext<Self>,
    ) -> ClaudeCodeReloadDecision {
        if !FeatureFlag::ClaudeCodeSessionResume.is_enabled() {
            return ClaudeCodeReloadDecision::NotApplicable;
        }
        let snapshot = self.claude_code_reload_snapshot(ctx);
        let (decision, activity) = {
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
            let decision =
                decide_claude_code_reload(&pane, force, ClaudeCodeResume::transcript_exists);
            let activity =
                session.map(|session| claude_code_activity(session, has_seen_prompt_submit));
            (decision, activity)
        };

        match &decision {
            ClaudeCodeReloadDecision::ReloadNow(resume) => {
                let shell_type = snapshot
                    .local_login_shell_type
                    .expect("a reloadable pane runs in a local login shell");
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
                    },
                    token,
                });
                let first_step = match activity {
                    Some(ClaudeCodeActivity::Idle) => ExitStep::ClearInput,
                    Some(ClaudeCodeActivity::Working | ClaudeCodeActivity::AwaitingUser) | None => {
                        ExitStep::Interrupt
                    }
                };
                // Status notifications arrive just before Claude Code redraws its prompt.
                self.schedule_claude_code_exit_step(token, first_step, KEYSTROKE_GAP, ctx);
            }
            ClaudeCodeReloadDecision::ReloadWhenIdle(resume) => {
                if !matches!(
                    self.claude_code_reload,
                    Some(ClaudeCodeReload {
                        stage: ClaudeCodeReloadStage::Deferred,
                        ..
                    })
                ) {
                    self.claude_code_reload = Some(ClaudeCodeReload {
                        resume: resume.clone(),
                        stage: ClaudeCodeReloadStage::Deferred,
                        token: NEXT_RELOAD_TOKEN.fetch_add(1, Ordering::Relaxed),
                    });
                }
            }
            ClaudeCodeReloadDecision::AlreadyExiting => {}
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

    pub(super) fn on_claude_code_status_changed_for_reload(&mut self, ctx: &mut ViewContext<Self>) {
        if self.is_claude_code_reload_pending() {
            self.reload_claude_code_session(false, ctx);
        }
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
        ctx: &mut ViewContext<Self>,
    ) {
        let Some(reload) = self.claude_code_reload.take() else {
            return;
        };
        let ClaudeCodeReloadStage::Exiting { command, .. } = reload.stage else {
            // A deferred session that exited on its own is not resumed behind the user's back.
            self.sync_claude_code_reload_marker(ctx);
            return;
        };
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
                    claude_block_id, ..
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

        if matches!(step, ExitStep::GiveUp) {
            log::warn!("Claude Code did not exit for a reload; leaving the session running");
            self.claude_code_reload = None;
            return;
        }
        // Re-checked before every keystroke: the user may have quit Claude Code or started
        // something else in the meantime.
        if !self.is_claude_code_still_running(&claude_block_id, ctx) {
            return;
        }

        let (bytes, next_step, delay) = match step {
            ExitStep::Interrupt => (CTRL_C, ExitStep::ClearInput, FORCE_INTERRUPT_SETTLE),
            ExitStep::ClearInput => (CTRL_C, ExitStep::TypeExit, KEYSTROKE_GAP),
            ExitStep::TypeExit => (EXIT_COMMAND, ExitStep::Submit, KEYSTROKE_GAP),
            ExitStep::Submit => (
                SUBMIT,
                ExitStep::FallbackCtrlC {
                    presses_left: FALLBACK_CTRL_C_PRESSES,
                },
                EXIT_TIMEOUT,
            ),
            ExitStep::FallbackCtrlC { presses_left } => match presses_left.saturating_sub(1) {
                0 => (CTRL_C, ExitStep::GiveUp, FALLBACK_TIMEOUT),
                presses_left => (
                    CTRL_C,
                    ExitStep::FallbackCtrlC { presses_left },
                    KEYSTROKE_GAP,
                ),
            },
            ExitStep::GiveUp => return,
        };
        if !self.write_user_bytes_to_pty(bytes, ctx) {
            // The block is under agent control; typing into it is not ours to do.
            self.claude_code_reload = None;
            return;
        }
        self.schedule_claude_code_exit_step(token, next_step, delay, ctx);
    }
}
