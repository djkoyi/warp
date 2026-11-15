//! Shared rendering of CLI agent usage meters for the coding agent toolbar, tabs and the sessions
//! overview. Every element here is passive (no mouse state), so it can be built during render.

use pathfinder_color::ColorU;
use warpui::elements::{Element, Flex, ParentElement, Text, Wrap};
use warpui::fonts::{Properties, Weight};
use warpui::{AppContext, EntityId, SingletonEntity};

use super::{CLIAgentSession, CLIAgentSessionsModel};
use super::meters::{
    CLIAgentActivity, CLIAgentMeters, CLIAgentMetersSnapshot, CompactionState, ContextLevel,
    RateLimitWindow, context_level, format_cost, format_pct, format_reset_in, format_tokens,
    format_window_size, now_epoch_secs,
};
use crate::appearance::Appearance;
use crate::code::editor::{add_color, remove_color};
use crate::features::FeatureFlag;
use crate::settings::AISettings;

/// Plan usage (percent) at which a rate-limit meter turns to a warning color.
const RATE_LIMIT_WARNING_PCT: f32 = 80.;
const RATE_LIMIT_DANGER_PCT: f32 = 95.;

const SEPARATOR: &str = "  ·  ";
const RELOAD_PENDING_LABEL: &str = "pendiente de recarga";

/// Semantic color of a meter segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeterTone {
    Muted,
    Normal,
    Success,
    Warning,
    Danger,
    Accent,
}

impl MeterTone {
    pub fn color(self, appearance: &Appearance) -> ColorU {
        let theme = appearance.theme();
        let colors = &theme.terminal_colors().normal;
        match self {
            Self::Muted => theme.sub_text_color(theme.background()).into_solid(),
            Self::Normal => theme.main_text_color(theme.background()).into_solid(),
            Self::Success => colors.green.into(),
            Self::Warning => colors.yellow.into(),
            Self::Danger => colors.red.into(),
            Self::Accent => theme.accent().into_solid(),
        }
    }
}

/// One colored piece of meter text.
#[derive(Debug, Clone, PartialEq)]
pub struct MeterSegment {
    pub text: String,
    pub tone: MeterTone,
}

impl MeterSegment {
    fn new(text: impl Into<String>, tone: MeterTone) -> Self {
        Self {
            text: text.into(),
            tone,
        }
    }
}

/// Whether meters should be rendered at all.
pub fn meters_enabled(app: &AppContext) -> bool {
    FeatureFlag::CLIAgentMeters.is_enabled() && *AISettings::as_ref(app).show_cli_agent_meters
}

/// The compaction warning threshold from settings, clamped to a usable range.
pub fn compaction_warning_threshold(app: &AppContext) -> u8 {
    (*AISettings::as_ref(app).cli_agent_compaction_warning_threshold).clamp(1, 100)
}

/// The activity to show for a session, or `None` when the session reports no lifecycle events
/// (no plugin and no bridge hooks), so its status would be a guess.
pub fn session_activity(
    session: &CLIAgentSession,
    meters: Option<&CLIAgentMeters>,
) -> Option<CLIAgentActivity> {
    session
        .supports_rich_status()
        .then(|| CLIAgentActivity::from_session(&session.status, meters))
}

pub fn activity_tone(activity: CLIAgentActivity) -> MeterTone {
    match activity {
        CLIAgentActivity::Idle | CLIAgentActivity::Cancelled => MeterTone::Muted,
        CLIAgentActivity::Working | CLIAgentActivity::Compacting => MeterTone::Accent,
        CLIAgentActivity::WaitingForPermission | CLIAgentActivity::WaitingForInput => {
            MeterTone::Warning
        }
        CLIAgentActivity::Done => MeterTone::Success,
        CLIAgentActivity::Failed => MeterTone::Danger,
    }
}

pub fn context_tone(used_pct: f32, threshold: u8) -> MeterTone {
    match context_level(used_pct, threshold) {
        ContextLevel::Normal => MeterTone::Normal,
        ContextLevel::Warning => MeterTone::Warning,
        ContextLevel::Critical => MeterTone::Danger,
    }
}

fn rate_limit_tone(used_pct: f32) -> MeterTone {
    if used_pct >= RATE_LIMIT_DANGER_PCT {
        MeterTone::Danger
    } else if used_pct >= RATE_LIMIT_WARNING_PCT {
        MeterTone::Warning
    } else {
        MeterTone::Normal
    }
}

/// Short segments for a tab: context usage and activity, e.g. `42%` `waiting for you`.
pub fn tab_segments(
    activity: Option<CLIAgentActivity>,
    meters: Option<&CLIAgentMeters>,
    threshold: u8,
) -> Vec<MeterSegment> {
    let mut segments = Vec::new();
    if let Some(used) = meters
        .and_then(|m| m.snapshot.as_ref())
        .and_then(|s| s.context_used_pct)
    {
        segments.push(MeterSegment::new(
            format_pct(used),
            context_tone(used, threshold),
        ));
    }
    if let Some(activity) = activity.filter(|a| *a != CLIAgentActivity::Idle) {
        segments.push(MeterSegment::new(activity.label(), activity_tone(activity)));
    }
    segments
}

/// The [`tab_segments`] of the CLI agent session in `terminal_view_id` (none when meters are off),
/// plus a pending-reload marker. Empty when the terminal has no session.
pub fn terminal_tab_segments(terminal_view_id: EntityId, app: &AppContext) -> Vec<MeterSegment> {
    let sessions = CLIAgentSessionsModel::as_ref(app);
    let Some(session) = sessions.session(terminal_view_id) else {
        return Vec::new();
    };
    let mut segments = if meters_enabled(app) {
        let meters = sessions.meters(terminal_view_id);
        tab_segments(
            session_activity(session, meters),
            meters,
            compaction_warning_threshold(app),
        )
    } else {
        Vec::new()
    };
    if sessions.is_reload_pending(terminal_view_id) {
        segments.push(MeterSegment::new(RELOAD_PENDING_LABEL, MeterTone::Warning));
    }
    segments
}

/// Full meter segments for the coding agent toolbar.
pub fn strip_segments(
    activity: Option<CLIAgentActivity>,
    meters: &CLIAgentMeters,
    threshold: u8,
    now: i64,
) -> Vec<MeterSegment> {
    let mut segments = Vec::new();
    if let Some(activity) = activity {
        segments.push(MeterSegment::new(activity.label(), activity_tone(activity)));
    }

    let compaction = &meters.compaction;
    if let Some(trigger) = compaction.in_progress {
        segments.push(MeterSegment::new(
            format!("compacting ({})", trigger.label()),
            MeterTone::Accent,
        ));
    } else if compaction.count > 0 {
        let last = compaction
            .last_trigger
            .map(|trigger| format!(", last {}", trigger.label()))
            .unwrap_or_default();
        segments.push(MeterSegment::new(
            format!("compacted ×{}{last}", compaction.count),
            MeterTone::Muted,
        ));
    }
    if compaction.last_backup_path.is_some() {
        segments.push(MeterSegment::new("transcript backed up", MeterTone::Muted));
    }

    let Some(snapshot) = &meters.snapshot else {
        return segments;
    };
    if let Some(model) = snapshot.model_name.as_ref().or(snapshot.model_id.as_ref()) {
        segments.push(MeterSegment::new(model.clone(), MeterTone::Muted));
    }
    segments.extend(context_segments(snapshot, threshold));
    if let Some(tokens) = token_segment(snapshot) {
        segments.push(tokens);
    }
    if let Some(cost) = snapshot.cost_usd {
        segments.push(MeterSegment::new(format_cost(cost), MeterTone::Normal));
    }
    if let Some(window) = snapshot.five_hour {
        segments.push(rate_limit_segment("5h", window, now));
    }
    if let Some(window) = snapshot.seven_day {
        segments.push(rate_limit_segment("7d", window, now));
    }
    segments
}

fn context_segments(snapshot: &CLIAgentMetersSnapshot, threshold: u8) -> Vec<MeterSegment> {
    let Some(used) = snapshot.context_used_pct else {
        return snapshot
            .context_window_size
            .map(|size| {
                vec![MeterSegment::new(
                    format!("ctx {}", format_window_size(size)),
                    MeterTone::Normal,
                )]
            })
            .unwrap_or_default();
    };
    let window = snapshot
        .context_window_size
        .map(|size| format!(" of {}", format_window_size(size)))
        .unwrap_or_default();
    let remaining = snapshot
        .context_remaining_pct()
        .map(|left| format!(" ({} left)", format_pct(left)))
        .unwrap_or_default();
    let mut segments = vec![MeterSegment::new(
        format!("ctx {}{window}{remaining}", format_pct(used)),
        context_tone(used, threshold),
    )];
    match context_level(used, threshold) {
        ContextLevel::Normal => {}
        ContextLevel::Warning => {
            segments.push(MeterSegment::new("compaction near", MeterTone::Warning));
        }
        ContextLevel::Critical => {
            segments.push(MeterSegment::new("compaction imminent", MeterTone::Danger));
        }
    }
    segments
}

fn token_segment(snapshot: &CLIAgentMetersSnapshot) -> Option<MeterSegment> {
    let (label, input, output) = match (
        snapshot.session_input_tokens,
        snapshot.session_output_tokens,
    ) {
        (Some(input), Some(output)) => ("session", input, output),
        _ => (
            "last turn",
            snapshot.context_input_tokens?,
            snapshot.context_output_tokens?,
        ),
    };
    Some(MeterSegment::new(
        format!(
            "{label} in {} / out {}",
            format_tokens(input),
            format_tokens(output)
        ),
        MeterTone::Muted,
    ))
}

fn rate_limit_segment(label: &str, window: RateLimitWindow, now: i64) -> MeterSegment {
    let reset = window
        .resets_at
        .and_then(|resets_at| format_reset_in(resets_at, now))
        .map(|left| format!(" (resets {left})"))
        .unwrap_or_default();
    MeterSegment::new(
        format!("{label} {}{reset}", format_pct(window.used_pct)),
        rate_limit_tone(window.used_pct),
    )
}

/// Lines added/removed, rendered in the diff colors.
fn lines_elements(
    snapshot: &CLIAgentMetersSnapshot,
    font_size: f32,
    appearance: &Appearance,
) -> Option<Box<dyn Element>> {
    let (added, removed) = (snapshot.lines_added?, snapshot.lines_removed?);
    let family = appearance.ui_font_family();
    Some(
        Flex::row()
            .with_child(
                Text::new_inline(format!("+{added}"), family, font_size)
                    .with_color(add_color(appearance))
                    .finish(),
            )
            .with_child(
                Text::new_inline(format!(" −{removed}"), family, font_size)
                    .with_color(remove_color(appearance))
                    .finish(),
            )
            .finish(),
    )
}

pub fn render_segment(
    segment: &MeterSegment,
    font_size: f32,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let mut text = Text::new_inline(segment.text.clone(), appearance.ui_font_family(), font_size)
        .with_color(segment.tone.color(appearance));
    if matches!(segment.tone, MeterTone::Warning | MeterTone::Danger) {
        text = text.with_style(Properties::default().weight(Weight::Semibold));
    }
    text.finish()
}

/// Renders segments inline, separated by dots, without wrapping (for tabs).
pub fn render_inline_segments(
    segments: &[MeterSegment],
    font_size: f32,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let mut row = Flex::row();
    for (index, segment) in segments.iter().enumerate() {
        if index > 0 {
            row.add_child(render_separator(" · ", font_size, appearance));
        }
        row.add_child(render_segment(segment, font_size, appearance));
    }
    row.finish()
}

fn render_separator(
    text: &'static str,
    font_size: f32,
    appearance: &Appearance,
) -> Box<dyn Element> {
    Text::new_inline(text, appearance.ui_font_family(), font_size)
        .with_color(MeterTone::Muted.color(appearance))
        .finish()
}

/// Renders the full meters line for the coding agent toolbar. Wraps onto several lines in narrow
/// panes rather than clipping. Returns `None` when there is nothing to show.
pub fn render_meters_strip(
    session: &CLIAgentSession,
    meters: &CLIAgentMeters,
    threshold: u8,
    appearance: &Appearance,
) -> Option<Box<dyn Element>> {
    // Without the bridge there is nothing beyond the status the toolbar already conveys.
    if meters.snapshot.is_none() && meters.compaction == CompactionState::default() {
        return None;
    }
    let font_size = (appearance.ui_font_size() - 1.).max(9.);
    let activity = session_activity(session, Some(meters));
    let segments = strip_segments(activity, meters, threshold, now_epoch_secs());
    let lines = meters
        .snapshot
        .as_ref()
        .and_then(|snapshot| lines_elements(snapshot, font_size, appearance));
    if segments.is_empty() && lines.is_none() {
        return None;
    }

    let mut wrap = Wrap::row().with_run_spacing(2.);
    let mut is_first = true;
    let mut push = |wrap: &mut Wrap, element: Box<dyn Element>| {
        if is_first {
            wrap.add_child(element);
        } else {
            wrap.add_child(
                Flex::row()
                    .with_child(render_separator(SEPARATOR, font_size, appearance))
                    .with_child(element)
                    .finish(),
            );
        }
        is_first = false;
    };
    for segment in &segments {
        push(&mut wrap, render_segment(segment, font_size, appearance));
    }
    if let Some(lines) = lines {
        push(&mut wrap, lines);
    }
    Some(wrap.finish())
}

#[cfg(test)]
#[path = "meters_ui_tests.rs"]
mod tests;
