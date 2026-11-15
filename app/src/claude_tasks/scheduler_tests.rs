use super::*;

#[test]
fn first_tick_evaluates_only_the_current_minute() {
    assert_eq!(pending_minutes_start(None, 100), 100);
}

#[test]
fn consecutive_ticks_evaluate_the_new_minute() {
    assert_eq!(pending_minutes_start(Some(99), 100), 100);
    assert_eq!(pending_minutes_start(Some(100), 100), 100);
}

#[test]
fn late_tick_catches_up_on_skipped_minutes() {
    assert_eq!(pending_minutes_start(Some(95), 100), 96);
}

#[test]
fn catch_up_is_capped() {
    assert_eq!(pending_minutes_start(Some(10), 100), 100 - MAX_CATCH_UP_MINUTES + 1);
}

#[test]
fn clock_moving_backwards_evaluates_only_the_current_minute() {
    assert_eq!(pending_minutes_start(Some(200), 100), 100);
}
