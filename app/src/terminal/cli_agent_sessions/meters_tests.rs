use serde_json::json;

use super::*;
use crate::terminal::CLIAgent;
use crate::terminal::cli_agent_sessions::event::{
    CLIAgentEventPayload, CLIAgentEventSource, parse_event,
};

const SENTINEL: Option<&str> = Some("warp://cli-agent");

fn event(event: CLIAgentEventType, payload: CLIAgentEventPayload) -> CLIAgentEvent {
    CLIAgentEvent {
        v: 1,
        agent: CLIAgent::Claude,
        event,
        session_id: None,
        cwd: None,
        project: None,
        payload,
        source: CLIAgentEventSource::RichPlugin,
    }
}

fn bare(event_type: CLIAgentEventType) -> CLIAgentEvent {
    event(event_type, CLIAgentEventPayload::default())
}

#[test]
fn parses_full_status_line_meters() {
    let meters = json!({
        "model": {"id": "claude-opus-5-5", "display_name": "Opus"},
        "context_window": {
            "total_input_tokens": 15500,
            "total_output_tokens": 1200,
            "context_window_size": 1000000,
            "used_percentage": 8,
            "remaining_percentage": 92
        },
        "session_tokens": {"input": 1234567, "output": 89012},
        "cost": {"total_cost_usd": 1.2345, "total_lines_added": 156, "total_lines_removed": 23},
        "rate_limits": {
            "five_hour": {"used_percentage": 23.5, "resets_at": 1738425600},
            "seven_day": {"used_percentage": 41.2, "resets_at": 1738857600}
        }
    });
    let snapshot = parse_meters(&meters).unwrap();

    assert_eq!(snapshot.model_name.as_deref(), Some("Opus"));
    assert_eq!(snapshot.model_id.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(snapshot.context_used_pct, Some(8.));
    assert_eq!(snapshot.context_remaining_pct(), Some(92.));
    assert_eq!(snapshot.context_window_size, Some(1_000_000));
    assert_eq!(snapshot.context_input_tokens, Some(15_500));
    assert_eq!(snapshot.context_output_tokens, Some(1_200));
    assert_eq!(snapshot.session_input_tokens, Some(1_234_567));
    assert_eq!(snapshot.session_output_tokens, Some(89_012));
    assert_eq!(snapshot.cost_usd, Some(1.2345));
    assert_eq!(snapshot.lines_added, Some(156));
    assert_eq!(snapshot.lines_removed, Some(23));
    assert_eq!(
        snapshot.five_hour,
        Some(RateLimitWindow {
            used_pct: 23.5,
            resets_at: Some(1_738_425_600)
        })
    );
    assert_eq!(snapshot.seven_day.map(|w| w.used_pct), Some(41.2));
}

#[test]
fn malformed_fields_are_dropped_individually() {
    let meters = json!({
        "model": {"display_name": 42},
        "context_window": {"used_percentage": "lots", "context_window_size": 200000},
        "cost": {"total_cost_usd": -3.0, "total_lines_added": -1, "total_lines_removed": 1e300},
        "rate_limits": {"five_hour": {"resets_at": 1738425600}}
    });
    let snapshot = parse_meters(&meters).unwrap();

    assert_eq!(snapshot.model_name, None);
    assert_eq!(snapshot.context_used_pct, None);
    assert_eq!(snapshot.context_window_size, Some(200_000));
    assert_eq!(snapshot.cost_usd, None);
    assert_eq!(snapshot.lines_added, None);
    assert_eq!(snapshot.lines_removed, Some(u64::MAX));
    assert_eq!(
        snapshot.five_hour, None,
        "a window without usage is meaningless"
    );
}

#[test]
fn percentages_are_clamped_and_derived_from_remaining() {
    let over = parse_meters(&json!({"context_window": {"used_percentage": 250}})).unwrap();
    assert_eq!(over.context_used_pct, Some(100.));

    let derived = parse_meters(&json!({"context_window": {"remaining_percentage": 30}})).unwrap();
    assert_eq!(derived.context_used_pct, Some(70.));
}

#[test]
fn empty_or_non_object_meters_are_rejected() {
    assert_eq!(parse_meters(&json!({})), None);
    assert_eq!(
        parse_meters(&json!({"context_window": {"used_percentage": null}})),
        None
    );
    assert_eq!(parse_meters(&json!("meters")), None);
    assert_eq!(parse_meters(&json!(null)), None);
}

#[test]
fn reset_times_accept_seconds_millis_and_rfc3339() {
    let window = |resets_at: Value| {
        parse_meters(&json!({
            "rate_limits": {"five_hour": {"used_percentage": 1, "resets_at": resets_at}}
        }))
        .and_then(|s| s.five_hour)
        .and_then(|w| w.resets_at)
    };
    assert_eq!(window(json!(1738425600)), Some(1_738_425_600));
    assert_eq!(window(json!(1738425600123_i64)), Some(1_738_425_600));
    assert_eq!(window(json!("1738425600")), Some(1_738_425_600));
    assert_eq!(window(json!("2025-02-01T16:00:00Z")), Some(1_738_425_600));
    assert_eq!(window(json!("tomorrow")), None);
    assert_eq!(window(json!(-5)), None);
}

#[test]
fn labels_are_stripped_of_control_and_bidi_characters() {
    assert_eq!(
        sanitize_label("Op\u{1b}[31mus\u{202E}\u{7}").as_deref(),
        Some("Op[31mus")
    );
    assert_eq!(sanitize_label("  \u{200B}\n "), None);
    assert_eq!(
        sanitize_label(&"x".repeat(500)).map(|s| s.chars().count()),
        Some(MAX_LABEL_CHARS)
    );
}

#[test]
fn parse_event_maps_meter_events() {
    let status = parse_event(
        SENTINEL,
        r#"{"v":1,"agent":"claude","event":"status_line","session_id":"s1","meters":{"cost":{"total_cost_usd":0.5}}}"#,
    )
    .unwrap();
    assert_eq!(status.event, CLIAgentEventType::StatusLine);
    assert_eq!(status.agent, CLIAgent::Claude);
    assert_eq!(status.session_id.as_deref(), Some("s1"));
    assert_eq!(status.payload.meters.and_then(|m| m.cost_usd), Some(0.5));

    let pre = parse_event(
        SENTINEL,
        r#"{"v":1,"agent":"claude","event":"pre_compact","trigger":"manual","backup_path":"/tmp/b.jsonl"}"#,
    )
    .unwrap();
    assert_eq!(pre.event, CLIAgentEventType::PreCompact);
    assert_eq!(pre.payload.compact_trigger, Some(CompactTrigger::Manual));
    assert_eq!(pre.payload.backup_path.as_deref(), Some("/tmp/b.jsonl"));
    assert!(pre.payload.meters.is_none());

    let post = parse_event(
        SENTINEL,
        r#"{"v":1,"agent":"claude","event":"post_compact","trigger":"auto"}"#,
    )
    .unwrap();
    assert_eq!(post.event, CLIAgentEventType::PostCompact);
    assert_eq!(post.payload.compact_trigger, Some(CompactTrigger::Auto));
}

#[test]
fn meters_are_ignored_on_non_meter_events() {
    let stop = parse_event(
        SENTINEL,
        r#"{"v":1,"agent":"claude","event":"stop","meters":{"cost":{"total_cost_usd":9}},"trigger":"auto"}"#,
    )
    .unwrap();
    assert!(stop.payload.meters.is_none());
    assert!(stop.payload.compact_trigger.is_none());
}

#[test]
fn escaped_semicolons_survive_parsing() {
    // The bridge escapes `;` because the terminal splits OSC parameters on it.
    let status = parse_event(
        SENTINEL,
        r#"{"v":1,"agent":"claude","event":"status_line","meters":{"model":{"display_name":"a;b"}}}"#,
    )
    .unwrap();
    assert_eq!(
        status.payload.meters.and_then(|m| m.model_name).as_deref(),
        Some("a;b")
    );
}

#[test]
fn compaction_lifecycle_is_tracked() {
    let mut meters = CLIAgentMeters::default();
    let pre = event(
        CLIAgentEventType::PreCompact,
        CLIAgentEventPayload {
            compact_trigger: Some(CompactTrigger::Auto),
            backup_path: Some("/backups/t.jsonl".to_owned()),
            ..Default::default()
        },
    );
    assert!(meters.apply_event(&pre));
    assert_eq!(meters.compaction.in_progress, Some(CompactTrigger::Auto));
    assert_eq!(
        CLIAgentActivity::from_session(&CLIAgentSessionStatus::InProgress, Some(&meters)),
        CLIAgentActivity::Compacting
    );

    assert!(meters.apply_event(&bare(CLIAgentEventType::PostCompact)));
    assert_eq!(meters.compaction.in_progress, None);
    assert_eq!(meters.compaction.count, 1);
    assert_eq!(meters.compaction.last_trigger, Some(CompactTrigger::Auto));
    assert_eq!(
        meters.compaction.last_backup_path.as_deref(),
        Some("/backups/t.jsonl")
    );
}

#[test]
fn compaction_without_post_compact_hook_ends_on_session_start() {
    let mut meters = CLIAgentMeters::default();
    meters.apply_event(&event(
        CLIAgentEventType::PreCompact,
        CLIAgentEventPayload {
            compact_trigger: Some(CompactTrigger::Manual),
            ..Default::default()
        },
    ));
    meters.apply_event(&bare(CLIAgentEventType::SessionStart));

    assert_eq!(meters.compaction.in_progress, None);
    assert_eq!(meters.compaction.count, 1);
    assert_eq!(meters.compaction.last_trigger, Some(CompactTrigger::Manual));
}

#[test]
fn unchanged_status_line_reports_no_change() {
    let mut meters = CLIAgentMeters::default();
    let status = event(
        CLIAgentEventType::StatusLine,
        CLIAgentEventPayload {
            meters: Some(CLIAgentMetersSnapshot {
                context_used_pct: Some(12.),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    assert!(meters.apply_event(&status));
    assert!(!meters.apply_event(&status));
}

#[test]
fn activity_reflects_status_and_idle_prompt() {
    let mut meters = CLIAgentMeters::default();
    let activity = |meters: &CLIAgentMeters, status: CLIAgentSessionStatus| {
        CLIAgentActivity::from_session(&status, Some(meters))
    };

    assert_eq!(
        activity(&meters, CLIAgentSessionStatus::InProgress),
        CLIAgentActivity::Idle,
        "registration sets InProgress before any prompt"
    );

    meters.apply_event(&bare(CLIAgentEventType::PromptSubmit));
    assert_eq!(
        activity(&meters, CLIAgentSessionStatus::InProgress),
        CLIAgentActivity::Working
    );
    assert_eq!(
        activity(
            &meters,
            CLIAgentSessionStatus::Blocked {
                message: None,
                source: BlockedSource::PermissionRequest
            }
        ),
        CLIAgentActivity::WaitingForPermission
    );

    meters.apply_event(&bare(CLIAgentEventType::Stop));
    assert_eq!(
        activity(&meters, CLIAgentSessionStatus::Success),
        CLIAgentActivity::Done
    );

    meters.apply_event(&bare(CLIAgentEventType::IdlePrompt));
    assert_eq!(
        activity(&meters, CLIAgentSessionStatus::Success),
        CLIAgentActivity::WaitingForInput
    );
    assert!(CLIAgentActivity::WaitingForInput.needs_attention());

    meters.apply_event(&bare(CLIAgentEventType::PromptSubmit));
    assert_eq!(
        activity(&meters, CLIAgentSessionStatus::InProgress),
        CLIAgentActivity::Working
    );
}

#[test]
fn context_level_follows_threshold() {
    assert_eq!(context_level(79.9, 80), ContextLevel::Normal);
    assert_eq!(context_level(80., 80), ContextLevel::Warning);
    assert_eq!(context_level(89.9, 80), ContextLevel::Warning);
    assert_eq!(context_level(90., 80), ContextLevel::Critical);
    assert_eq!(context_level(50., 50), ContextLevel::Warning);
    assert_eq!(context_level(100., 100), ContextLevel::Critical);
}

#[test]
fn formats_tokens_cost_and_resets() {
    assert_eq!(format_tokens(950), "950");
    assert_eq!(format_tokens(15_500), "15.5k");
    assert_eq!(format_tokens(200_000), "200k");
    assert_eq!(format_tokens(999_999), "1M");
    assert_eq!(format_tokens(1_234_567), "1.2M");
    assert_eq!(format_window_size(1_000_000), "1M");
    assert_eq!(format_cost(1.234), "$1.23");
    assert_eq!(format_cost(12.6), "$13");
    assert_eq!(format_pct(41.6), "42%");

    let now = 1_000_000;
    assert_eq!(format_reset_in(now + 45 * 60, now).as_deref(), Some("45m"));
    assert_eq!(
        format_reset_in(now + 2 * 3600 + 13 * 60, now).as_deref(),
        Some("2h13m")
    );
    assert_eq!(
        format_reset_in(now + 3 * 86_400 + 4 * 3600, now).as_deref(),
        Some("3d4h")
    );
    assert_eq!(format_reset_in(now, now), None);
    assert_eq!(format_reset_in(now - 10, now), None);
}
