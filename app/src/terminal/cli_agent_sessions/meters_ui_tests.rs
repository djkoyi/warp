use super::*;
use crate::terminal::cli_agent_sessions::meters::{
    CompactTrigger, CompactionState, GlobalMeters, RateLimitWindow,
};

fn texts(segments: &[MeterSegment]) -> Vec<&str> {
    segments.iter().map(|s| s.text.as_str()).collect()
}

fn meters_with(snapshot: CLIAgentMetersSnapshot) -> CLIAgentMeters {
    CLIAgentMeters {
        snapshot: Some(snapshot),
        ..Default::default()
    }
}

#[test]
fn tab_segments_show_context_and_activity() {
    let meters = meters_with(CLIAgentMetersSnapshot {
        context_used_pct: Some(85.),
        ..Default::default()
    });
    let segments = tab_segments(
        Some(CLIAgentActivity::WaitingForPermission),
        Some(&meters),
        80,
    );
    assert_eq!(texts(&segments), vec!["85%", "needs permission"]);
    assert_eq!(segments[0].tone, MeterTone::Warning);
    assert_eq!(segments[1].tone, MeterTone::Warning);
}

#[test]
fn tab_segments_hide_idle_and_missing_values() {
    assert!(tab_segments(Some(CLIAgentActivity::Idle), None, 80).is_empty());
    assert!(tab_segments(None, Some(&CLIAgentMeters::default()), 80).is_empty());
}

#[test]
fn strip_segments_cover_all_meters() {
    let now = 1_000_000;
    let meters = CLIAgentMeters {
        snapshot: Some(CLIAgentMetersSnapshot {
            model_name: Some("Opus".to_owned()),
            context_used_pct: Some(92.),
            context_window_size: Some(200_000),
            session_input_tokens: Some(1_500_000),
            session_output_tokens: Some(42_000),
            cost_usd: Some(3.5),
            five_hour: Some(RateLimitWindow {
                used_pct: 23.,
                resets_at: Some(now + 2 * 3600 + 13 * 60),
            }),
            seven_day: Some(RateLimitWindow {
                used_pct: 96.,
                resets_at: None,
            }),
            ..Default::default()
        }),
        compaction: CompactionState {
            in_progress: None,
            count: 2,
            last_trigger: Some(CompactTrigger::Auto),
            last_backup_path: None,
        },
        ..Default::default()
    };

    let segments = strip_segments(Some(CLIAgentActivity::Working), &meters, 80, now);
    assert_eq!(
        texts(&segments),
        vec![
            "working",
            "compacted ×2, last auto",
            "Opus",
            "ctx 92% of 200k (8% left)",
            "compaction imminent",
            "session in 1.5M / out 42k",
            "$3.50",
            "5h 23% (resets 2h13m)",
            "7d 96%",
        ]
    );
    let seven_day = segments.last().unwrap();
    assert_eq!(seven_day.tone, MeterTone::Danger);
}

#[test]
fn strip_segments_fall_back_to_last_turn_tokens() {
    let meters = meters_with(CLIAgentMetersSnapshot {
        context_input_tokens: Some(15_500),
        context_output_tokens: Some(1_200),
        ..Default::default()
    });
    assert_eq!(
        texts(&strip_segments(None, &meters, 80, 0)),
        vec!["last turn in 15.5k / out 1.2k"]
    );
}

#[test]
fn strip_segments_show_compaction_in_progress() {
    let meters = CLIAgentMeters {
        compaction: CompactionState {
            in_progress: Some(CompactTrigger::Manual),
            last_backup_path: Some("/b".to_owned()),
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        texts(&strip_segments(None, &meters, 80, 0)),
        vec!["compacting (manual)", "transcript backed up"]
    );
}
