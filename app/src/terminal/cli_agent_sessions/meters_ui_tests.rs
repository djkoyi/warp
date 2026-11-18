use super::*;
use crate::terminal::cli_agent_sessions::meters::{
    CompactTrigger, CompactionState, GlobalMeters, RateLimitWindow,
};
use crate::terminal::cli_agent_sessions::plan::ClaudePlan;

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
            "sesión 23% (se reinicia en 2h13m)",
            "semana 96%",
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

fn ultra_plan() -> ClaudePlan {
    ClaudePlan {
        name: "Ultra".to_owned(),
        tier: Some("Max 20x".to_owned()),
    }
}

#[test]
fn global_chip_segments_show_plan_name_and_usage_without_cost() {
    let global = GlobalMeters {
        five_hour: Some(RateLimitWindow {
            used_pct: 42.,
            resets_at: Some(1_000),
        }),
        seven_day: Some(RateLimitWindow {
            used_pct: 96.,
            resets_at: None,
        }),
        cost_usd: Some(12.34),
    };
    let segments = global_chip_segments(Some(&ultra_plan()), Some(&global));
    assert_eq!(
        texts(&segments),
        vec!["Plan Ultra", "Sesión 42%", "Semana 96%"]
    );
    assert_eq!(segments[0].tone, MeterTone::Accent);
    assert_eq!(segments[1].tone, MeterTone::Normal);
    assert_eq!(segments[2].tone, MeterTone::Danger);
}

#[test]
fn global_chip_segments_show_the_plan_before_any_usage_arrives() {
    assert_eq!(
        texts(&global_chip_segments(Some(&ultra_plan()), None)),
        vec!["Plan Ultra"]
    );
}

#[test]
fn global_chip_segments_skip_missing_window() {
    let only_five_hour = GlobalMeters {
        five_hour: Some(RateLimitWindow {
            used_pct: 10.,
            resets_at: None,
        }),
        ..Default::default()
    };
    assert_eq!(
        texts(&global_chip_segments(None, Some(&only_five_hour))),
        vec!["Sesión 10%"]
    );

    let only_seven_day = GlobalMeters {
        seven_day: Some(RateLimitWindow {
            used_pct: 88.,
            resets_at: None,
        }),
        ..Default::default()
    };
    assert_eq!(
        texts(&global_chip_segments(None, Some(&only_seven_day))),
        vec!["Semana 88%"]
    );
}

#[test]
fn global_chip_segments_empty_without_plan_or_window_even_with_cost() {
    let cost_only = GlobalMeters {
        five_hour: None,
        seven_day: None,
        cost_usd: Some(5.),
    };
    assert!(global_chip_segments(None, Some(&cost_only)).is_empty());
    assert!(global_chip_segments(None, Some(&GlobalMeters::default())).is_empty());
    assert!(global_chip_segments(None, None).is_empty());
}

#[test]
fn global_usage_rows_describe_each_limit_and_its_reset() {
    let now = 10_000;
    let global = GlobalMeters {
        five_hour: Some(RateLimitWindow {
            used_pct: 42.,
            resets_at: Some(now + 2 * 3_600 + 13 * 60),
        }),
        seven_day: Some(RateLimitWindow {
            used_pct: 96.,
            resets_at: None,
        }),
        cost_usd: None,
    };
    let rows = global_usage_rows(Some(&global), now);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].title, "Sesión actual (5 horas)");
    assert_eq!(rows[0].detail, "42% usado · se reinicia en 2 h 13 min");
    assert_eq!(rows[0].tone, MeterTone::Normal);
    assert_eq!(rows[1].title, "Semana (todos los modelos)");
    assert_eq!(rows[1].detail, "96% usado");
    assert_eq!(rows[1].tone, MeterTone::Danger);
    assert_eq!(rows[0].pace, None);
    assert!(global_usage_rows(None, now).is_empty());
}

#[test]
fn global_usage_rows_warn_when_the_pace_exhausts_the_session() {
    let now = 1_000_000;
    // 2 h into the 5 h window with 60 % used: the rest lasts 80 minutes, the reset is in 3 h.
    let global = GlobalMeters {
        five_hour: Some(RateLimitWindow {
            used_pct: 60.,
            resets_at: Some(now + 3 * 3_600),
        }),
        ..Default::default()
    };
    let rows = global_usage_rows(Some(&global), now);
    assert_eq!(
        rows[0].pace.as_deref(),
        Some("Al ritmo actual llegas al tope en 1 h 20 min")
    );
}

#[test]
fn quota_alert_texts_name_the_plan_the_limit_and_the_reset() {
    use crate::terminal::cli_agent_sessions::quota::{QuotaAlert, QuotaLevel, QuotaWindow};
    let now = 1_000_000;
    let warning = QuotaAlert {
        window: QuotaWindow::Session,
        level: QuotaLevel::Warning,
        used_pct: 82.,
        resets_at: Some(now + 65 * 60),
    };
    assert_eq!(
        quota_alert_texts(&warning, Some(&ultra_plan()), now),
        (
            "Plan Ultra: sesión al 82%".to_owned(),
            "Queda poco de la sesión actual (5 horas). Se reinicia en 1 h 5 min.".to_owned()
        )
    );
    let danger = QuotaAlert {
        window: QuotaWindow::Week,
        level: QuotaLevel::Danger,
        used_pct: 96.,
        resets_at: None,
    };
    assert_eq!(
        quota_alert_texts(&danger, None, now),
        (
            "Claude: semana al 96%".to_owned(),
            "Casi se agota la semana.".to_owned()
        )
    );
}

#[test]
fn format_reset_when_uses_a_countdown_within_a_day_and_a_date_beyond() {
    let now = 1_000_000;
    assert_eq!(
        format_reset_when(now + 45 * 60, now).as_deref(),
        Some("en 45 min")
    );
    assert_eq!(format_reset_when(now - 1, now), None);
    let far = format_reset_when(now + 3 * 86_400, now).unwrap();
    assert!(far.starts_with("el "), "{far}");
}

#[test]
fn format_spanish_date_is_short_and_in_spanish() {
    use chrono::{TimeZone, Utc};
    // 2026-11-26 is a Thursday.
    let at = Utc.with_ymd_and_hms(2026, 11, 26, 9, 5, 0).unwrap();
    assert_eq!(format_spanish_date(&at), "jue 26 nov, 9:05");
}
