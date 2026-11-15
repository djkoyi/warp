use super::{ClaudeCodeResume, is_valid_session_id, quote_for_shell};
use crate::terminal::CLIAgent;
use crate::terminal::cli_agent_sessions::{
    CLIAgentInputState, CLIAgentSession, CLIAgentSessionContext, CLIAgentSessionStatus,
};
use crate::terminal::shell::ShellType;

const SESSION_ID: &str = "0f6c1a2b-3c4d-4e5f-8a9b-0c1d2e3f4a5b";

fn resume(cwd: Option<&str>) -> ClaudeCodeResume {
    ClaudeCodeResume {
        session_id: SESSION_ID.to_owned(),
        cwd: cwd.map(str::to_owned),
    }
}

fn claude_session(context: CLIAgentSessionContext) -> CLIAgentSession {
    CLIAgentSession {
        agent: CLIAgent::Claude,
        status: CLIAgentSessionStatus::InProgress,
        session_context: context,
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

#[test]
fn session_id_validation_rejects_shell_syntax() {
    assert!(is_valid_session_id(SESSION_ID));
    assert!(!is_valid_session_id(""));
    assert!(!is_valid_session_id("-rf"));
    assert!(!is_valid_session_id("abc; rm -rf ~"));
    assert!(!is_valid_session_id("abc$(id)"));
    assert!(!is_valid_session_id("abc\ndef"));
    assert!(!is_valid_session_id(&"a".repeat(129)));
}

#[test]
fn from_persisted_drops_invalid_values() {
    assert_eq!(
        ClaudeCodeResume::from_persisted(Some("bad id".to_owned()), Some("/tmp".to_owned())),
        None
    );
    assert_eq!(
        ClaudeCodeResume::from_persisted(Some(SESSION_ID.to_owned()), Some("relative".to_owned())),
        Some(resume(None))
    );
    assert_eq!(
        ClaudeCodeResume::from_persisted(Some(SESSION_ID.to_owned()), Some("/a\nb".to_owned())),
        Some(resume(None))
    );
}

#[test]
fn from_live_session_prefers_resume_cwd_over_drifted_cwd() {
    let session = claude_session(CLIAgentSessionContext {
        session_id: Some(SESSION_ID.to_owned()),
        cwd: Some("/projects/app/subdir".to_owned()),
        resume_cwd: Some("/projects/app".to_owned()),
        ..Default::default()
    });
    assert_eq!(
        ClaudeCodeResume::from_live_session(&session),
        Some(resume(Some("/projects/app")))
    );
}

#[test]
fn from_live_session_ignores_remote_and_other_agents() {
    let context = CLIAgentSessionContext {
        session_id: Some(SESSION_ID.to_owned()),
        cwd: Some("/projects/app".to_owned()),
        ..Default::default()
    };

    let mut remote = claude_session(context.clone());
    remote.remote_host = Some("user@host".to_owned());
    assert_eq!(ClaudeCodeResume::from_live_session(&remote), None);

    let mut codex = claude_session(context);
    codex.agent = CLIAgent::Codex;
    assert_eq!(ClaudeCodeResume::from_live_session(&codex), None);
}

#[test]
fn command_uses_explicit_session_id_and_default_flag() {
    let resume_in_project = resume(Some("/Users/me/my project"));
    assert_eq!(
        resume_in_project.command("--dangerously-skip-permissions", ShellType::Zsh),
        format!(
            "builtin cd '/Users/me/my project' && \
             claude --resume {SESSION_ID} --dangerously-skip-permissions"
        )
    );
    assert_eq!(
        resume(None).command("", ShellType::Bash),
        format!("claude --resume {SESSION_ID}")
    );
}

#[test]
fn command_quotes_flags_with_shell_syntax() {
    assert_eq!(
        resume(None).command("--model opus ;rm $(id)", ShellType::Bash),
        format!("claude --resume {SESSION_ID} --model opus ';rm' '$(id)'")
    );
}

#[test]
fn command_for_powershell_only_runs_claude_after_successful_cd() {
    assert_eq!(
        resume(Some("/tmp/it's")).command("", ShellType::PowerShell),
        format!(
            "Set-Location -LiteralPath '/tmp/it''s'; \
             if ($?) {{ claude --resume {SESSION_ID} }}"
        )
    );
}

#[test]
fn quoting_neutralizes_quotes_per_shell() {
    assert_eq!(quote_for_shell("a'b", ShellType::Zsh), r#"'a'"'"'b'"#);
    assert_eq!(quote_for_shell("a'b", ShellType::Bash), r#"'a'"'"'b'"#);
    assert_eq!(quote_for_shell(r"a\'b", ShellType::Fish), r"'a\\\'b'");
    assert_eq!(quote_for_shell("a\u{2019}b", ShellType::PowerShell), "'a\u{2019}\u{2019}b'");
    assert_eq!(quote_for_shell("$(id) `x` $HOME", ShellType::Zsh), "'$(id) `x` $HOME'");
}

#[test]
fn command_never_trusts_unvalidated_fields() {
    let resume = ClaudeCodeResume {
        session_id: "x; touch /tmp/pwned".to_owned(),
        cwd: Some("/tmp\n; touch /tmp/pwned".to_owned()),
    };
    assert_eq!(
        resume.command("", ShellType::Zsh),
        "claude --resume 'x; touch /tmp/pwned'"
    );
}
