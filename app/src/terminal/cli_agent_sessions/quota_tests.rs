use super::*;

const HOUR: i64 = 3_600;
const NOW: i64 = 1_000_000;

fn window(used_pct: f32, resets_in: i64) -> RateLimitWindow {
    RateLimitWindow {
        used_pct,
        resets_at: Some(NOW + resets_in),
    }
}

fn session(used_pct: f32, resets_in: i64) -> GlobalMeters {
    GlobalMeters {
        five_hour: Some(window(used_pct, resets_in)),
        ..Default::default()
    }
}

#[test]
fn pace_that_exhausts_the_session_before_the_reset_is_projected() {
    // 2 h into the 5 h window with 60 % used: 30 %/h, so the remaining 40 % lasts 80 min.
    assert_eq!(
        seconds_to_exhaustion(window(60., 3 * HOUR), QuotaWindow::Session, NOW),
        Some(80 * 60)
    );
}

#[test]
fn sustainable_pace_projects_nothing() {
    // 4 h in with 40 % used: 10 %/h, the remaining 60 % would take 6 h but the reset is in 1 h.
    assert_eq!(
        seconds_to_exhaustion(window(40., HOUR), QuotaWindow::Session, NOW),
        None
    );
}

#[test]
fn pace_needs_enough_of_the_window_and_of_the_usage() {
    // 5 minutes into the window: too early.
    assert_eq!(
        seconds_to_exhaustion(window(30., 5 * HOUR - 300), QuotaWindow::Session, NOW),
        None
    );
    // Barely used.
    assert_eq!(
        seconds_to_exhaustion(window(3., HOUR), QuotaWindow::Session, NOW),
        None
    );
    // Already exhausted, already reset, or without a reset time.
    assert_eq!(
        seconds_to_exhaustion(window(100., HOUR), QuotaWindow::Session, NOW),
        None
    );
    assert_eq!(
        seconds_to_exhaustion(window(60., -10), QuotaWindow::Session, NOW),
        None
    );
    let no_reset = RateLimitWindow {
        used_pct: 60.,
        resets_at: None,
    };
    assert_eq!(
        seconds_to_exhaustion(no_reset, QuotaWindow::Session, NOW),
        None
    );
}

#[test]
fn weekly_pace_uses_the_seven_day_window() {
    // 2 days into the week with 50 % used: the other 50 % lasts 2 more days, reset is in 5.
    assert_eq!(
        seconds_to_exhaustion(window(50., 5 * 24 * HOUR), QuotaWindow::Week, NOW),
        Some(2 * 24 * HOUR)
    );
}

#[test]
fn alerts_fire_once_per_level_and_window() {
    let mut state = QuotaAlertState::default();
    assert!(state.update(Some(&session(50., HOUR)), NOW).is_empty());

    let alerts = state.update(Some(&session(82., HOUR)), NOW);
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].window, QuotaWindow::Session);
    assert_eq!(alerts[0].level, QuotaLevel::Warning);

    // Still in the warning band: silent, even if the reset time jitters by seconds.
    assert!(state.update(Some(&session(90., HOUR + 30)), NOW).is_empty());

    let alerts = state.update(Some(&session(96., HOUR)), NOW);
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].level, QuotaLevel::Danger);
    assert!(state.update(Some(&session(99., HOUR)), NOW).is_empty());
}

#[test]
fn alerts_fire_again_after_the_window_resets() {
    let mut state = QuotaAlertState::default();
    assert_eq!(state.update(Some(&session(85., HOUR)), NOW).len(), 1);
    // New window: low usage clears the memory, so the next crossing notifies again.
    assert!(state.update(Some(&session(10., 5 * HOUR)), NOW).is_empty());
    assert_eq!(state.update(Some(&session(81., 5 * HOUR)), NOW).len(), 1);
}

#[test]
fn stale_or_missing_reports_never_alert() {
    let mut state = QuotaAlertState::default();
    // A report from a window that already reset (a session left open overnight).
    assert!(state.update(Some(&session(99., -60)), NOW).is_empty());
    assert!(state.update(None, NOW).is_empty());
    assert!(state.update(Some(&GlobalMeters::default()), NOW).is_empty());
}

#[test]
fn session_and_week_alert_independently() {
    let mut state = QuotaAlertState::default();
    let global = GlobalMeters {
        five_hour: Some(window(97., HOUR)),
        seven_day: Some(window(81., 24 * HOUR)),
        cost_usd: None,
    };
    let alerts = state.update(Some(&global), NOW);
    assert_eq!(
        alerts
            .iter()
            .map(|a| (a.window, a.level))
            .collect::<Vec<_>>(),
        vec![
            (QuotaWindow::Session, QuotaLevel::Danger),
            (QuotaWindow::Week, QuotaLevel::Warning),
        ]
    );
    assert!(state.update(Some(&global), NOW).is_empty());
}
