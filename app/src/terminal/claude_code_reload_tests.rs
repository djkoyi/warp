use std::time::Duration;

use super::{
    CTRL_C, ClaudeCodeActivity, ClaudeCodeReloadDecision, ClaudeCodeReloadPane,
    ClaudeCodeReloadProgress, ClaudeCodeReloadSkipReason, ClaudeCodeReloadSummary, EXIT_TIMEOUT,
    ExitKeystrokeCheck, ExitStep, FORCE_INTERRUPT_SETTLE, KEYSTROKE_GAP, check_exit_keystroke,
    claude_code_activity, decide_claude_code_reload, reload_failed_message,
};
use crate::terminal::CLIAgent;
use crate::terminal::claude_code_resume::ClaudeCodeResume;
use crate::terminal::cli_agent_sessions::{
    BlockedSource, CLIAgentInputState, CLIAgentSession, CLIAgentSessionContext,
    CLIAgentSessionStatus,
};

const SESSION_ID: &str = "0f6c1a2b-3c4d-4e5f-8a9b-0c1d2e3f4a5b";
const PROJECT: &str = "/Users/dev/proyecto";

fn session(status: CLIAgentSessionStatus) -> CLIAgentSession {
    CLIAgentSession {
        agent: CLIAgent::Claude,
        status,
        session_context: CLIAgentSessionContext {
            cwd: Some("/Users/dev/proyecto/subdir".to_owned()),
            resume_cwd: Some(PROJECT.to_owned()),
            session_id: Some(SESSION_ID.to_owned()),
            ..Default::default()
        },
        input_state: CLIAgentInputState::Closed,
        should_auto_toggle_input: false,
        listener: None,
        plugin_version: None,
        remote_host: None,
        draft_text: None,
        custom_command_prefix: None,
        received_rich_notification: true,
    }
}

fn blocked(source: BlockedSource) -> CLIAgentSessionStatus {
    CLIAgentSessionStatus::Blocked {
        message: None,
        source,
    }
}

fn pane(session: Option<&CLIAgentSession>) -> ClaudeCodeReloadPane<'_> {
    ClaudeCodeReloadPane {
        claude_running: true,
        is_shared_session_viewer: false,
        is_local_login_shell: true,
        session,
        has_seen_prompt_submit: true,
        progress: None,
    }
}

fn expected_resume() -> ClaudeCodeResume {
    ClaudeCodeResume {
        session_id: SESSION_ID.to_owned(),
        cwd: Some(PROJECT.to_owned()),
    }
}

fn decide(pane: &ClaudeCodeReloadPane, force: bool) -> ClaudeCodeReloadDecision {
    decide_claude_code_reload(pane, force, |_| true)
}

#[test]
fn idle_sessions_reload_now_from_the_pinned_cwd() {
    for status in [
        CLIAgentSessionStatus::Success,
        CLIAgentSessionStatus::Cancelled,
        CLIAgentSessionStatus::Failed {
            error_type: None,
            message: None,
        },
    ] {
        let session = session(status);
        assert_eq!(
            decide(&pane(Some(&session)), false),
            ClaudeCodeReloadDecision::ReloadNow(expected_resume())
        );
    }
}

#[test]
fn session_that_never_started_a_turn_is_idle() {
    let session = session(CLIAgentSessionStatus::InProgress);
    let mut pane = pane(Some(&session));
    pane.has_seen_prompt_submit = false;
    assert_eq!(
        decide(&pane, false),
        ClaudeCodeReloadDecision::ReloadNow(expected_resume())
    );
}

#[test]
fn working_or_prompting_sessions_reload_when_idle() {
    for status in [
        CLIAgentSessionStatus::InProgress,
        blocked(BlockedSource::PermissionRequest),
        blocked(BlockedSource::QuestionAsked),
        blocked(BlockedSource::NeedsInput),
    ] {
        let session = session(status);
        assert_eq!(
            decide(&pane(Some(&session)), false),
            ClaudeCodeReloadDecision::ReloadWhenIdle(expected_resume())
        );
    }
}

#[test]
fn force_reloads_busy_sessions_now() {
    for status in [
        CLIAgentSessionStatus::InProgress,
        blocked(BlockedSource::PermissionRequest),
    ] {
        let session = session(status);
        assert_eq!(
            decide(&pane(Some(&session)), true),
            ClaudeCodeReloadDecision::ReloadNow(expected_resume())
        );
    }
}

#[test]
fn deferred_pane_is_decided_again() {
    let session = session(CLIAgentSessionStatus::Success);
    let mut pane = pane(Some(&session));
    pane.progress = Some(ClaudeCodeReloadProgress::Deferred);
    assert_eq!(
        decide(&pane, false),
        ClaudeCodeReloadDecision::ReloadNow(expected_resume())
    );
}

#[test]
fn exiting_pane_is_not_typed_into_again() {
    let session = session(CLIAgentSessionStatus::Success);
    let mut pane = pane(Some(&session));
    pane.progress = Some(ClaudeCodeReloadProgress::Exiting);
    assert_eq!(
        decide(&pane, true),
        ClaudeCodeReloadDecision::AlreadyExiting
    );
}

#[test]
fn panes_without_claude_are_not_applicable() {
    let mut no_claude = pane(None);
    no_claude.claude_running = false;
    assert_eq!(
        decide(&no_claude, true),
        ClaudeCodeReloadDecision::NotApplicable
    );

    let mut codex = session(CLIAgentSessionStatus::Success);
    codex.agent = CLIAgent::Codex;
    let mut codex_pane = pane(Some(&codex));
    codex_pane.claude_running = false;
    assert_eq!(
        decide(&codex_pane, true),
        ClaudeCodeReloadDecision::NotApplicable
    );
}

#[test]
fn unverified_plugin_report_is_skipped() {
    let session = session(CLIAgentSessionStatus::Success);
    let mut pane = pane(Some(&session));
    pane.claude_running = false;
    assert_eq!(
        decide(&pane, true),
        ClaudeCodeReloadDecision::Skip(ClaudeCodeReloadSkipReason::NotVerified)
    );
}

#[test]
fn ssh_and_subshell_panes_are_skipped() {
    let mut remote = session(CLIAgentSessionStatus::Success);
    remote.remote_host = Some("dev@servidor".to_owned());
    assert_eq!(
        decide(&pane(Some(&remote)), true),
        ClaudeCodeReloadDecision::Skip(ClaudeCodeReloadSkipReason::RemoteOrSubshell)
    );

    let local = session(CLIAgentSessionStatus::Success);
    let mut subshell = pane(Some(&local));
    subshell.is_local_login_shell = false;
    assert_eq!(
        decide(&subshell, true),
        ClaudeCodeReloadDecision::Skip(ClaudeCodeReloadSkipReason::RemoteOrSubshell)
    );
}

#[test]
fn shared_session_viewers_are_skipped() {
    let session = session(CLIAgentSessionStatus::Success);
    let mut pane = pane(Some(&session));
    pane.is_shared_session_viewer = true;
    assert_eq!(
        decide(&pane, true),
        ClaudeCodeReloadDecision::Skip(ClaudeCodeReloadSkipReason::SharedSession)
    );
}

#[test]
fn missing_or_unsafe_session_id_is_skipped() {
    let without_plugin = pane(None);
    assert_eq!(
        decide(&without_plugin, true),
        ClaudeCodeReloadDecision::Skip(ClaudeCodeReloadSkipReason::NoSessionId)
    );

    let mut injected = session(CLIAgentSessionStatus::Success);
    injected.session_context.session_id = Some("abc; rm -rf ~".to_owned());
    assert_eq!(
        decide(&pane(Some(&injected)), true),
        ClaudeCodeReloadDecision::Skip(ClaudeCodeReloadSkipReason::NoSessionId)
    );
}

#[test]
fn missing_transcript_is_skipped_and_only_checked_when_needed() {
    let session = session(CLIAgentSessionStatus::Success);
    assert_eq!(
        decide_claude_code_reload(&pane(Some(&session)), false, |_| false),
        ClaudeCodeReloadDecision::Skip(ClaudeCodeReloadSkipReason::TranscriptMissing)
    );

    let mut remote = session.clone();
    remote.remote_host = Some("dev@servidor".to_owned());
    decide_claude_code_reload(&pane(Some(&remote)), false, |_| {
        panic!("transcript checked for a pane that cannot reload")
    });
}

#[test]
fn activity_maps_statuses() {
    let working = session(CLIAgentSessionStatus::InProgress);
    assert_eq!(
        claude_code_activity(&working, true),
        ClaudeCodeActivity::Working
    );
    assert_eq!(
        claude_code_activity(&working, false),
        ClaudeCodeActivity::Idle
    );
    let permission = session(blocked(BlockedSource::PermissionRequest));
    assert_eq!(
        claude_code_activity(&permission, true),
        ClaudeCodeActivity::AwaitingUser
    );
}

#[test]
fn summary_message_counts_and_groups_skip_reasons() {
    let mut summary = ClaudeCodeReloadSummary::default();
    assert_eq!(
        summary.message(),
        "No hay sesiones de Claude Code en ejecución."
    );

    let resume = expected_resume();
    for decision in [
        ClaudeCodeReloadDecision::NotApplicable,
        ClaudeCodeReloadDecision::ReloadNow(resume.clone()),
        ClaudeCodeReloadDecision::AlreadyExiting,
        ClaudeCodeReloadDecision::ReloadWhenIdle(resume),
        ClaudeCodeReloadDecision::Skip(ClaudeCodeReloadSkipReason::NoSessionId),
        ClaudeCodeReloadDecision::Skip(ClaudeCodeReloadSkipReason::RemoteOrSubshell),
        ClaudeCodeReloadDecision::Skip(ClaudeCodeReloadSkipReason::RemoteOrSubshell),
    ] {
        summary.record(&decision);
    }
    assert_eq!(summary.reloaded, 2);
    assert_eq!(summary.pending, 1);
    assert_eq!(summary.skipped_count(), 3);
    assert_eq!(
        summary.message(),
        "Recargando 2 · pendientes 1 · omitidas 3 (2 SSH/subshell, 1 sin session_id)"
    );
}

/// Status-line-only sessions stay `InProgress` forever, so "no prompt submitted yet" says
/// nothing about whether they are idle.
#[test]
fn session_without_rich_status_is_unknown() {
    let mut status_line_only = session(CLIAgentSessionStatus::InProgress);
    status_line_only.received_rich_notification = false;
    for has_seen_prompt_submit in [false, true] {
        assert_eq!(
            claude_code_activity(&status_line_only, has_seen_prompt_submit),
            ClaudeCodeActivity::Unknown
        );
    }

    let mut pane = pane(Some(&status_line_only));
    pane.has_seen_prompt_submit = false;
    assert_eq!(
        decide(&pane, false),
        ClaudeCodeReloadDecision::Skip(ClaudeCodeReloadSkipReason::UnknownStatus)
    );
    assert_eq!(
        decide(&pane, true),
        ClaudeCodeReloadDecision::ReloadNow(expected_resume())
    );
    assert_eq!(
        ExitStep::first(Some(ClaudeCodeActivity::Unknown)),
        ExitStep::Interrupt
    );
    assert_eq!(
        ClaudeCodeReloadSkipReason::UnknownStatus.label(),
        "estado desconocido"
    );
}

fn exit_sequence(first: ExitStep) -> Vec<(ExitStep, &'static [u8], Duration)> {
    let mut sequence = Vec::new();
    let mut step = first;
    while let Some((bytes, next, delay)) = step.plan() {
        sequence.push((step, bytes, delay));
        step = next;
    }
    assert_eq!(step, ExitStep::GiveUp);
    sequence
}

/// Text typed into a session that is not at its prompt could answer a permission dialog or
/// reach the shell, so the exit sequence is Ctrl-C only.
#[test]
fn exit_sequence_types_only_ctrl_c() {
    assert_eq!(
        exit_sequence(ExitStep::first(Some(ClaudeCodeActivity::Idle))),
        vec![
            (ExitStep::ClearInput, CTRL_C, KEYSTROKE_GAP),
            (ExitStep::Arm, CTRL_C, KEYSTROKE_GAP),
            (ExitStep::Exit, CTRL_C, EXIT_TIMEOUT),
        ]
    );
    for activity in [
        Some(ClaudeCodeActivity::Working),
        Some(ClaudeCodeActivity::AwaitingUser),
        None,
    ] {
        assert_eq!(
            exit_sequence(ExitStep::first(activity)),
            vec![
                (ExitStep::Interrupt, CTRL_C, FORCE_INTERRUPT_SETTLE),
                (ExitStep::ClearInput, CTRL_C, KEYSTROKE_GAP),
                (ExitStep::Arm, CTRL_C, KEYSTROKE_GAP),
                (ExitStep::Exit, CTRL_C, EXIT_TIMEOUT),
            ]
        );
    }
    assert_eq!(ExitStep::GiveUp.plan(), None);
}

#[test]
fn non_forced_exit_keystrokes_wait_for_idle() {
    assert_eq!(
        check_exit_keystroke(false, Some(ClaudeCodeActivity::Idle)),
        ExitKeystrokeCheck::Type
    );
    for activity in [
        ClaudeCodeActivity::Working,
        ClaudeCodeActivity::AwaitingUser,
        ClaudeCodeActivity::Unknown,
    ] {
        assert_eq!(
            check_exit_keystroke(false, Some(activity)),
            ExitKeystrokeCheck::Defer
        );
        assert_eq!(
            check_exit_keystroke(true, Some(activity)),
            ExitKeystrokeCheck::Type
        );
    }
    assert_eq!(
        check_exit_keystroke(false, None),
        ExitKeystrokeCheck::Abandon
    );
}

#[test]
fn reload_failure_names_the_pane() {
    assert_eq!(
        reload_failed_message("api-server"),
        "Claude no salió en api-server; sesión sin recargar"
    );
}
