//! Overview of every CLI agent session (e.g. Claude Code) in the window, with context usage, cost
//! and status per tab. Clicking a row focuses that pane.

use pathfinder_color::ColorU;
use pathfinder_geometry::vector::vec2f;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::Fill;
use warp_core::ui::theme::color::internal_colors;
use warpui::elements::{
    Align, Border, ChildAnchor, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox,
    Container, CornerRadius, CrossAxisAlignment, DropShadow, Element, Flex, Hoverable,
    MainAxisAlignment, MainAxisSize, MouseStateHandle, OffsetPositioning, ParentAnchor,
    ParentElement, ParentOffsetBounds, Radius, ScrollbarWidth, Shrinkable, Stack, Text,
};
use warpui::fonts::{Properties, Weight};
use warpui::keymap::FixedBinding;
use warpui::platform::Cursor;
use warpui::text_layout::ClipConfig;
use warpui::ui_components::components::UiComponent;
use warpui::{AppContext, Entity, SingletonEntity, TypedActionView, View, ViewContext};

use crate::terminal::CLIAgent;
use crate::terminal::cli_agent_sessions::meters::{
    CLIAgentActivity, format_cost, format_pct, format_tokens,
};
use crate::terminal::cli_agent_sessions::meters_ui::{self, MeterTone};
use crate::workspace::PaneViewLocator;

const MODAL_WIDTH: f32 = 640.;
const MAX_LIST_HEIGHT: f32 = 420.;
const CLOSE_BUTTON_DIAMETER: f32 = 20.;
const STATUS_COLUMN_WIDTH: f32 = 130.;
const NUMBER_COLUMN_WIDTH: f32 = 70.;

pub fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    app.register_fixed_bindings([FixedBinding::new(
        "escape",
        CLIAgentOverviewAction::Close,
        id!("CLIAgentOverviewModal"),
    )]);
}

/// One CLI agent session, as listed in the overview.
#[derive(Debug, Clone, PartialEq)]
pub struct CLIAgentOverviewRow {
    pub locator: PaneViewLocator,
    pub tab_title: String,
    pub agent: CLIAgent,
    pub activity: Option<CLIAgentActivity>,
    pub model_name: Option<String>,
    pub context_used_pct: Option<f32>,
    pub context_window_size: Option<u64>,
    pub cost_usd: Option<f64>,
}

pub struct CLIAgentOverviewModal {
    rows: Vec<CLIAgentOverviewRow>,
    row_mouse_states: Vec<MouseStateHandle>,
    close_button_mouse_state: MouseStateHandle,
    list_scroll_state: ClippedScrollStateHandle,
}

impl CLIAgentOverviewModal {
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            row_mouse_states: Vec::new(),
            close_button_mouse_state: MouseStateHandle::default(),
            list_scroll_state: ClippedScrollStateHandle::default(),
        }
    }

    pub fn set_rows(&mut self, rows: Vec<CLIAgentOverviewRow>, ctx: &mut ViewContext<Self>) {
        if rows == self.rows {
            return;
        }
        self.row_mouse_states
            .resize_with(rows.len(), MouseStateHandle::default);
        self.rows = rows;
        ctx.notify();
    }

    fn render_header(&self, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let attention = self
            .rows
            .iter()
            .filter(|row| row.activity.is_some_and(CLIAgentActivity::needs_attention))
            .count();
        let total_cost: f64 = self.rows.iter().filter_map(|row| row.cost_usd).sum();
        let mut summary = format!(
            "{} session{}",
            self.rows.len(),
            if self.rows.len() == 1 { "" } else { "s" }
        );
        if attention > 0 {
            summary.push_str(&format!("  ·  {attention} need attention"));
        }
        if total_cost > 0. {
            summary.push_str(&format!("  ·  {} total", format_cost(total_cost)));
        }

        Flex::column()
            .with_child(
                Text::new_inline(
                    "Coding agent sessions",
                    appearance.ui_font_family(),
                    appearance.ui_font_size() + 4.,
                )
                .with_color(theme.main_text_color(theme.surface_1()).into_solid())
                .with_style(Properties::default().weight(Weight::Semibold))
                .finish(),
            )
            .with_child(
                Container::new(
                    Text::new_inline(
                        summary,
                        appearance.ui_font_family(),
                        appearance.ui_font_size(),
                    )
                    .with_color(theme.sub_text_color(theme.surface_1()).into_solid())
                    .finish(),
                )
                .with_margin_top(4.)
                .finish(),
            )
            .finish()
    }

    fn render_cell(
        text: String,
        width: f32,
        color: ColorU,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        ConstrainedBox::new(
            Text::new_inline(text, appearance.ui_font_family(), appearance.ui_font_size())
                .with_color(color)
                .with_clip(ClipConfig::ellipsis())
                .finish(),
        )
        .with_width(width)
        .finish()
    }

    fn render_row(
        &self,
        index: usize,
        row: &CLIAgentOverviewRow,
        threshold: u8,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let main_color = theme.main_text_color(theme.surface_1()).into_solid();
        let sub_color = theme.sub_text_color(theme.surface_1()).into_solid();

        let subtitle = [
            Some(row.agent.display_name().to_owned()),
            row.model_name.clone(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        let title_column = Flex::column()
            .with_child(
                Text::new_inline(
                    row.tab_title.clone(),
                    appearance.ui_font_family(),
                    appearance.ui_font_size(),
                )
                .with_color(main_color)
                .with_clip(ClipConfig::ellipsis())
                .finish(),
            )
            .with_child(
                Text::new_inline(
                    subtitle,
                    appearance.ui_font_family(),
                    appearance.ui_font_size() - 2.,
                )
                .with_color(sub_color)
                .with_clip(ClipConfig::ellipsis())
                .finish(),
            )
            .finish();

        let (status_text, status_color) = match row.activity {
            Some(activity) => (
                activity.label().to_owned(),
                meters_ui::activity_tone(activity).color(appearance),
            ),
            None => ("–".to_owned(), sub_color),
        };
        let (context_text, context_color) = match row.context_used_pct {
            Some(used) => {
                let window = row
                    .context_window_size
                    .map(|size| format!("/{}", format_tokens(size)))
                    .unwrap_or_default();
                (
                    format!("{}{window}", format_pct(used)),
                    meters_ui::context_tone(used, threshold).color(appearance),
                )
            }
            None => ("–".to_owned(), sub_color),
        };
        let cost_text = row
            .cost_usd
            .map(format_cost)
            .unwrap_or_else(|| "–".to_owned());

        let content = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Max)
            .with_child(Shrinkable::new(1., title_column).finish())
            .with_child(Self::render_cell(
                status_text,
                STATUS_COLUMN_WIDTH,
                status_color,
                appearance,
            ))
            .with_child(Self::render_cell(
                context_text,
                NUMBER_COLUMN_WIDTH + 20.,
                context_color,
                appearance,
            ))
            .with_child(Self::render_cell(
                cost_text,
                NUMBER_COLUMN_WIDTH,
                MeterTone::Normal.color(appearance),
                appearance,
            ))
            .finish();

        let mouse_state = self
            .row_mouse_states
            .get(index)
            .cloned()
            .expect("row_mouse_states is resized with rows");
        Hoverable::new(mouse_state, move |state| {
            let mut container = Container::new(content)
                .with_horizontal_padding(12.)
                .with_vertical_padding(6.)
                .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)));
            if state.is_hovered() {
                container = container.with_background(internal_colors::fg_overlay_2(theme));
            }
            container.finish()
        })
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(CLIAgentOverviewAction::Focus(index));
        })
        .with_cursor(Cursor::PointingHand)
        .finish()
    }
}

impl Default for CLIAgentOverviewModal {
    fn default() -> Self {
        Self::new()
    }
}

impl Entity for CLIAgentOverviewModal {
    type Event = CLIAgentOverviewEvent;
}

impl View for CLIAgentOverviewModal {
    fn ui_name() -> &'static str {
        "CLIAgentOverviewModal"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();
        let threshold = meters_ui::compaction_warning_threshold(app);

        let mut list = Flex::column();
        if self.rows.is_empty() {
            list.add_child(
                Container::new(
                    Text::new(
                        "No coding agent sessions in this window. Install the meters bridge from \
                         Settings > Third party CLI agents to see context, cost and status here.",
                        appearance.ui_font_family(),
                        appearance.ui_font_size(),
                    )
                    .with_color(theme.sub_text_color(theme.surface_1()).into_solid())
                    .finish(),
                )
                .with_vertical_padding(12.)
                .finish(),
            );
        }
        for (index, row) in self.rows.iter().enumerate() {
            list.add_child(self.render_row(index, row, threshold, appearance));
        }

        let close_button = appearance
            .ui_builder()
            .close_button(CLOSE_BUTTON_DIAMETER, self.close_button_mouse_state.clone())
            .build()
            .on_click(|ctx, _, _| ctx.dispatch_typed_action(CLIAgentOverviewAction::Close))
            .finish();

        let header = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Start)
            .with_child(Shrinkable::new(1., self.render_header(appearance)).finish())
            .with_child(close_button)
            .finish();

        let card = Container::new(
            ConstrainedBox::new(
                Flex::column()
                    .with_child(Container::new(header).with_margin_bottom(12.).finish())
                    .with_child(
                        ConstrainedBox::new(
                            ClippedScrollable::vertical(
                                self.list_scroll_state.clone(),
                                list.finish(),
                                ScrollbarWidth::Auto,
                                theme.nonactive_ui_detail().into(),
                                theme.active_ui_detail().into(),
                                warpui::elements::Fill::None,
                            )
                            .with_overlayed_scrollbar()
                            .finish(),
                        )
                        .with_max_height(MAX_LIST_HEIGHT)
                        .finish(),
                    )
                    .finish(),
            )
            .with_width(MODAL_WIDTH)
            .finish(),
        )
        .with_uniform_padding(16.)
        .with_background(theme.surface_1())
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.)))
        .with_border(Border::all(1.).with_border_fill(theme.outline()))
        .with_drop_shadow(DropShadow::default())
        .finish();

        let mut stack = Stack::new();
        stack.add_positioned_child(
            card,
            OffsetPositioning::offset_from_parent(
                vec2f(0., 0.),
                ParentOffsetBounds::WindowByPosition,
                ParentAnchor::Center,
                ChildAnchor::Center,
            ),
        );
        Container::new(Align::new(stack.finish()).finish())
            .with_background(Fill::Solid(ColorU::new(97, 97, 97, 255)).with_opacity(50))
            .finish()
    }
}

impl TypedActionView for CLIAgentOverviewModal {
    type Action = CLIAgentOverviewAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            CLIAgentOverviewAction::Close => ctx.emit(CLIAgentOverviewEvent::Close),
            CLIAgentOverviewAction::Focus(index) => {
                if let Some(row) = self.rows.get(*index) {
                    ctx.emit(CLIAgentOverviewEvent::FocusPane(row.locator));
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum CLIAgentOverviewEvent {
    Close,
    FocusPane(PaneViewLocator),
}

#[derive(Clone, Debug)]
pub enum CLIAgentOverviewAction {
    Close,
    Focus(usize),
}
