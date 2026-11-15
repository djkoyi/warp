//! Claude Code details for vertical tab cards and their hover sidecar, in Spanish and colored with
//! the tones of [`meters_ui`]. Every element here is passive (no mouse state), so it can be built
//! during render.

use warpui::elements::{
    ClipConfig, CrossAxisAlignment, Element, Flex, Highlight, HighlightedRange, MainAxisSize,
    ParentElement, Text,
};
use warpui::fonts::{Properties, Weight};

use super::CLIAgentSession;
use super::meters::{CLIAgentActivity, CLIAgentMeters, CLIAgentMetersSnapshot, RateLimitWindow};
use super::meters_ui::{
    MeterSegment, MeterTone, activity_tone, context_tone, rate_limit_tone, session_activity,
};
use crate::appearance::Appearance;

/// Title shown on the card until Claude names the conversation.
pub const CLAUDE_FALLBACK_TITLE: &str = "Claude Code";
/// Shown on the card while the session reports status but no meters have arrived yet.
pub const WAITING_FIRST_MESSAGE: &str = "Esperando el primer mensaje";

const ITEM_SEPARATOR: &str = " · ";

/// A piece of Claude Code data that can be shown on a vertical tab card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClaudeTabField {
    Model,
    Context,
    Tokens,
    Cost,
    RateLimits,
    Lines,
    Status,
}

impl ClaudeTabField {
    pub const ALL: [Self; 7] = [
        Self::Model,
        Self::Context,
        Self::Tokens,
        Self::Cost,
        Self::RateLimits,
        Self::Lines,
        Self::Status,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Model => "Modelo",
            Self::Context => "Contexto usado",
            Self::Tokens => "Tokens",
            Self::Cost => "Costo",
            Self::RateLimits => "Límites 5 h / 7 d",
            Self::Lines => "Líneas cambiadas",
            Self::Status => "Estado",
        }
    }

    pub fn default_enabled(self) -> bool {
        match self {
            Self::Model | Self::Context | Self::Cost | Self::Status => true,
            Self::Tokens | Self::RateLimits | Self::Lines => false,
        }
    }
}

/// What the Claude row of a tab card shows.
#[derive(Debug, Clone, PartialEq)]
pub enum ClaudeTabRow {
    /// Nothing reports data for this session, or every field is turned off.
    Hidden,
    /// The session reports status but has no data for the enabled fields yet.
    Waiting,
    /// Items joined by ` · `; the segments of one item are joined by a space.
    Items(Vec<Vec<MeterSegment>>),
}

/// The conversation title Claude Code sets as terminal title, without its leading status glyph
/// (`✳`, spinner frames). `None` while the title is still the generic one.
pub fn conversation_title(terminal_title: &str) -> Option<String> {
    let title = terminal_title
        .trim_start_matches(|c: char| !c.is_alphanumeric() && !is_leading_punctuation(c))
        .trim();
    (!title.is_empty() && !title.eq_ignore_ascii_case(CLAUDE_FALLBACK_TITLE))
        .then(|| title.to_string())
}

fn is_leading_punctuation(c: char) -> bool {
    matches!(c, '¿' | '¡' | '"' | '\'' | '(' | '[' | '~' | '/' | '.' | '#' | '@' | '$')
}

/// `carpeta · rama`: the last component of the working directory (`~` for home) and the git
/// branch when there is one.
pub fn location_line(working_directory: Option<&str>, branch: Option<&str>) -> String {
    let folder = working_directory
        .map(str::trim)
        .filter(|wd| !wd.is_empty())
        .map(folder_name)
        .unwrap_or_else(|| "~".to_string());
    match branch.map(str::trim).filter(|branch| !branch.is_empty()) {
        Some(branch) => format!("{folder}{ITEM_SEPARATOR}{branch}"),
        None => folder,
    }
}

fn folder_name(working_directory: &str) -> String {
    let trimmed = working_directory.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        return working_directory.to_string();
    }
    trimmed
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(trimmed)
        .to_string()
}

/// Whether the session reports anything beyond its existence (plugin, bridge hooks or meters).
pub fn session_reports(session: &CLIAgentSession, meters: Option<&CLIAgentMeters>) -> bool {
    session.supports_rich_status()
        || meters.is_some_and(|m| m.bridge_connected || m.snapshot.is_some())
}

/// Formats a token count with a decimal comma: `950`, `88,9k`, `1,2M`.
pub fn format_count(tokens: u64) -> String {
    if tokens < 1_000 {
        tokens.to_string()
    } else if tokens < 999_950 {
        format!("{}k", one_decimal(tokens as f64 / 1_000.))
    } else {
        format!("{}M", one_decimal(tokens as f64 / 1_000_000.))
    }
}

fn one_decimal(value: f64) -> String {
    let text = format!("{value:.1}");
    text.strip_suffix(".0")
        .map(str::to_owned)
        .unwrap_or(text)
        .replace('.', ",")
}

pub fn format_pct(pct: f32) -> String {
    format!("{:.0} %", pct.clamp(0., 100.))
}

/// Formats a cost in dollars with a decimal comma: `$0,42`, `$1,20`, `$12`.
pub fn format_cost(usd: f64) -> String {
    let usd = usd.max(0.);
    if usd < 10. {
        format!("${usd:.2}").replace('.', ",")
    } else {
        format!("${usd:.0}")
    }
}

/// Time left until `resets_at` (epoch seconds): `45 min`, `2 h 13 min`, `3 d 4 h`. `None` once
/// the reset time has passed.
pub fn format_reset_in(resets_at: i64, now: i64) -> Option<String> {
    let remaining = resets_at.checked_sub(now)?;
    if remaining <= 0 {
        return None;
    }
    let minutes = remaining.saturating_add(59) / 60;
    let (days, hours, mins) = (minutes / 1_440, (minutes % 1_440) / 60, minutes % 60);
    Some(if days > 0 {
        format!("{days} d {hours} h")
    } else if hours > 0 {
        format!("{hours} h {mins} min")
    } else {
        format!("{mins} min")
    })
}

pub fn activity_label(activity: CLIAgentActivity) -> &'static str {
    match activity {
        CLIAgentActivity::Idle => "En espera",
        CLIAgentActivity::Working => "Trabajando",
        CLIAgentActivity::Compacting => "Compactando",
        CLIAgentActivity::WaitingForPermission => "Necesita permiso",
        CLIAgentActivity::WaitingForInput => "Esperando tu respuesta",
        CLIAgentActivity::Done => "Terminado",
        CLIAgentActivity::Failed => "Falló",
        CLIAgentActivity::Cancelled => "Cancelado",
    }
}

/// The model name without the `Claude ` prefix, or the model id when there is no name.
pub fn model_label(snapshot: &CLIAgentMetersSnapshot) -> Option<String> {
    let model = snapshot
        .model_name
        .as_deref()
        .or(snapshot.model_id.as_deref())?
        .trim();
    let model = model.strip_prefix("Claude ").unwrap_or(model).trim();
    (!model.is_empty()).then(|| model.to_string())
}

fn session_tokens(snapshot: &CLIAgentMetersSnapshot) -> Option<u64> {
    match (
        snapshot.session_input_tokens,
        snapshot.session_output_tokens,
    ) {
        (None, None) => None,
        (input, output) => Some(input.unwrap_or(0).saturating_add(output.unwrap_or(0))),
    }
}

fn lines_segments(snapshot: &CLIAgentMetersSnapshot) -> Option<Vec<MeterSegment>> {
    let (added, removed) = (
        snapshot.lines_added.unwrap_or(0),
        snapshot.lines_removed.unwrap_or(0),
    );
    if added == 0 && removed == 0 {
        return None;
    }
    Some(vec![
        MeterSegment::new(format!("+{added}"), MeterTone::Success),
        MeterSegment::new(format!("−{removed}"), MeterTone::Danger),
    ])
}

fn rate_limit_segment(label: &str, window: RateLimitWindow) -> MeterSegment {
    MeterSegment::new(
        format!("{label} {}", format_pct(window.used_pct)),
        rate_limit_tone(window.used_pct),
    )
}

/// The Claude row of a tab card. `reports` is [`session_reports`]; `enabled` says which fields the
/// user turned on.
pub fn tab_row(
    reports: bool,
    activity: Option<CLIAgentActivity>,
    meters: Option<&CLIAgentMeters>,
    threshold: u8,
    enabled: impl Fn(ClaudeTabField) -> bool,
) -> ClaudeTabRow {
    if !reports || !ClaudeTabField::ALL.into_iter().any(&enabled) {
        return ClaudeTabRow::Hidden;
    }
    let snapshot = meters.and_then(|m| m.snapshot.as_ref());
    let mut items = Vec::new();
    for field in ClaudeTabField::ALL.into_iter().filter(|field| enabled(*field)) {
        match field {
            ClaudeTabField::Model => {
                if let Some(model) = snapshot.and_then(model_label) {
                    items.push(vec![MeterSegment::new(model, MeterTone::Normal)]);
                }
            }
            ClaudeTabField::Context => {
                if let Some(used) = snapshot.and_then(|s| s.context_used_pct) {
                    items.push(vec![MeterSegment::new(
                        format!("ctx {}", format_pct(used)),
                        context_tone(used, threshold),
                    )]);
                }
            }
            ClaudeTabField::Tokens => {
                if let Some(tokens) = snapshot.and_then(session_tokens) {
                    items.push(vec![MeterSegment::new(
                        format!("{} tok", format_count(tokens)),
                        MeterTone::Muted,
                    )]);
                }
            }
            ClaudeTabField::Cost => {
                if let Some(cost) = snapshot.and_then(|s| s.cost_usd) {
                    items.push(vec![MeterSegment::new(format_cost(cost), MeterTone::Normal)]);
                }
            }
            ClaudeTabField::RateLimits => {
                if let Some(window) = snapshot.and_then(|s| s.five_hour) {
                    items.push(vec![rate_limit_segment("5 h", window)]);
                }
                if let Some(window) = snapshot.and_then(|s| s.seven_day) {
                    items.push(vec![rate_limit_segment("7 d", window)]);
                }
            }
            ClaudeTabField::Lines => {
                if let Some(lines) = snapshot.and_then(lines_segments) {
                    items.push(lines);
                }
            }
            ClaudeTabField::Status => {
                if let Some(activity) = activity.filter(|a| *a != CLIAgentActivity::Idle) {
                    items.push(vec![MeterSegment::new(
                        activity_label(activity),
                        activity_tone(activity),
                    )]);
                }
            }
        }
    }
    if items.is_empty() {
        ClaudeTabRow::Waiting
    } else {
        ClaudeTabRow::Items(items)
    }
}

/// [`tab_row`] for a live session.
pub fn tab_row_for_session(
    session: &CLIAgentSession,
    meters: Option<&CLIAgentMeters>,
    threshold: u8,
    enabled: impl Fn(ClaudeTabField) -> bool,
) -> ClaudeTabRow {
    tab_row(
        session_reports(session, meters),
        session_activity(session, meters),
        meters,
        threshold,
        enabled,
    )
}

/// Labeled lines with everything known about the session, for the hover sidecar.
pub fn detail_lines(
    activity: Option<CLIAgentActivity>,
    meters: &CLIAgentMeters,
    threshold: u8,
    now: i64,
) -> Vec<(&'static str, Vec<MeterSegment>)> {
    let mut lines = Vec::new();
    if let Some(activity) = activity {
        lines.push((
            "Estado",
            vec![MeterSegment::new(
                activity_label(activity),
                activity_tone(activity),
            )],
        ));
    }
    if let Some(snapshot) = &meters.snapshot {
        if let Some(model) = model_label(snapshot) {
            let mut segments = vec![MeterSegment::new(model.clone(), MeterTone::Normal)];
            if let Some(id) = snapshot.model_id.as_ref().filter(|id| **id != model) {
                segments.push(MeterSegment::new(format!("({id})"), MeterTone::Muted));
            }
            lines.push(("Modelo", segments));
        }
        if let Some(segments) = context_detail(snapshot, threshold) {
            lines.push(("Contexto", segments));
        }
        if let (Some(input), Some(output)) = (
            snapshot.context_input_tokens,
            snapshot.context_output_tokens,
        ) {
            lines.push((
                "Último turno",
                vec![MeterSegment::new(
                    format!(
                        "entrada {} · salida {}",
                        format_count(input),
                        format_count(output)
                    ),
                    MeterTone::Muted,
                )],
            ));
        }
        if snapshot.session_input_tokens.is_some() || snapshot.session_output_tokens.is_some() {
            lines.push((
                "Sesión",
                vec![MeterSegment::new(
                    format!(
                        "entrada {} · salida {}",
                        format_count(snapshot.session_input_tokens.unwrap_or(0)),
                        format_count(snapshot.session_output_tokens.unwrap_or(0))
                    ),
                    MeterTone::Normal,
                )],
            ));
        }
        if let Some(cost) = snapshot.cost_usd {
            lines.push((
                "Costo",
                vec![MeterSegment::new(format_cost(cost), MeterTone::Normal)],
            ));
        }
        if let Some(segments) = lines_segments(snapshot) {
            lines.push(("Líneas", segments));
        }
        if let Some(window) = snapshot.five_hour {
            lines.push(("Límite 5 h", rate_limit_detail(window, now)));
        }
        if let Some(window) = snapshot.seven_day {
            lines.push(("Límite 7 d", rate_limit_detail(window, now)));
        }
    }
    if let Some(segments) = compaction_detail(meters) {
        lines.push(("Compactación", segments));
    }
    lines
}

fn context_detail(snapshot: &CLIAgentMetersSnapshot, threshold: u8) -> Option<Vec<MeterSegment>> {
    let window = snapshot.context_window_size;
    let Some(used) = snapshot.context_used_pct else {
        return window.map(|size| {
            vec![MeterSegment::new(
                format!("ventana de {} tok", format_count(size)),
                MeterTone::Normal,
            )]
        });
    };
    let tokens = match (snapshot.context_input_tokens, window) {
        (Some(input), Some(size)) => {
            format!("{} de {} tok ", format_count(input), format_count(size))
        }
        (None, Some(size)) => format!("de {} tok ", format_count(size)),
        (Some(input), None) => format!("{} tok ", format_count(input)),
        (None, None) => String::new(),
    };
    let mut segments = vec![MeterSegment::new(
        format!("{tokens}({})", format_pct(used)),
        context_tone(used, threshold),
    )];
    if let Some(left) = snapshot.context_remaining_pct() {
        segments.push(MeterSegment::new(
            format!("quedan {}", format_pct(left)),
            MeterTone::Muted,
        ));
    }
    Some(segments)
}

fn rate_limit_detail(window: RateLimitWindow, now: i64) -> Vec<MeterSegment> {
    let mut segments = vec![MeterSegment::new(
        format!("{} usado", format_pct(window.used_pct)),
        rate_limit_tone(window.used_pct),
    )];
    if let Some(left) = window
        .resets_at
        .and_then(|resets_at| format_reset_in(resets_at, now))
    {
        segments.push(MeterSegment::new(
            format!("se reinicia en {left}"),
            MeterTone::Muted,
        ));
    }
    segments
}

fn compaction_detail(meters: &CLIAgentMeters) -> Option<Vec<MeterSegment>> {
    let compaction = &meters.compaction;
    let mut segments = Vec::new();
    if let Some(trigger) = compaction.in_progress {
        segments.push(MeterSegment::new(
            format!("compactando ({})", trigger.label()),
            MeterTone::Accent,
        ));
    } else if compaction.count > 0 {
        let times = if compaction.count == 1 {
            "1 vez".to_string()
        } else {
            format!("{} veces", compaction.count)
        };
        let last = compaction
            .last_trigger
            .map(|trigger| format!(", la última {}", trigger.label()))
            .unwrap_or_default();
        segments.push(MeterSegment::new(
            format!("{times}{last}"),
            MeterTone::Muted,
        ));
    }
    if compaction.last_backup_path.is_some() {
        segments.push(MeterSegment::new(
            "historial respaldado",
            MeterTone::Muted,
        ));
    }
    (!segments.is_empty()).then_some(segments)
}

/// The plain text of a tab row and, for every segment, its char range and tone.
fn row_text(items: &[Vec<MeterSegment>]) -> (String, Vec<(std::ops::Range<usize>, MeterTone)>) {
    let mut text = String::new();
    let mut ranges = Vec::new();
    let mut chars = 0;
    for (item_index, item) in items.iter().enumerate() {
        if item_index > 0 {
            text.push_str(ITEM_SEPARATOR);
            chars += ITEM_SEPARATOR.chars().count();
        }
        for (segment_index, segment) in item.iter().enumerate() {
            if segment_index > 0 {
                text.push(' ');
                chars += 1;
            }
            let len = segment.text.chars().count();
            text.push_str(&segment.text);
            ranges.push((chars..chars + len, segment.tone));
            chars += len;
        }
    }
    (text, ranges)
}

/// Renders a [`ClaudeTabRow`] on one line, clipped with an ellipsis. `None` when hidden.
pub fn render_tab_row(
    row: &ClaudeTabRow,
    font_size: f32,
    appearance: &Appearance,
) -> Option<Box<dyn Element>> {
    let family = appearance.ui_font_family();
    let muted = MeterTone::Muted.color(appearance);
    match row {
        ClaudeTabRow::Hidden => None,
        ClaudeTabRow::Waiting => Some(
            Text::new_inline(WAITING_FIRST_MESSAGE, family, font_size)
                .with_clip(ClipConfig::ellipsis())
                .with_color(muted)
                .finish(),
        ),
        ClaudeTabRow::Items(items) => {
            let (text, ranges) = row_text(items);
            let highlights = ranges.into_iter().filter(|(range, _)| !range.is_empty()).map(
                |(range, tone)| {
                    let mut highlight = Highlight::new().with_foreground_color(tone.color(appearance));
                    if matches!(tone, MeterTone::Warning | MeterTone::Danger) {
                        highlight =
                            highlight.with_properties(Properties::default().weight(Weight::Semibold));
                    }
                    HighlightedRange {
                        highlight,
                        highlight_indices: range.collect(),
                    }
                },
            );
            Some(
                Text::new_inline(text, family, font_size)
                    .with_clip(ClipConfig::ellipsis())
                    .with_color(muted)
                    .with_highlights(highlights)
                    .finish(),
            )
        }
    }
}

/// Renders the labeled lines of [`detail_lines`] for the hover sidecar.
pub fn render_detail_lines(
    lines: &[(&'static str, Vec<MeterSegment>)],
    font_size: f32,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let family = appearance.ui_font_family();
    let label_color = MeterTone::Muted.color(appearance);
    let mut column = Flex::column()
        .with_main_axis_size(MainAxisSize::Min)
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_spacing(2.);
    for (label, segments) in lines {
        let (value, ranges) = row_text(std::slice::from_ref(segments));
        let label_text = format!("{label}: ");
        let offset = label_text.chars().count();
        let highlights = ranges
            .into_iter()
            .filter(|(range, _)| !range.is_empty())
            .map(|(range, tone)| {
                let mut highlight = Highlight::new().with_foreground_color(tone.color(appearance));
                if matches!(tone, MeterTone::Warning | MeterTone::Danger) {
                    highlight =
                        highlight.with_properties(Properties::default().weight(Weight::Semibold));
                }
                HighlightedRange {
                    highlight,
                    highlight_indices: (range.start + offset..range.end + offset).collect(),
                }
            });
        column.add_child(
            Text::new(format!("{label_text}{value}"), family, font_size)
                .soft_wrap(true)
                .with_color(label_color)
                .with_highlights(highlights)
                .finish(),
        );
    }
    column.finish()
}

#[cfg(test)]
#[path = "tab_details_tests.rs"]
mod tests;
