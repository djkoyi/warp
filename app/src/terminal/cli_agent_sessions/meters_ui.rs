//! Shared rendering of CLI agent usage meters for the coding agent toolbar, tabs and the sessions
//! overview. Every element here is passive (no mouse state), so it can be built during render.

use chrono::{DateTime, Datelike, Local, TimeZone, Timelike};
use pathfinder_color::ColorU;
use warpui::elements::{
    Border, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Element, Empty, Flex,
    ParentElement, Radius, Text, Wrap,
};
use warpui::fonts::{FamilyId, Properties, Weight};
use warpui::{AppContext, EntityId, SingletonEntity};

use super::meters::{
    CLIAgentActivity, CLIAgentMeters, CLIAgentMetersSnapshot, CompactionState, ContextLevel,
    GlobalMeters, RateLimitWindow, context_level, format_pct, format_reset_in, format_tokens,
    format_window_size, now_epoch_secs,
};
use super::plan::ClaudePlan;
use super::quota::{
    QUOTA_DANGER_PCT, QUOTA_WARNING_PCT, QuotaAlert, QuotaLevel, QuotaWindow, seconds_to_exhaustion,
};
use super::tab_details::format_reset_in as format_reset_in_words;
use super::{CLIAgentSession, CLIAgentSessionsModel};
use crate::appearance::Appearance;
use crate::code::editor::{add_color, remove_color};
use crate::features::FeatureFlag;
use crate::settings::AISettings;

/// Below this many seconds to the reset the usage panel shows a countdown; above it, the date.
const RESET_COUNTDOWN_MAX_SECS: i64 = 24 * 3_600;
/// Width and height (px) of a usage bar in the plan usage panel.
const USAGE_BAR_WIDTH: f32 = 220.;
const USAGE_BAR_HEIGHT: f32 = 6.;

const SEPARATOR: &str = "  ·  ";
const RELOAD_PENDING_LABEL: &str = "recarga pendiente";

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
    pub(crate) fn new(text: impl Into<String>, tone: MeterTone) -> Self {
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

pub(crate) fn rate_limit_tone(used_pct: f32) -> MeterTone {
    if used_pct >= QUOTA_DANGER_PCT {
        MeterTone::Danger
    } else if used_pct >= QUOTA_WARNING_PCT {
        MeterTone::Warning
    } else {
        MeterTone::Normal
    }
}

/// Compact colored segments for the global plan chip in the top bar: the plan name followed by the
/// usage of the current session (5-hour window) and of the week (7-day window), e.g. `Plan Ultra`,
/// `Sesión 42%`, `Semana 18%`. The cost is deliberately omitted (meaningless on a subscription). The
/// reset time lives only in the usage panel, so these segments are stable minute to minute and can
/// also drive the chip's repaint check. Whatever is unknown is skipped, so the list is empty when
/// neither the plan nor a window is available.
pub fn global_chip_segments(
    plan: Option<&ClaudePlan>,
    global: Option<&GlobalMeters>,
) -> Vec<MeterSegment> {
    let mut segments = Vec::new();
    if let Some(plan) = plan {
        segments.push(MeterSegment::new(plan.label(), MeterTone::Accent));
    }
    if let Some(window) = global.and_then(|global| global.five_hour) {
        segments.push(MeterSegment::new(
            format!("Sesión {}", format_pct(window.used_pct)),
            rate_limit_tone(window.used_pct),
        ));
    }
    if let Some(window) = global.and_then(|global| global.seven_day) {
        segments.push(MeterSegment::new(
            format!("Semana {}", format_pct(window.used_pct)),
            rate_limit_tone(window.used_pct),
        ));
    }
    segments
}

/// One limit of the plan in the usage panel: a title, a bar and a line of detail.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageRow {
    pub title: &'static str,
    /// Percentage of the window consumed, 0–100.
    pub used_pct: f32,
    pub tone: MeterTone,
    /// E.g. `42% usado · se reinicia en 2 h 13 min`.
    pub detail: String,
    /// Set when the current pace exhausts the limit before it resets, e.g.
    /// `Al ritmo actual llegas al tope en 1 h 20 min`.
    pub pace: Option<String>,
}

/// The rows of the plan usage panel (Spanish), in the order Claude shows them: current session
/// first, then the week. A window that has not been reported is skipped.
pub fn global_usage_rows(global: Option<&GlobalMeters>, now: i64) -> Vec<UsageRow> {
    let row = |title, kind: QuotaWindow, window: RateLimitWindow| UsageRow {
        title,
        pace: seconds_to_exhaustion(window, kind, now)
            .and_then(|secs| format_reset_in_words(now.saturating_add(secs), now))
            .map(|left| format!("Al ritmo actual llegas al tope en {left}")),
        used_pct: window.used_pct.clamp(0., 100.),
        tone: rate_limit_tone(window.used_pct),
        detail: match window
            .resets_at
            .and_then(|resets_at| format_reset_when(resets_at, now))
        {
            Some(when) => format!("{} usado · se reinicia {when}", format_pct(window.used_pct)),
            None => format!("{} usado", format_pct(window.used_pct)),
        },
    };
    let mut rows = Vec::new();
    if let Some(window) = global.and_then(|global| global.five_hour) {
        rows.push(row("Sesión actual (5 horas)", QuotaWindow::Session, window));
    }
    if let Some(window) = global.and_then(|global| global.seven_day) {
        rows.push(row("Semana (todos los modelos)", QuotaWindow::Week, window));
    }
    rows
}

/// Title and body (Spanish) of the desktop notification for a plan limit that crossed a threshold,
/// e.g. `Plan Ultra: sesión al 82%` / `Queda poco de la sesión actual. Se reinicia en 1 h 5 min.`
pub fn quota_alert_texts(
    alert: &QuotaAlert,
    plan: Option<&ClaudePlan>,
    now: i64,
) -> (String, String) {
    let (short, long) = match alert.window {
        QuotaWindow::Session => ("sesión", "la sesión actual (5 horas)"),
        QuotaWindow::Week => ("semana", "la semana"),
    };
    let owner = plan.map_or_else(|| "Claude".to_owned(), ClaudePlan::label);
    let title = format!("{owner}: {short} al {}", format_pct(alert.used_pct));
    let lead = match alert.level {
        QuotaLevel::Warning => format!("Queda poco de {long}."),
        QuotaLevel::Danger => format!("Casi se agota {long}."),
    };
    let body = match alert
        .resets_at
        .and_then(|resets_at| format_reset_when(resets_at, now))
    {
        Some(when) => format!("{lead} Se reinicia {when}."),
        None => lead,
    };
    (title, body)
}

/// When a limit resets, ready to follow "se reinicia": a countdown within the next day
/// (`en 2 h 13 min`), the local date and time beyond it (`el jue 26 nov, 9:00`). `None` once the
/// reset time has passed.
pub fn format_reset_when(resets_at: i64, now: i64) -> Option<String> {
    let countdown = format_reset_in_words(resets_at, now)?;
    if resets_at.saturating_sub(now) <= RESET_COUNTDOWN_MAX_SECS {
        return Some(format!("en {countdown}"));
    }
    match Local.timestamp_opt(resets_at, 0).single() {
        Some(at) => Some(format!("el {}", format_spanish_date(&at))),
        None => Some(format!("en {countdown}")),
    }
}

/// `jue 26 nov, 9:00`.
pub(crate) fn format_spanish_date<Tz: TimeZone>(at: &DateTime<Tz>) -> String {
    const WEEKDAYS: [&str; 7] = ["lun", "mar", "mié", "jue", "vie", "sáb", "dom"];
    const MONTHS: [&str; 12] = [
        "ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic",
    ];
    format!(
        "{} {} {}, {}:{:02}",
        WEEKDAYS[at.weekday().num_days_from_monday() as usize],
        at.day(),
        MONTHS[at.month0() as usize],
        at.hour(),
        at.minute()
    )
}

/// Plain colors of the plan usage panel, resolved up front so the panel can be built from a
/// `'static` tooltip closure.
#[derive(Debug, Clone, Copy)]
pub struct UsagePanelColors {
    pub background: ColorU,
    pub outline: ColorU,
    pub text: ColorU,
    pub accent: ColorU,
    pub warning: ColorU,
    pub danger: ColorU,
}

impl UsagePanelColors {
    fn tone(&self, tone: MeterTone) -> ColorU {
        match tone {
            MeterTone::Danger => self.danger,
            _ => self.warning,
        }
    }

    /// Colors for a panel drawn on the theme's tooltip background.
    pub fn for_tooltip(appearance: &Appearance) -> Self {
        let theme = appearance.theme();
        let colors = &theme.terminal_colors().normal;
        Self {
            background: theme.tooltip_background(),
            outline: theme.outline().into_solid(),
            text: theme.background().into_solid(),
            accent: theme.accent().into_solid(),
            warning: colors.yellow.into(),
            danger: colors.red.into(),
        }
    }

    fn bar(&self, tone: MeterTone) -> ColorU {
        match tone {
            MeterTone::Warning => self.warning,
            MeterTone::Danger => self.danger,
            _ => self.accent,
        }
    }

    fn faded_text(&self, alpha: u8) -> ColorU {
        ColorU::new(self.text.r, self.text.g, self.text.b, alpha)
    }
}

/// The plan usage panel shown when hovering the plan chip: the plan name and, for each limit, its
/// title, a usage bar and when it resets. Mirrors the "Plan usage limits" view of Claude.
pub fn render_usage_panel(
    plan: Option<&ClaudePlan>,
    rows: &[UsageRow],
    colors: UsagePanelColors,
    family: FamilyId,
) -> Box<dyn Element> {
    let muted = colors.faded_text(170);
    let mut column = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Start);
    column.add_child(
        Text::new_inline(
            plan.map(ClaudePlan::full_label)
                .unwrap_or_else(|| "Uso del plan".to_owned()),
            family,
            12.,
        )
        .with_color(colors.text)
        .with_style(Properties::default().weight(Weight::Semibold))
        .finish(),
    );
    if rows.is_empty() {
        column.add_child(
            Container::new(
                Text::new_inline(
                    "El uso aparece cuando Claude responde el primer mensaje.",
                    family,
                    10.,
                )
                .with_color(muted)
                .finish(),
            )
            .with_margin_top(4.)
            .finish(),
        );
    }
    for row in rows {
        column.add_child(
            Container::new(
                Text::new_inline(row.title, family, 11.)
                    .with_color(colors.text)
                    .finish(),
            )
            .with_margin_top(8.)
            .finish(),
        );
        column.add_child(
            Container::new(render_usage_bar(
                row.used_pct,
                colors.bar(row.tone),
                colors.faded_text(60),
            ))
            .with_margin_top(4.)
            .finish(),
        );
        column.add_child(
            Container::new(
                Text::new_inline(row.detail.clone(), family, 10.)
                    .with_color(muted)
                    .finish(),
            )
            .with_margin_top(3.)
            .finish(),
        );
        if let Some(pace) = &row.pace {
            // A pace that runs out is at least a warning, whatever the usage so far.
            column.add_child(
                Container::new(
                    Text::new_inline(pace.clone(), family, 10.)
                        .with_color(colors.tone(row.tone))
                        .with_style(Properties::default().weight(Weight::Semibold))
                        .finish(),
                )
                .with_margin_top(2.)
                .finish(),
            );
        }
    }
    Container::new(column.finish())
        .with_background_color(colors.background)
        .with_border(Border::all(1.).with_border_color(colors.outline))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
        .with_horizontal_padding(12.)
        .with_vertical_padding(10.)
        .finish()
}

/// A horizontal bar filled up to `used_pct`.
fn render_usage_bar(used_pct: f32, fill: ColorU, track: ColorU) -> Box<dyn Element> {
    let radius = CornerRadius::with_all(Radius::Pixels(USAGE_BAR_HEIGHT / 2.));
    let filled = USAGE_BAR_WIDTH * used_pct.clamp(0., 100.) / 100.;
    let block = |width: f32, color: ColorU| {
        ConstrainedBox::new(
            Container::new(Empty::new().finish())
                .with_background_color(color)
                .with_corner_radius(radius)
                .finish(),
        )
        .with_width(width)
        .with_height(USAGE_BAR_HEIGHT)
        .finish()
    };
    let mut bar = Flex::row();
    if filled > 0. {
        bar.add_child(block(filled, fill));
    }
    if filled < USAGE_BAR_WIDTH {
        bar.add_child(block(USAGE_BAR_WIDTH - filled, track));
    }
    bar.finish()
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
    // No cost: on a subscription the plan limits are what matters.
    if let Some(window) = snapshot.five_hour {
        segments.push(rate_limit_segment("sesión", window, now));
    }
    if let Some(window) = snapshot.seven_day {
        segments.push(rate_limit_segment("semana", window, now));
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
        .map(|left| format!(" (se reinicia en {left})"))
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
