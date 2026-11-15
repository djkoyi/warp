use std::time::SystemTime;

use ordered_float::OrderedFloat;
use warpui::elements::{Container, Expanded, Flex, Highlight, MainAxisSize, ParentElement, Text};
use warpui::fonts::{Properties, Weight};
use warpui::{AppContext, Element, SingletonEntity};

use crate::appearance::Appearance;
use crate::search::command_palette::mixer::CommandPaletteItemAction;
use crate::search::command_palette::render_util::render_search_item_icon;
use crate::search::item::IconLocation;
use crate::search::result_renderer::ItemHighlightState;
use crate::terminal::claude_sessions_index::{ClaudeSessionEntry, relative_time_es};
use crate::ui_components::icons::Icon;

/// One recent Claude Code conversation: title, project directory and how long ago it was used.
#[derive(Debug)]
pub struct SearchItem {
    entry: ClaudeSessionEntry,
    display_cwd: String,
    score: f64,
    title_indices: Vec<usize>,
}

impl SearchItem {
    pub fn new(
        entry: ClaudeSessionEntry,
        display_cwd: String,
        score: f64,
        title_indices: Vec<usize>,
    ) -> Self {
        Self {
            entry,
            display_cwd,
            score,
            title_indices,
        }
    }
}

impl crate::search::item::SearchItem for SearchItem {
    type Action = CommandPaletteItemAction;

    fn is_multiline(&self) -> bool {
        true
    }

    fn render_icon(
        &self,
        highlight_state: ItemHighlightState,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let color = appearance.theme().foreground().into_solid();
        render_search_item_icon(appearance, Icon::ClaudeLogo, color, highlight_state)
    }

    fn icon_location(&self, appearance: &Appearance) -> IconLocation {
        // Align the icon with the first (title) line, as the conversations palette does.
        let margin_top = (appearance.line_height_ratio() * appearance.monospace_font_size())
            - appearance.monospace_font_size();
        IconLocation::Top { margin_top }
    }

    fn render_item(
        &self,
        highlight_state: ItemHighlightState,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let font_size = appearance.monospace_font_size();
        let sub_text_size = font_size - 2.;
        let sub_text_color = highlight_state.sub_text_fill(appearance).into_solid();

        let mut title = Text::new_inline(
            self.entry.title.clone(),
            appearance.ui_font_family(),
            font_size,
        )
        .with_color(highlight_state.main_text_fill(appearance).into_solid())
        .with_style(Properties::default().weight(Weight::Bold));
        if !self.title_indices.is_empty() {
            let highlight = Highlight::new()
                .with_properties(Properties::default().weight(Weight::Bold))
                .with_foreground_color(highlight_state.main_text_fill(appearance).into_solid());
            title = title.with_single_highlight(highlight, self.title_indices.clone());
        }

        let cwd = Text::new_inline(
            self.display_cwd.clone(),
            appearance.ui_font_family(),
            sub_text_size,
        )
        .with_color(sub_text_color);

        let when = Container::new(
            Text::new_inline(
                relative_time_es(SystemTime::now(), self.entry.modified),
                appearance.ui_font_family(),
                sub_text_size,
            )
            .with_color(sub_text_color)
            .finish(),
        )
        .with_padding_left(8.)
        .finish();

        let left = Flex::column()
            .with_spacing(4.)
            .with_child(title.finish())
            .with_child(cwd.finish())
            .finish();

        Flex::row()
            .with_child(Expanded::new(1.0, left).finish())
            .with_child(when)
            .with_main_axis_size(MainAxisSize::Max)
            .finish()
    }

    fn score(&self) -> OrderedFloat<f64> {
        OrderedFloat::from(self.score)
    }

    fn accept_result(&self) -> Self::Action {
        CommandPaletteItemAction::ResumeClaudeSession {
            session_id: self.entry.session_id.clone(),
            cwd: self.entry.cwd.clone(),
        }
    }

    fn execute_result(&self) -> Self::Action {
        self.accept_result()
    }

    fn accessibility_label(&self) -> String {
        format!("Sesión de Claude: {}", self.entry.title)
    }

    fn accessibility_help_message(&self) -> Option<String> {
        Some(format!(
            "Pulsa Intro para retomar \"{}\" en una pestaña nueva.",
            self.entry.title
        ))
    }
}
