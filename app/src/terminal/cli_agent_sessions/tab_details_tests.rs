use super::*;
use crate::terminal::cli_agent_sessions::meters::{CompactTrigger, CompactionState};

fn meters_with(snapshot: CLIAgentMetersSnapshot) -> CLIAgentMeters {
    CLIAgentMeters {
        snapshot: Some(snapshot),
        ..Default::default()
    }
}

fn row_texts(row: &ClaudeTabRow) -> Vec<String> {
    match row {
        ClaudeTabRow::Items(items) => items
            .iter()
            .map(|item| {
                item.iter()
                    .map(|segment| segment.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect(),
        ClaudeTabRow::Hidden | ClaudeTabRow::Waiting => Vec::new(),
    }
}

fn full_snapshot() -> CLIAgentMetersSnapshot {
    CLIAgentMetersSnapshot {
        model_name: Some("Claude Opus 5.5".to_string()),
        model_id: Some("claude-opus-5-5".to_string()),
        context_used_pct: Some(32.4),
        context_window_size: Some(200_000),
        context_input_tokens: Some(64_200),
        context_output_tokens: Some(1_250),
        session_input_tokens: Some(80_000),
        session_output_tokens: Some(8_900),
        cost_usd: Some(1.2),
        lines_added: Some(120),
        lines_removed: Some(8),
        five_hour: Some(RateLimitWindow {
            used_pct: 40.,
            resets_at: Some(10_000 + 2 * 3_600 + 13 * 60),
        }),
        seven_day: Some(RateLimitWindow {
            used_pct: 12.,
            resets_at: None,
        }),
    }
}

#[test]
fn conversation_title_strips_status_glyphs() {
    assert_eq!(
        conversation_title("✳ Saludo y conversación inicial").as_deref(),
        Some("Saludo y conversación inicial")
    );
    assert_eq!(
        conversation_title("⠋  Refactor del parser").as_deref(),
        Some("Refactor del parser")
    );
    assert_eq!(
        conversation_title("✻ ¿Qué hace esto?").as_deref(),
        Some("¿Qué hace esto?")
    );
    assert_eq!(conversation_title("Sin glifo").as_deref(), Some("Sin glifo"));
}

#[test]
fn conversation_title_is_none_for_the_generic_title() {
    assert_eq!(conversation_title("✳ Claude Code"), None);
    assert_eq!(conversation_title("✳ "), None);
    assert_eq!(conversation_title(""), None);
}

#[test]
fn location_line_uses_folder_and_branch() {
    assert_eq!(
        location_line(Some("~/proyectos/warp-propio"), Some("css/integracion")),
        "warp-propio · css/integracion"
    );
    assert_eq!(location_line(Some("~"), None), "~");
    assert_eq!(location_line(None, Some("main")), "~ · main");
    assert_eq!(location_line(Some("/tmp/demo/"), Some("  ")), "demo");
    assert_eq!(location_line(Some("/"), None), "/");
}

#[test]
fn counts_use_decimal_comma() {
    assert_eq!(format_count(950), "950");
    assert_eq!(format_count(88_900), "88,9k");
    assert_eq!(format_count(15_000), "15k");
    assert_eq!(format_count(999_960), "1M");
    assert_eq!(format_count(1_250_000), "1,2M");
}

#[test]
fn percentages_and_costs_use_spanish_format() {
    assert_eq!(format_pct(32.4), "32 %");
    assert_eq!(format_pct(140.), "100 %");
    assert_eq!(format_cost(1.2), "$1,20");
    assert_eq!(format_cost(0.042), "$0,04");
    assert_eq!(format_cost(12.6), "$13");
}

#[test]
fn reset_times_are_spelled_out() {
    assert_eq!(format_reset_in(3_000, 300).as_deref(), Some("45 min"));
    assert_eq!(
        format_reset_in(2 * 3_600 + 13 * 60, 0).as_deref(),
        Some("2 h 13 min")
    );
    assert_eq!(
        format_reset_in(3 * 86_400 + 4 * 3_600, 0).as_deref(),
        Some("3 d 4 h")
    );
    assert_eq!(format_reset_in(10, 20), None);
}

#[test]
fn tab_row_shows_default_fields() {
    let meters = meters_with(full_snapshot());
    let row = tab_row(
        true,
        Some(CLIAgentActivity::Working),
        Some(&meters),
        80,
        ClaudeTabField::default_enabled,
    );
    assert_eq!(
        row_texts(&row),
        vec!["Opus 5.5", "ctx 32 %", "$1,20", "Trabajando"]
    );
}

#[test]
fn tab_row_shows_every_field_when_enabled() {
    let meters = meters_with(full_snapshot());
    let row = tab_row(
        true,
        Some(CLIAgentActivity::WaitingForInput),
        Some(&meters),
        80,
        |_| true,
    );
    assert_eq!(
        row_texts(&row),
        vec![
            "Opus 5.5",
            "ctx 32 %",
            "88,9k tok",
            "$1,20",
            "5 h 40 %",
            "7 d 12 %",
            "+120 −8",
            "Esperando tu respuesta",
        ]
    );
}

#[test]
fn tab_row_colors_context_and_status() {
    let meters = meters_with(CLIAgentMetersSnapshot {
        context_used_pct: Some(95.),
        ..Default::default()
    });
    let ClaudeTabRow::Items(items) = tab_row(
        true,
        Some(CLIAgentActivity::WaitingForPermission),
        Some(&meters),
        80,
        |_| true,
    ) else {
        panic!("expected items");
    };
    assert_eq!(items[0][0].tone, MeterTone::Danger);
    assert_eq!(items[1][0].text, "Necesita permiso");
    assert_eq!(items[1][0].tone, MeterTone::Warning);
}

#[test]
fn tab_row_waits_for_the_first_message() {
    let meters = CLIAgentMeters {
        bridge_connected: true,
        ..Default::default()
    };
    let row = tab_row(
        true,
        Some(CLIAgentActivity::Idle),
        Some(&meters),
        80,
        ClaudeTabField::default_enabled,
    );
    assert_eq!(row, ClaudeTabRow::Waiting);
}

#[test]
fn tab_row_is_hidden_without_reports_or_fields() {
    let meters = meters_with(full_snapshot());
    assert_eq!(
        tab_row(false, None, Some(&meters), 80, |_| true),
        ClaudeTabRow::Hidden
    );
    assert_eq!(
        tab_row(true, None, Some(&meters), 80, |_| false),
        ClaudeTabRow::Hidden
    );
}

#[test]
fn row_text_tracks_char_ranges_of_segments() {
    let items = vec![
        vec![MeterSegment::new("ctx 32 %", MeterTone::Normal)],
        vec![
            MeterSegment::new("+120", MeterTone::Success),
            MeterSegment::new("−8", MeterTone::Danger),
        ],
    ];
    let (text, ranges) = row_text(&items);
    assert_eq!(text, "ctx 32 % · +120 −8");
    assert_eq!(ranges[0].0, 0..8);
    assert_eq!(ranges[1].0, 11..15);
    assert_eq!(ranges[2].0, 16..18);
}

#[test]
fn detail_lines_include_everything_known() {
    let meters = CLIAgentMeters {
        snapshot: Some(full_snapshot()),
        compaction: CompactionState {
            count: 2,
            last_trigger: Some(CompactTrigger::Auto),
            ..Default::default()
        },
        ..Default::default()
    };
    let lines = detail_lines(Some(CLIAgentActivity::Working), &meters, 80, 10_000);
    let rendered: Vec<String> = lines
        .iter()
        .map(|(label, items)| format!("{label}: {}", row_text(items).0))
        .collect();
    assert_eq!(
        rendered,
        vec![
            "Estado: Trabajando",
            "Modelo: Opus 5.5 (claude-opus-5-5)",
            "Contexto: 64,2k de 200k tok (32 %) · quedan 68 %",
            "Último turno: entrada 64,2k · salida 1,2k",
            "Tokens de la sesión: entrada 80k · salida 8,9k",
            "Costo: $1,20",
            "Líneas: +120 −8",
            "Límite 5 h: 40 % usado · se reinicia en 2 h 13 min",
            "Límite 7 d: 12 % usado",
            "Compactación: 2 veces, la última auto",
        ]
    );
}
