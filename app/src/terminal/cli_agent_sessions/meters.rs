//! Per-session usage meters (context window, tokens, cost, plan limits, compaction) reported by
//! the Claude Code meters bridge (`app/resources/claude_meters/warp_claude_meters.py`) over the
//! `warp://cli-agent` OSC 777 channel.
//!
//! Everything here is derived from terminal output, so every value is treated as untrusted:
//! numbers are range-checked and strings are stripped of control/format characters and truncated
//! before they can reach the UI.

use serde_json::Value;

use super::event::{CLIAgentEvent, CLIAgentEventType};
use super::{BlockedSource, CLIAgentSessionStatus};

/// Longest label (model name, backup path) kept from the wire.
const MAX_LABEL_CHARS: usize = 96;

/// Epoch values above this are interpreted as milliseconds rather than seconds.
const EPOCH_MILLIS_THRESHOLD: f64 = 100_000_000_000.;

/// Default context usage (percent) at which the pre-compaction warning is shown.
pub const DEFAULT_COMPACTION_WARNING_THRESHOLD: u8 = 80;

/// What started a context compaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactTrigger {
    /// The user ran `/compact`.
    Manual,
    /// Claude Code compacted on its own because the context window was nearly full.
    Auto,
}

impl CompactTrigger {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "manual" => Some(Self::Manual),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Auto => "auto",
        }
    }
}

/// One plan rate-limit window (5-hour or 7-day).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateLimitWindow {
    /// Percentage of the window consumed, 0–100.
    pub used_pct: f32,
    /// Unix epoch seconds at which the window resets.
    pub resets_at: Option<i64>,
}

/// The latest values reported by Claude Code's `statusLine` input.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CLIAgentMetersSnapshot {
    pub model_name: Option<String>,
    pub model_id: Option<String>,
    /// Percentage of the context window in use, 0–100.
    pub context_used_pct: Option<f32>,
    /// Context window size in tokens (200k by default, 1M for extended-context models).
    pub context_window_size: Option<u64>,
    /// Input tokens currently in the context window (from the latest API response).
    pub context_input_tokens: Option<u64>,
    /// Output tokens of the latest API response.
    pub context_output_tokens: Option<u64>,
    /// Input tokens summed over every API response of the session (computed by the bridge from
    /// the transcript; includes cache reads and writes).
    pub session_input_tokens: Option<u64>,
    /// Output tokens summed over every API response of the session.
    pub session_output_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub lines_added: Option<u64>,
    pub lines_removed: Option<u64>,
    pub five_hour: Option<RateLimitWindow>,
    pub seven_day: Option<RateLimitWindow>,
}

impl CLIAgentMetersSnapshot {
    pub fn context_remaining_pct(&self) -> Option<f32> {
        self.context_used_pct.map(|used| (100. - used).max(0.))
    }
}

/// Compaction history of a session.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CompactionState {
    /// Set between `pre_compact` and the end of that compaction.
    pub in_progress: Option<CompactTrigger>,
    /// Number of compactions observed since Warp started tracking this session.
    pub count: u32,
    pub last_trigger: Option<CompactTrigger>,
    /// Where the bridge copied the transcript before the latest compaction, if it did.
    pub last_backup_path: Option<String>,
}

impl CompactionState {
    fn finish(&mut self, trigger: Option<CompactTrigger>) {
        let trigger = trigger.or(self.in_progress.take());
        self.in_progress = None;
        self.count = self.count.saturating_add(1);
        self.last_trigger = trigger.or(self.last_trigger);
    }
}

/// Meter state accumulated from the events of one CLI agent session.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CLIAgentMeters {
    pub snapshot: Option<CLIAgentMetersSnapshot>,
    pub compaction: CompactionState,
    /// The agent reported that it is idle at its prompt (or explicitly needs input) and nothing
    /// has happened since.
    pub awaiting_user: bool,
    /// A prompt has been submitted at least once, so `InProgress` really means "working" rather
    /// than the optimistic status set when the session is first registered.
    pub has_seen_turn: bool,
    /// Meter events (which only the meters bridge sends) have been received, so the bridge's
    /// hooks stand in for the official Warp plugin.
    pub bridge_connected: bool,
}

impl CLIAgentMeters {
    /// Folds `event` into the meter state. Returns whether anything changed.
    pub fn apply_event(&mut self, event: &CLIAgentEvent) -> bool {
        let before = self.clone();
        match &event.event {
            CLIAgentEventType::StatusLine => {
                self.bridge_connected = true;
                if let Some(snapshot) = &event.payload.meters {
                    self.snapshot = Some(snapshot.clone());
                }
            }
            CLIAgentEventType::PreCompact => {
                self.bridge_connected = true;
                self.compaction.in_progress = Some(
                    event
                        .payload
                        .compact_trigger
                        .unwrap_or(CompactTrigger::Auto),
                );
                if event.payload.backup_path.is_some() {
                    self.compaction.last_backup_path = event.payload.backup_path.clone();
                }
            }
            CLIAgentEventType::PostCompact => {
                self.bridge_connected = true;
                // `SessionStart` (source `compact`) may already have ended this compaction.
                if self.compaction.in_progress.is_some() {
                    self.compaction.finish(event.payload.compact_trigger);
                }
            }
            CLIAgentEventType::IdlePrompt | CLIAgentEventType::NeedsInput => {
                self.awaiting_user = true;
            }
            CLIAgentEventType::PromptSubmit => {
                self.has_seen_turn = true;
                self.awaiting_user = false;
                self.finish_stale_compaction();
            }
            // Older Claude Code versions have no `PostCompact` hook; the compaction is over once
            // the session restarts (`SessionStart` with source `compact`) or the turn ends.
            CLIAgentEventType::SessionStart | CLIAgentEventType::Stop => {
                self.awaiting_user = false;
                self.finish_stale_compaction();
            }
            CLIAgentEventType::StopFailure
            | CLIAgentEventType::PermissionRequest
            | CLIAgentEventType::PermissionReplied
            | CLIAgentEventType::ToolComplete
            | CLIAgentEventType::QuestionAsked => {
                self.awaiting_user = false;
            }
            CLIAgentEventType::Unknown(_) => {}
        }
        *self != before
    }

    fn finish_stale_compaction(&mut self) {
        if self.compaction.in_progress.is_some() {
            self.compaction.finish(None);
        }
    }
}

/// What a CLI agent tab is doing, as shown on its status chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CLIAgentActivity {
    /// The session is open but no prompt has been submitted yet.
    Idle,
    Working,
    Compacting,
    WaitingForPermission,
    WaitingForInput,
    Done,
    Failed,
    Cancelled,
}

impl CLIAgentActivity {
    pub fn from_session(status: &CLIAgentSessionStatus, meters: Option<&CLIAgentMeters>) -> Self {
        let awaiting_user = meters.is_some_and(|m| m.awaiting_user);
        if meters.is_some_and(|m| m.compaction.in_progress.is_some()) {
            return Self::Compacting;
        }
        match status {
            CLIAgentSessionStatus::Blocked {
                source: BlockedSource::PermissionRequest,
                ..
            } => Self::WaitingForPermission,
            CLIAgentSessionStatus::Blocked {
                source: BlockedSource::QuestionAsked | BlockedSource::NeedsInput,
                ..
            } => Self::WaitingForInput,
            CLIAgentSessionStatus::InProgress if awaiting_user => Self::WaitingForInput,
            CLIAgentSessionStatus::InProgress => {
                if meters.is_some_and(|m| m.has_seen_turn) {
                    Self::Working
                } else {
                    Self::Idle
                }
            }
            CLIAgentSessionStatus::Success if awaiting_user => Self::WaitingForInput,
            CLIAgentSessionStatus::Success => Self::Done,
            CLIAgentSessionStatus::Failed { .. } => Self::Failed,
            CLIAgentSessionStatus::Cancelled => Self::Cancelled,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Compacting => "compacting",
            Self::WaitingForPermission => "needs permission",
            Self::WaitingForInput => "waiting for you",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Whether the user has to act for the agent to continue.
    pub fn needs_attention(self) -> bool {
        match self {
            Self::WaitingForPermission | Self::WaitingForInput | Self::Failed => true,
            Self::Idle | Self::Working | Self::Compacting | Self::Done | Self::Cancelled => false,
        }
    }
}

/// Severity of the context-window meter relative to the compaction warning threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextLevel {
    Normal,
    /// At or above the warning threshold: compaction is approaching.
    Warning,
    /// Halfway between the threshold and a full window.
    Critical,
}

pub fn context_level(used_pct: f32, threshold: u8) -> ContextLevel {
    let threshold = f32::from(threshold.min(100));
    let critical = threshold + (100. - threshold) / 2.;
    if used_pct >= critical {
        ContextLevel::Critical
    } else if used_pct >= threshold {
        ContextLevel::Warning
    } else {
        ContextLevel::Normal
    }
}

/// Formats a token count compactly: `950`, `15.5k`, `1.2M`.
pub fn format_tokens(tokens: u64) -> String {
    if tokens < 1_000 {
        tokens.to_string()
    } else if tokens < 999_950 {
        // Below this `{:.1}` would round up to "1000.0k".
        trim_decimal(format!("{:.1}", tokens as f64 / 1_000.)) + "k"
    } else {
        trim_decimal(format!("{:.1}", tokens as f64 / 1_000_000.)) + "M"
    }
}

/// Formats a context window size: `200k`, `1M`.
pub fn format_window_size(tokens: u64) -> String {
    format_tokens(tokens)
}

pub fn format_cost(usd: f64) -> String {
    if usd < 10. {
        format!("${usd:.2}")
    } else {
        format!("${usd:.0}")
    }
}

pub fn format_pct(pct: f32) -> String {
    format!("{:.0}%", pct.clamp(0., 100.))
}

/// Formats the time until `resets_at` (epoch seconds) relative to `now` (epoch seconds):
/// `45m`, `2h13m`, `3d4h`. Returns `None` once the reset time has passed.
pub fn format_reset_in(resets_at: i64, now: i64) -> Option<String> {
    let remaining = resets_at.checked_sub(now)?;
    if remaining <= 0 {
        return None;
    }
    let minutes = remaining.saturating_add(59) / 60;
    let (days, hours, mins) = (minutes / 1_440, (minutes % 1_440) / 60, minutes % 60);
    Some(if days > 0 {
        format!("{days}d{hours}h")
    } else if hours > 0 {
        format!("{hours}h{mins:02}m")
    } else {
        format!("{mins}m")
    })
}

pub fn now_epoch_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn trim_decimal(value: String) -> String {
    value.strip_suffix(".0").map(str::to_owned).unwrap_or(value)
}

/// Parses the `meters` object of a `status_line` event. The object mirrors the shape of Claude
/// Code's `statusLine` stdin JSON (`model`, `context_window`, `cost`, `rate_limits`) plus the
/// bridge's own `session_tokens`. Fields are read individually so one malformed value only drops
/// that value. Returns `None` if the object carries no usable value at all.
pub fn parse_meters(value: &Value) -> Option<CLIAgentMetersSnapshot> {
    if !value.is_object() {
        return None;
    }
    let snapshot = CLIAgentMetersSnapshot {
        model_name: label_at(value, &["model", "display_name"]),
        model_id: label_at(value, &["model", "id"]),
        context_used_pct: pct_at(value, &["context_window", "used_percentage"]).or_else(|| {
            pct_at(value, &["context_window", "remaining_percentage"]).map(|r| 100. - r)
        }),
        context_window_size: count_at(value, &["context_window", "context_window_size"])
            .filter(|size| *size > 0),
        context_input_tokens: count_at(value, &["context_window", "total_input_tokens"]),
        context_output_tokens: count_at(value, &["context_window", "total_output_tokens"]),
        session_input_tokens: count_at(value, &["session_tokens", "input"]),
        session_output_tokens: count_at(value, &["session_tokens", "output"]),
        cost_usd: number_at(value, &["cost", "total_cost_usd"]).filter(|usd| *usd >= 0.),
        lines_added: count_at(value, &["cost", "total_lines_added"]),
        lines_removed: count_at(value, &["cost", "total_lines_removed"]),
        five_hour: rate_limit_at(value, "five_hour"),
        seven_day: rate_limit_at(value, "seven_day"),
    };
    (snapshot != CLIAgentMetersSnapshot::default()).then_some(snapshot)
}

fn value_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter()
        .try_fold(value, |current, key| current.get(key))
        .filter(|v| !v.is_null())
}

fn number_at(value: &Value, path: &[&str]) -> Option<f64> {
    value_at(value, path)?.as_f64().filter(|n| n.is_finite())
}

fn pct_at(value: &Value, path: &[&str]) -> Option<f32> {
    number_at(value, path).map(|pct| pct.clamp(0., 100.) as f32)
}

fn count_at(value: &Value, path: &[&str]) -> Option<u64> {
    // `as` saturates for out-of-range floats.
    number_at(value, path)
        .filter(|n| *n >= 0.)
        .map(|n| n as u64)
}

fn label_at(value: &Value, path: &[&str]) -> Option<String> {
    value_at(value, path)?.as_str().and_then(sanitize_label)
}

fn rate_limit_at(value: &Value, window: &str) -> Option<RateLimitWindow> {
    let used_pct = pct_at(value, &["rate_limits", window, "used_percentage"])?;
    let resets_at = value_at(value, &["rate_limits", window, "resets_at"]).and_then(parse_epoch);
    Some(RateLimitWindow {
        used_pct,
        resets_at,
    })
}

/// Accepts epoch seconds, epoch milliseconds, or an RFC 3339 timestamp.
fn parse_epoch(value: &Value) -> Option<i64> {
    if let Some(number) = value.as_f64().filter(|n| n.is_finite() && *n > 0.) {
        let secs = if number > EPOCH_MILLIS_THRESHOLD {
            number / 1_000.
        } else {
            number
        };
        return Some(secs as i64);
    }
    let text = value.as_str()?.trim();
    if let Ok(secs) = text.parse::<i64>() {
        return (secs > 0).then_some(secs);
    }
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|time| time.timestamp())
}

/// Strips control and invisible formatting characters (including bidi overrides) and truncates.
/// Returns `None` if nothing printable remains.
pub fn sanitize_label(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control() && !is_invisible_format_char(*c))
        .take(MAX_LABEL_CHARS)
        .collect();
    let trimmed = cleaned.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn is_invisible_format_char(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
    )
}

#[cfg(test)]
#[path = "meters_tests.rs"]
mod tests;
