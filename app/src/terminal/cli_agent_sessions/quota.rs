//! What a subscription cares about instead of cost: how fast the plan limits are being consumed
//! and when one of them is about to run out. Pure logic over the rate-limit windows reported by
//! the meters bridge; nothing here renders or notifies.

use super::meters::{GlobalMeters, RateLimitWindow};

/// Plan usage (percent) at which a limit is worth a warning, and at which it is nearly exhausted.
pub const QUOTA_WARNING_PCT: f32 = 80.;
pub const QUOTA_DANGER_PCT: f32 = 95.;

const SESSION_WINDOW_SECS: i64 = 5 * 3_600;
const WEEK_WINDOW_SECS: i64 = 7 * 86_400;
/// A pace is only projected once this fraction of the window has elapsed and this much of it has
/// been used; earlier than that a single long reply would look like a runaway pace.
const MIN_ELAPSED_FRACTION: f64 = 0.05;
const MIN_USED_PCT_FOR_PACE: f32 = 5.;
/// Reset times of the same window can differ by a few seconds between reports.
const SAME_WINDOW_TOLERANCE_SECS: i64 = 120;

/// One of the two limits of a subscription plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaWindow {
    /// The rolling 5-hour window ("current session").
    Session,
    /// The 7-day window.
    Week,
}

impl QuotaWindow {
    pub fn duration_secs(self) -> i64 {
        match self {
            Self::Session => SESSION_WINDOW_SECS,
            Self::Week => WEEK_WINDOW_SECS,
        }
    }
}

/// How close a limit is to running out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum QuotaLevel {
    Warning,
    Danger,
}

impl QuotaLevel {
    pub fn for_pct(used_pct: f32) -> Option<Self> {
        if used_pct >= QUOTA_DANGER_PCT {
            Some(Self::Danger)
        } else if used_pct >= QUOTA_WARNING_PCT {
            Some(Self::Warning)
        } else {
            None
        }
    }
}

/// Seconds from `now` until the window is exhausted at the average pace since it started, when
/// that happens before the window resets. `None` when the pace is sustainable, the window is
/// already exhausted or expired, or it is too early in the window to tell.
pub fn seconds_to_exhaustion(
    window: RateLimitWindow,
    kind: QuotaWindow,
    now: i64,
) -> Option<i64> {
    let resets_at = window.resets_at?;
    let duration = kind.duration_secs();
    let elapsed = now.checked_sub(resets_at.checked_sub(duration)?)?;
    if now >= resets_at
        || (elapsed as f64) < duration as f64 * MIN_ELAPSED_FRACTION
        || window.used_pct < MIN_USED_PCT_FOR_PACE
        || window.used_pct >= 100.
    {
        return None;
    }
    let remaining_pct = f64::from(100. - window.used_pct);
    let secs_left = (remaining_pct * elapsed as f64 / f64::from(window.used_pct)).ceil() as i64;
    (now.saturating_add(secs_left) < resets_at).then_some(secs_left)
}

/// A limit that just crossed a threshold and deserves a notification.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuotaAlert {
    pub window: QuotaWindow,
    pub level: QuotaLevel,
    pub used_pct: f32,
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Announced {
    level: QuotaLevel,
    resets_at: Option<i64>,
}

/// Remembers which thresholds were already announced, so each one notifies once per window: once
/// at the warning level, once more at the danger level, and again only after the window resets.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QuotaAlertState {
    session: Option<Announced>,
    week: Option<Announced>,
}

impl QuotaAlertState {
    /// Feeds the latest account usage and returns the alerts that became due.
    pub fn update(&mut self, global: Option<&GlobalMeters>, now: i64) -> Vec<QuotaAlert> {
        let mut alerts = Vec::new();
        let windows = [
            (
                QuotaWindow::Session,
                global.and_then(|global| global.five_hour),
                &mut self.session,
            ),
            (
                QuotaWindow::Week,
                global.and_then(|global| global.seven_day),
                &mut self.week,
            ),
        ];
        for (kind, window, announced) in windows {
            // No report, or a stale one from a window that already reset: nothing to say.
            let Some(window) = window.filter(|w| w.resets_at.is_none_or(|at| at > now)) else {
                continue;
            };
            if announced.is_some_and(|a| !same_window(a.resets_at, window.resets_at)) {
                *announced = None;
            }
            let Some(level) = QuotaLevel::for_pct(window.used_pct) else {
                continue;
            };
            if announced.is_none_or(|a| level > a.level) {
                *announced = Some(Announced {
                    level,
                    resets_at: window.resets_at,
                });
                alerts.push(QuotaAlert {
                    window: kind,
                    level,
                    used_pct: window.used_pct,
                    resets_at: window.resets_at,
                });
            }
        }
        alerts
    }
}

fn same_window(a: Option<i64>, b: Option<i64>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() <= SAME_WINDOW_TOLERANCE_SECS,
        // Without a reset time there is no way to tell windows apart; assume it is the same one
        // rather than repeat the alert.
        _ => true,
    }
}

#[cfg(test)]
#[path = "quota_tests.rs"]
mod tests;
