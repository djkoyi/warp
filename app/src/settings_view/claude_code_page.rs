//! The "Claude Code" settings pages: view and edit the user's Claude Code configuration
//! (skills, MCP servers, plugins, subagents, slash commands, hooks, settings files and memory
//! files) for the user scope and the project of the active terminal pane.
//!
//! Each subpage of the sidebar's "Claude Code" group ([`ClaudeSubpage`]) is its own instance of
//! [`ClaudeCodePageView`]; every instance loads its own snapshot when it is selected.
//!
//! All disk I/O runs on the background executor through [`claude_code::snapshot`] and
//! [`claude_code::edits`]; this view only holds the latest snapshot and the open form.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use pathfinder_geometry::vector::vec2f;
use serde_json::{Map, Value};
use warpui::clipboard::ClipboardContent;
use warpui::elements::{
    Border, ChildAnchor, ChildView, Clipped, ClippedScrollStateHandle, ClippedScrollable,
    ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Element, Expanded, Fill, Flex,
    MainAxisAlignment, MouseStateHandle, OffsetPositioning, ParentAnchor, ParentElement,
    ParentOffsetBounds, Radius, ScrollbarWidth, Stack, Text, Wrap,
};
use warpui::fonts::{Properties, Weight};
use warpui::ui_components::button::ButtonVariant;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::ui_components::switch::SwitchStateHandle;
use warpui::{AppContext, Entity, SingletonEntity, TypedActionView, View, ViewContext, ViewHandle};

use super::claude_code::edits::{
    ClaudeEdit, CommonSettingsUpdate, HookLocation, HookSpec, ListDiff, apply_edit,
    build_mcp_entry, mcp_form_transport, plugin_toggle_scope,
};
use super::claude_code::masking::{MASK, mask_args, mask_url, masked_kv_text, merge_masked_kv};
use super::claude_code::paths::ClaudePaths;
use super::claude_code::snapshot::{
    ClaudeSnapshot, HookItem, HookSource, McpScope, McpState, MdItem, MdKind, load_snapshot,
};
use super::claude_code::{ItemOrigin, SettingsScope};
use super::settings_page::{
    CONTENT_FONT_SIZE, HEADER_PADDING, MatchData, PageTitle, PageType, SettingsPageMeta,
    SettingsPageViewHandle, SettingsWidget, render_sub_header,
};
use super::{SettingsSection, editor_text_colors};
use crate::appearance::Appearance;
use crate::editor::{
    EditorOptions, EditorView, EnterAction, EnterSettings, PropagateAndNoOpNavigationKeys,
    SingleLineEditorOptions, TextOptions,
};
use crate::menu::{self, Menu, MenuItem, MenuItemFields};
use crate::search_bar::SearchBar;
use crate::ui_components::blended_colors;
use crate::ui_components::buttons::icon_button;
use crate::ui_components::icons::Icon;
use crate::view_components::ToastFlavor;
use crate::view_components::action_button::{ActionButton, NakedTheme};
use crate::workspace::{ActiveSession, WorkspaceAction};

const RELOAD_HINT: &str = "Recarga Claude en las pestañas para aplicar el cambio";
const RELOAD_ALL_TABS_LABEL: &str = "Recargar Claude en todas las pestañas";
const TITLE_FONT_SIZE: f32 = 13.;
const ROW_SPACING: f32 = 10.;
const SEARCH_BAR_WIDTH: f32 = 220.;
const CARD_SPACING: f32 = 8.;
const CARD_PADDING: f32 = 12.;
const CARD_INTERIOR_SPACING: f32 = 4.;
const CARD_CORNER_RADIUS: f32 = 4.;
const CHIP_FONT_SIZE: f32 = 10.;
const MENU_WIDTH: f32 = 220.;
/// Descriptions are cut to this many lines (and to `DESCRIPTION_MAX_CHARS`) on the cards.
const DESCRIPTION_LINES: f32 = 2.;
const DESCRIPTION_MAX_CHARS: usize = 220;
/// Matches warpui's default UI line height ratio, used to size the two-line description clip.
const LINE_HEIGHT_RATIO: f32 = 1.2;
const BODY_EDITOR_MIN_HEIGHT: f32 = 120.;
const BODY_EDITOR_MAX_HEIGHT: f32 = 360.;
const KEY_ORDER_NOTE: &str = "Al guardar, las claves JSON se ordenan alfabéticamente (los valores \
                              no cambian).";
const DEFAULT_MODE_PLACEHOLDER: &str =
    "default, acceptEdits, plan, dontAsk, bypassPermissions, auto";
const HOOK_EVENTS_HINT: &str = "PreToolUse, PostToolUse, UserPromptSubmit, Notification, Stop, \
                                SubagentStop, PreCompact, SessionStart, SessionEnd";

/// A subpage of the sidebar's "Claude Code" group. Each one is its own settings page (and its own
/// [`ClaudeCodePageView`] instance).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClaudeSubpage {
    Mcp,
    Skills,
    Plugins,
    Agents,
    Commands,
    Hooks,
    Settings,
}

impl ClaudeSubpage {
    /// Sidebar order. The first entry is what `SettingsSection::ClaudeCode` (Cmd+Alt+,) opens.
    pub const ALL: [ClaudeSubpage; 7] = [
        ClaudeSubpage::Mcp,
        ClaudeSubpage::Skills,
        ClaudeSubpage::Plugins,
        ClaudeSubpage::Agents,
        ClaudeSubpage::Commands,
        ClaudeSubpage::Hooks,
        ClaudeSubpage::Settings,
    ];

    pub fn section(self) -> SettingsSection {
        match self {
            ClaudeSubpage::Mcp => SettingsSection::ClaudeCode,
            ClaudeSubpage::Skills => SettingsSection::ClaudeCodeSkills,
            ClaudeSubpage::Plugins => SettingsSection::ClaudeCodePlugins,
            ClaudeSubpage::Agents => SettingsSection::ClaudeCodeAgents,
            ClaudeSubpage::Commands => SettingsSection::ClaudeCodeCommands,
            ClaudeSubpage::Hooks => SettingsSection::ClaudeCodeHooks,
            ClaudeSubpage::Settings => SettingsSection::ClaudeCodeSettings,
        }
    }

    /// The label shown in the sidebar.
    pub fn nav_label(self) -> &'static str {
        match self {
            ClaudeSubpage::Mcp => "MCP",
            ClaudeSubpage::Skills => "Skills",
            ClaudeSubpage::Plugins => "Plugins",
            ClaudeSubpage::Agents => "Subagentes",
            ClaudeSubpage::Commands => "Comandos",
            ClaudeSubpage::Hooks => "Hooks",
            ClaudeSubpage::Settings => "Ajustes",
        }
    }

    fn title(self) -> &'static str {
        match self {
            ClaudeSubpage::Mcp => "Servidores MCP",
            ClaudeSubpage::Skills => "Skills",
            ClaudeSubpage::Plugins => "Plugins",
            ClaudeSubpage::Agents => "Subagentes",
            ClaudeSubpage::Commands => "Comandos slash",
            ClaudeSubpage::Hooks => "Hooks",
            ClaudeSubpage::Settings => "Ajustes",
        }
    }

    fn search_placeholder(self) -> &'static str {
        match self {
            ClaudeSubpage::Mcp => "Buscar servidores",
            ClaudeSubpage::Skills => "Buscar skills",
            ClaudeSubpage::Plugins => "Buscar plugins",
            ClaudeSubpage::Agents => "Buscar subagentes",
            ClaudeSubpage::Commands => "Buscar comandos",
            ClaudeSubpage::Hooks => "Buscar hooks",
            ClaudeSubpage::Settings => "Buscar archivos",
        }
    }

    /// The label of the header's "+ Nuevo" button, if the subpage can create entries.
    fn new_label(self) -> Option<&'static str> {
        match self {
            ClaudeSubpage::Skills => Some("Nueva"),
            ClaudeSubpage::Mcp
            | ClaudeSubpage::Agents
            | ClaudeSubpage::Commands
            | ClaudeSubpage::Hooks => Some("Nuevo"),
            ClaudeSubpage::Plugins | ClaudeSubpage::Settings => None,
        }
    }
}

/// An entry rendered as a card, used to build its "⋯" menu.
#[derive(Clone, Debug, PartialEq)]
pub enum CardRef {
    Item(MdKind, usize),
    Mcp(usize),
    Plugin(usize),
    Marketplace(usize),
    Hook(usize),
    SettingsFile(SettingsScope),
    Memory(usize),
}

/// What the page's single dropdown menu is currently attached to.
#[derive(Clone, Debug, PartialEq)]
enum MenuAnchor {
    New,
    Card(CardRef),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClaudeCodePageAction {
    Reload,
    OpenNewMenu,
    OpenCardMenu(CardRef),
    CloseMenu,
    OpenFile(PathBuf),
    CopyPath(PathBuf),
    NewItem { kind: MdKind, project: bool },
    EditItem { kind: MdKind, index: usize },
    SetSkillEnabled { index: usize, enabled: bool },
    NewMcp(NewMcpScope),
    EditMcp(usize),
    ToggleMcp(usize),
    TogglePlugin(usize),
    NewHook(SettingsScope),
    EditHook(usize),
    EditSettings(SettingsScope),
    SetFormTransport(&'static str),
    SaveForm,
    CancelForm,
    RequestDelete(DeleteTarget),
    ConfirmDelete(DeleteTarget),
    CancelDelete,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NewMcpScope {
    User,
    Local,
    Project,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DeleteTarget {
    Item(MdKind, usize),
    Mcp(usize),
    Hook(usize),
}

pub enum ClaudeCodePageEvent {
    ShowToast {
        message: String,
        flavor: ToastFlavor,
    },
    OpenFile(PathBuf),
}

/// The page section a form is rendered in.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum FormSection {
    Items(MdKind),
    Mcp,
    Hooks,
    Settings,
}

enum FormKind {
    Markdown {
        kind: MdKind,
        project: bool,
        original_path: Option<PathBuf>,
    },
    Mcp {
        scope: McpScope,
        original_name: Option<String>,
        original: Option<Map<String, Value>>,
    },
    Hook {
        scope: SettingsScope,
        original: Option<HookLocation>,
    },
    Settings {
        scope: SettingsScope,
        initial: SettingsFormInitial,
        original_env: Option<Map<String, Value>>,
    },
}

/// The values the settings form was opened with, so saving writes only what the user changed.
struct SettingsFormInitial {
    model: String,
    default_mode: String,
    output_style: String,
    status_line: String,
    allow: Vec<String>,
    ask: Vec<String>,
    deny: Vec<String>,
}

impl FormKind {
    fn section(&self) -> FormSection {
        match self {
            FormKind::Markdown { kind, .. } => FormSection::Items(*kind),
            FormKind::Mcp { .. } => FormSection::Mcp,
            FormKind::Hook { .. } => FormSection::Hooks,
            FormKind::Settings { .. } => FormSection::Settings,
        }
    }
}

struct FieldSpec {
    key: &'static str,
    label: &'static str,
    value: String,
    multiline: bool,
    placeholder: &'static str,
    /// MCP transports this field applies to; `None` means always shown.
    transports: Option<&'static [&'static str]>,
}

impl FieldSpec {
    fn line(
        key: &'static str,
        label: &'static str,
        value: String,
        placeholder: &'static str,
    ) -> Self {
        Self {
            key,
            label,
            value,
            multiline: false,
            placeholder,
            transports: None,
        }
    }

    fn multiline(
        key: &'static str,
        label: &'static str,
        value: String,
        placeholder: &'static str,
    ) -> Self {
        Self {
            multiline: true,
            ..Self::line(key, label, value, placeholder)
        }
    }

    fn for_transports(mut self, transports: &'static [&'static str]) -> Self {
        self.transports = Some(transports);
        self
    }
}

struct FormField {
    key: &'static str,
    label: &'static str,
    multiline: bool,
    transports: Option<&'static [&'static str]>,
    editor: ViewHandle<EditorView>,
    scroll_state: ClippedScrollStateHandle,
}

struct ActiveForm {
    kind: FormKind,
    title: String,
    /// The selected MCP transport (`stdio`, `http` or `sse`); unused by other forms.
    transport: &'static str,
    fields: Vec<FormField>,
    error: Option<String>,
}

impl ActiveForm {
    fn text(&self, key: &str, app: &AppContext) -> String {
        self.fields
            .iter()
            .find(|field| field.key == key)
            .map(|field| field.editor.as_ref(app).buffer_text(app))
            .unwrap_or_default()
    }

    fn is_field_visible(&self, field: &FormField) -> bool {
        field
            .transports
            .is_none_or(|transports| transports.contains(&self.transport))
    }
}

enum Status {
    Success(String),
    Error(String),
}

pub struct ClaudeCodePageView {
    subpage: ClaudeSubpage,
    page: PageType<Self>,
    snapshot: Option<ClaudeSnapshot>,
    loading: bool,
    busy: bool,
    status: Option<Status>,
    show_reload_hint: bool,
    form: Option<ActiveForm>,
    pending_delete: Option<DeleteTarget>,
    /// Bumped by every reload and edit; a background load that finishes with an older value is
    /// discarded so it cannot overwrite a newer snapshot.
    generation: u64,
    /// Button mouse states keyed by a stable per-button id, so rows created from the snapshot
    /// keep their hover/click state across renders.
    mouse_states: RefCell<HashMap<String, MouseStateHandle>>,
    /// Card switch states, keyed like `mouse_states`.
    switch_states: RefCell<HashMap<String, SwitchStateHandle>>,
    search_editor: ViewHandle<EditorView>,
    search_bar: ViewHandle<SearchBar>,
    new_button: Option<ViewHandle<ActionButton>>,
    /// The page's dropdown, shared by the "+ Nuevo" button and every card's "⋯" button.
    menu: ViewHandle<Menu<ClaudeCodePageAction>>,
    open_menu: Option<MenuAnchor>,
}

impl ClaudeCodePageView {
    pub fn new(subpage: ClaudeSubpage, ctx: &mut ViewContext<Self>) -> Self {
        let mut widgets: Vec<Box<dyn SettingsWidget<View = Self>>> =
            vec![Box::new(OverviewWidget {
                searchable: subpage == ClaudeSubpage::Mcp,
            })];
        match subpage {
            ClaudeSubpage::Mcp => widgets.push(Box::new(McpWidget)),
            ClaudeSubpage::Skills => widgets.push(Box::new(ItemsWidget::skills())),
            ClaudeSubpage::Plugins => widgets.push(Box::new(PluginsWidget)),
            ClaudeSubpage::Agents => widgets.push(Box::new(ItemsWidget::agents())),
            ClaudeSubpage::Commands => widgets.push(Box::new(ItemsWidget::commands())),
            ClaudeSubpage::Hooks => widgets.push(Box::new(HooksWidget)),
            ClaudeSubpage::Settings => {
                widgets.push(Box::new(SettingsFilesWidget));
                widgets.push(Box::new(MemoryWidget));
            }
        }
        let title = PageTitle::new(subpage.title())
            .with_trailing_element(|view: &Self, _appearance, _app| view.render_header_controls());

        let appearance = Appearance::as_ref(ctx);
        let search_text = TextOptions::ui_text(Some(appearance.ui_font_size()), appearance);
        let search_editor = ctx.add_typed_action_view(|ctx| {
            let mut editor = EditorView::single_line(
                SingleLineEditorOptions {
                    text: search_text,
                    propagate_and_no_op_vertical_navigation_keys:
                        PropagateAndNoOpNavigationKeys::Always,
                    ..Default::default()
                },
                ctx,
            );
            editor.set_placeholder_text(subpage.search_placeholder(), ctx);
            editor
        });
        ctx.subscribe_to_view(&search_editor, |_, _, _, ctx| ctx.notify());
        let search_bar = ctx.add_typed_action_view(|_| SearchBar::new(search_editor.clone()));

        let new_button = subpage.new_label().map(|label| {
            ctx.add_typed_action_view(|_| {
                ActionButton::new(label, NakedTheme)
                    .with_icon(Icon::Plus)
                    .on_click(|ctx| ctx.dispatch_typed_action(ClaudeCodePageAction::OpenNewMenu))
            })
        });

        let menu =
            ctx.add_typed_action_view(|_| Menu::new().with_width(MENU_WIDTH).with_drop_shadow());
        ctx.subscribe_to_view(&menu, |me, _, event, ctx| {
            if let menu::Event::Close { .. } = event {
                me.open_menu = None;
                ctx.notify();
            }
        });

        Self {
            subpage,
            page: PageType::new_uncategorized(widgets, Some(title)),
            snapshot: None,
            loading: false,
            busy: false,
            status: None,
            show_reload_hint: false,
            form: None,
            pending_delete: None,
            generation: 0,
            mouse_states: Default::default(),
            switch_states: Default::default(),
            search_editor,
            search_bar,
            new_button,
            menu,
            open_menu: None,
        }
    }

    fn mouse_state(&self, key: &str) -> MouseStateHandle {
        self.mouse_states
            .borrow_mut()
            .entry(key.to_owned())
            .or_default()
            .clone()
    }

    fn reload(&mut self, ctx: &mut ViewContext<Self>) {
        // A running edit delivers a snapshot read after its write, which is fresher than
        // anything a load started now could guarantee.
        if self.loading || self.busy {
            return;
        }
        self.loading = true;
        self.generation += 1;
        let generation = self.generation;
        let cwd = ActiveSession::as_ref(ctx)
            .path_if_local(ctx.window_id())
            .map(Path::to_path_buf);
        ctx.spawn(
            async move { ClaudePaths::from_env(cwd).map(|paths| load_snapshot(&paths)) },
            move |me, snapshot, ctx| {
                me.loading = false;
                if generation != me.generation {
                    ctx.notify();
                    return;
                }
                match snapshot {
                    Some(snapshot) => me.snapshot = Some(snapshot),
                    None => {
                        me.status = Some(Status::Error(
                            "No se pudo determinar la carpeta de Claude Code (¿HOME?)".to_owned(),
                        ))
                    }
                }
                me.pending_delete = None;
                ctx.notify();
            },
        );
        ctx.notify();
    }

    fn run_edit(&mut self, edit: ClaudeEdit, ctx: &mut ViewContext<Self>) {
        let Some(paths) = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.paths.clone())
        else {
            return;
        };
        if self.busy {
            return;
        }
        self.busy = true;
        self.generation += 1;
        let generation = self.generation;
        let needs_reload = edit.needs_claude_reload();
        ctx.spawn(
            async move {
                let result = apply_edit(&paths, edit);
                (result, load_snapshot(&paths))
            },
            move |me, (result, snapshot), ctx| {
                me.busy = false;
                if generation == me.generation {
                    me.snapshot = Some(snapshot);
                }
                me.pending_delete = None;
                match result {
                    Ok(message) => {
                        me.form = None;
                        me.show_reload_hint |= needs_reload;
                        me.status = Some(Status::Success(message.clone()));
                        ctx.emit(ClaudeCodePageEvent::ShowToast {
                            message,
                            flavor: ToastFlavor::Success,
                        });
                    }
                    Err(error) => match &mut me.form {
                        Some(form) => form.error = Some(error),
                        None => me.status = Some(Status::Error(error)),
                    },
                }
                ctx.notify();
            },
        );
        ctx.notify();
    }

    fn open_form(
        &mut self,
        kind: FormKind,
        title: String,
        transport: &'static str,
        specs: Vec<FieldSpec>,
        ctx: &mut ViewContext<Self>,
    ) {
        let appearance = Appearance::as_ref(ctx);
        let font_family = appearance.ui_font_family();
        let text_colors = editor_text_colors(appearance);
        let fields: Vec<FormField> = specs
            .into_iter()
            .map(|spec| {
                let text = TextOptions {
                    font_family_override: Some(font_family),
                    text_colors_override: Some(text_colors.clone()),
                    ..Default::default()
                };
                let multiline = spec.multiline;
                let editor = ctx.add_typed_action_view(move |ctx| {
                    let mut editor = if multiline {
                        EditorView::new(
                            EditorOptions {
                                text,
                                soft_wrap: true,
                                autogrow: true,
                                single_line: false,
                                propagate_and_no_op_vertical_navigation_keys:
                                    PropagateAndNoOpNavigationKeys::Never,
                                enter_settings: EnterSettings {
                                    shift_enter: EnterAction::InsertNewLineIfMultiLine,
                                    enter: EnterAction::InsertNewLineIfMultiLine,
                                    alt_enter: EnterAction::InsertNewLineIfMultiLine,
                                    ..Default::default()
                                },
                                ..Default::default()
                            },
                            ctx,
                        )
                    } else {
                        EditorView::single_line(
                            SingleLineEditorOptions {
                                text,
                                propagate_and_no_op_vertical_navigation_keys:
                                    PropagateAndNoOpNavigationKeys::Always,
                                ..Default::default()
                            },
                            ctx,
                        )
                    };
                    editor.set_placeholder_text(spec.placeholder, ctx);
                    editor.set_buffer_text(&spec.value, ctx);
                    editor
                });
                FormField {
                    key: spec.key,
                    label: spec.label,
                    multiline: spec.multiline,
                    transports: spec.transports,
                    editor,
                    scroll_state: Default::default(),
                }
            })
            .collect();
        if let Some(first) = fields.first() {
            ctx.focus(&first.editor);
        }
        self.form = Some(ActiveForm {
            kind,
            title,
            transport,
            fields,
            error: None,
        });
        self.pending_delete = None;
        ctx.notify();
    }

    fn open_item_form(
        &mut self,
        kind: MdKind,
        project: bool,
        item: Option<&MdItem>,
        ctx: &mut ViewContext<Self>,
    ) {
        let doc = item.and_then(|item| item.doc.clone()).unwrap_or_default();
        let value = |key: &str| doc.get(key).unwrap_or_default();
        let is_new = item.is_none();
        let mut specs = Vec::new();
        match kind {
            MdKind::Skill | MdKind::Agent => specs.push(FieldSpec::line(
                "name",
                "Nombre",
                item.map(|item| item.name.clone()).unwrap_or_default(),
                "mi-nombre (letras, números, - y _)",
            )),
            MdKind::Command if is_new => specs.push(FieldSpec::line(
                "name",
                "Nombre (se invoca como /nombre)",
                String::new(),
                "revisar-pr",
            )),
            MdKind::Command => {}
        }
        specs.push(FieldSpec::line(
            "description",
            "Descripción",
            value("description"),
            "Qué hace y cuándo usarlo",
        ));
        match kind {
            MdKind::Skill => {}
            MdKind::Agent => {
                specs.push(FieldSpec::line(
                    "tools",
                    "Herramientas (vacío = todas)",
                    value("tools"),
                    "Read, Grep, Glob",
                ));
                specs.push(FieldSpec::line(
                    "model",
                    "Modelo",
                    value("model"),
                    "sonnet, opus, haiku o inherit",
                ));
            }
            MdKind::Command => {
                specs.push(FieldSpec::line(
                    "argument-hint",
                    "Pista de argumentos",
                    value("argument-hint"),
                    "[número-de-issue]",
                ));
                specs.push(FieldSpec::line(
                    "allowed-tools",
                    "Herramientas permitidas",
                    value("allowed-tools"),
                    "Bash(git:*), Read",
                ));
                specs.push(FieldSpec::line(
                    "model",
                    "Modelo",
                    value("model"),
                    "opcional",
                ));
            }
        }
        specs.push(FieldSpec::multiline(
            "body",
            "Contenido (markdown)",
            doc.body.trim_start_matches(['\r', '\n']).to_owned(),
            "Instrucciones…",
        ));
        let scope_label = if project { "proyecto" } else { "usuario" };
        let title = match item {
            Some(item) => format!("Editar {} \"{}\"", kind.label(), item.name),
            None => format!("Nuevo {} de {scope_label}", kind.label()),
        };
        self.open_form(
            FormKind::Markdown {
                kind,
                project,
                original_path: item.map(|item| item.path.clone()),
            },
            title,
            "stdio",
            specs,
            ctx,
        );
    }

    fn open_mcp_form(
        &mut self,
        scope: McpScope,
        original_name: Option<String>,
        original: Option<Map<String, Value>>,
        ctx: &mut ViewContext<Self>,
    ) {
        let entry = original.clone().unwrap_or_default();
        let str_field = |key: &str| {
            entry
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let Some(transport) = mcp_form_transport(&entry) else {
            self.status = Some(Status::Error(
                "Este tipo de servidor MCP no se puede editar aquí; usa \"Abrir archivo\""
                    .to_owned(),
            ));
            ctx.notify();
            return;
        };
        let args: Vec<String> = entry
            .get("args")
            .and_then(Value::as_array)
            .map(|args| {
                args.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let args = mask_args(&args).join("\n");
        let specs = vec![
            FieldSpec::line(
                "name",
                "Nombre",
                original_name.clone().unwrap_or_default(),
                "mi-servidor",
            ),
            FieldSpec::line("command", "Comando", str_field("command"), "npx")
                .for_transports(&["stdio"]),
            FieldSpec::multiline(
                "args",
                "Argumentos (uno por línea; •••••• conserva el valor guardado)",
                args,
                "-y\n@paquete/servidor",
            )
            .for_transports(&["stdio"]),
            FieldSpec::multiline(
                "env",
                "Variables de entorno (CLAVE=VALOR por línea; •••••• conserva el valor guardado)",
                masked_kv_text(entry.get("env").and_then(Value::as_object)),
                "API_KEY=…",
            )
            .for_transports(&["stdio"]),
            FieldSpec::line(
                "url",
                "URL (•••••• conserva las credenciales guardadas)",
                mask_url(&str_field("url")),
                "https://…",
            )
            .for_transports(&["http", "sse"]),
            FieldSpec::multiline(
                "headers",
                "Cabeceras (CLAVE=VALOR por línea; •••••• conserva el valor guardado)",
                masked_kv_text(entry.get("headers").and_then(Value::as_object)),
                "Authorization=Bearer …",
            )
            .for_transports(&["http", "sse"]),
        ];
        let title = match &original_name {
            Some(name) => format!("Editar servidor MCP \"{name}\" ({})", scope.label()),
            None => format!("Nuevo servidor MCP ({})", scope.label()),
        };
        self.open_form(
            FormKind::Mcp {
                scope,
                original_name,
                original,
            },
            title,
            transport,
            specs,
            ctx,
        );
    }

    fn open_settings_form(&mut self, scope: SettingsScope, ctx: &mut ViewContext<Self>) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        if let Some(error) = snapshot
            .settings_file(scope)
            .and_then(|file| file.error.clone())
        {
            self.status = Some(Status::Error(format!(
                "Corrige el archivo antes de editarlo aquí: {error}"
            )));
            ctx.notify();
            return;
        }
        let settings = snapshot.settings_value(scope).cloned().unwrap_or_default();
        let str_at =
            |value: Option<&Value>| value.and_then(Value::as_str).unwrap_or_default().to_owned();
        let permissions = settings.get("permissions");
        let rules = |kind: &str| -> Vec<String> {
            permissions
                .and_then(|permissions| permissions.get(kind))
                .and_then(Value::as_array)
                .map(|rules| {
                    rules
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|rule| rule.trim().to_owned())
                        .filter(|rule| !rule.is_empty())
                        .collect()
                })
                .unwrap_or_default()
        };
        let initial = SettingsFormInitial {
            model: str_at(settings.get("model")),
            default_mode: str_at(permissions.and_then(|p| p.get("defaultMode"))),
            output_style: str_at(settings.get("outputStyle")),
            status_line: str_at(settings.get("statusLine").and_then(|s| s.get("command"))),
            allow: rules("allow"),
            ask: rules("ask"),
            deny: rules("deny"),
        };
        let default_mode_label = match scope {
            SettingsScope::Project | SettingsScope::Local => {
                "Modo de permisos (permissions.defaultMode; bypassPermissions y auto no tienen \
                 efecto desde archivos de proyecto o local)"
            }
            SettingsScope::User | SettingsScope::Managed => {
                "Modo de permisos (permissions.defaultMode)"
            }
        };
        let original_env = settings.get("env").and_then(Value::as_object).cloned();
        let specs = vec![
            FieldSpec::line(
                "model",
                "Modelo (model)",
                initial.model.clone(),
                "opus, sonnet…",
            ),
            FieldSpec::line(
                "defaultMode",
                default_mode_label,
                initial.default_mode.clone(),
                DEFAULT_MODE_PLACEHOLDER,
            ),
            FieldSpec::line(
                "outputStyle",
                "Estilo de salida (outputStyle)",
                initial.output_style.clone(),
                "default, Explanatory, Learning…",
            ),
            FieldSpec::line(
                "statusLine",
                "Comando de statusLine",
                initial.status_line.clone(),
                "~/.claude/statusline.sh",
            ),
            FieldSpec::multiline(
                "allow",
                "Permitir (una regla por línea)",
                initial.allow.join("\n"),
                "Bash(git status:*)",
            ),
            FieldSpec::multiline(
                "ask",
                "Preguntar (una regla por línea)",
                initial.ask.join("\n"),
                "Bash(git push:*)",
            ),
            FieldSpec::multiline(
                "deny",
                "Denegar (una regla por línea)",
                initial.deny.join("\n"),
                "Read(./.env)",
            ),
            FieldSpec::multiline(
                "env",
                "Variables de entorno (CLAVE=VALOR por línea; •••••• conserva el valor guardado)",
                masked_kv_text(original_env.as_ref()),
                "CLAVE=valor",
            ),
        ];
        self.open_form(
            FormKind::Settings {
                scope,
                initial,
                original_env,
            },
            format!("Editar configuración: {}", scope.label()),
            "stdio",
            specs,
            ctx,
        );
    }

    fn save_form(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(form) = &self.form else {
            return;
        };
        match self.build_edit(form, ctx) {
            Ok(edit) => self.run_edit(edit, ctx),
            Err(error) => {
                if let Some(form) = &mut self.form {
                    form.error = Some(error);
                }
                ctx.notify();
            }
        }
    }

    fn build_edit(&self, form: &ActiveForm, app: &AppContext) -> Result<ClaudeEdit, String> {
        let text = |key: &str| form.text(key, app);
        let lines = |key: &str| {
            text(key)
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        match &form.kind {
            FormKind::Markdown {
                kind,
                project,
                original_path,
            } => {
                let field_keys: &[&str] = match kind {
                    MdKind::Skill => &["description"],
                    MdKind::Agent => &["description", "tools", "model"],
                    MdKind::Command => &["description", "argument-hint", "allowed-tools", "model"],
                };
                Ok(ClaudeEdit::SaveMarkdown {
                    kind: *kind,
                    project: *project,
                    original_path: original_path.clone(),
                    name: text("name").trim().to_owned(),
                    fields: field_keys
                        .iter()
                        .map(|key| ((*key).to_owned(), text(*key)))
                        .collect(),
                    body: text("body"),
                })
            }
            FormKind::Mcp {
                scope,
                original_name,
                original,
            } => {
                let original_env = original
                    .as_ref()
                    .and_then(|entry| entry.get("env"))
                    .and_then(Value::as_object);
                let original_headers = original
                    .as_ref()
                    .and_then(|entry| entry.get("headers"))
                    .and_then(Value::as_object);
                let (env, headers) = match form.transport {
                    "stdio" => (merge_masked_kv(original_env, &text("env"))?, Map::new()),
                    _ => (
                        Map::new(),
                        merge_masked_kv(original_headers, &text("headers"))?,
                    ),
                };
                let entry = build_mcp_entry(
                    original.as_ref(),
                    form.transport,
                    &text("command"),
                    &text("args"),
                    &text("url"),
                    env,
                    headers,
                )?;
                Ok(ClaudeEdit::UpsertMcpServer {
                    scope: scope.clone(),
                    original_name: original_name.clone(),
                    name: text("name").trim().to_owned(),
                    entry,
                })
            }
            FormKind::Hook { scope, original } => {
                let timeout = text("timeout");
                let timeout = timeout.trim();
                let timeout = if timeout.is_empty() {
                    None
                } else {
                    Some(
                        timeout
                            .parse::<u64>()
                            .map_err(|_| "El timeout debe ser un número de segundos".to_owned())?,
                    )
                };
                let matcher = text("matcher");
                Ok(ClaudeEdit::UpsertHook {
                    scope: *scope,
                    original: original.clone(),
                    hook: HookSpec {
                        event: text("event").trim().to_owned(),
                        matcher: Some(matcher.trim().to_owned()).filter(|m| !m.is_empty()),
                        command: text("command").trim().to_owned(),
                        timeout,
                    },
                })
            }
            FormKind::Settings {
                scope,
                initial,
                original_env,
            } => {
                let changed = |key: &str, before: &str| {
                    let value = text(key).trim().to_owned();
                    (value != before.trim()).then_some(value)
                };
                let edited_env = merge_masked_kv(original_env.as_ref(), &text("env"))?;
                let env_set = edited_env
                    .iter()
                    .filter(|(key, value)| {
                        original_env.as_ref().and_then(|env| env.get(*key)) != Some(*value)
                    })
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect();
                let env_remove = original_env
                    .iter()
                    .flat_map(|env| env.keys())
                    .filter(|key| !edited_env.contains_key(*key))
                    .cloned()
                    .collect();
                Ok(ClaudeEdit::SetCommonSettings {
                    scope: *scope,
                    update: CommonSettingsUpdate {
                        model: changed("model", &initial.model),
                        default_mode: changed("defaultMode", &initial.default_mode),
                        output_style: changed("outputStyle", &initial.output_style),
                        status_line_command: changed("statusLine", &initial.status_line),
                        allow: ListDiff::between(&initial.allow, &lines("allow")),
                        deny: ListDiff::between(&initial.deny, &lines("deny")),
                        ask: ListDiff::between(&initial.ask, &lines("ask")),
                        env_set,
                        env_remove,
                    },
                })
            }
        }
    }

    fn delete_edit(&self, target: DeleteTarget) -> Option<ClaudeEdit> {
        let snapshot = self.snapshot.as_ref()?;
        match target {
            DeleteTarget::Item(kind, index) => {
                let item = snapshot.items(kind).get(index)?;
                (!item.origin.is_read_only()).then(|| ClaudeEdit::DeleteItem {
                    path: item.removal_path(),
                })
            }
            DeleteTarget::Mcp(index) => {
                let server = snapshot.mcp_servers.get(index)?;
                (!server.scope.is_read_only()).then(|| ClaudeEdit::DeleteMcpServer {
                    scope: server.scope.clone(),
                    name: server.name.clone(),
                })
            }
            DeleteTarget::Hook(index) => {
                let hook = snapshot.hooks.get(index)?;
                let HookSource::Settings(scope) = hook.source else {
                    return None;
                };
                scope.is_writable().then(|| ClaudeEdit::RemoveHook {
                    scope,
                    location: hook_location(hook),
                })
            }
        }
    }

    // ── Rendering helpers ──────────────────────────────────────────────

    fn button(
        &self,
        appearance: &Appearance,
        key: String,
        label: &str,
        action: ClaudeCodePageAction,
        variant: ButtonVariant,
    ) -> Box<dyn Element> {
        let mut button = appearance
            .ui_builder()
            .button(variant, self.mouse_state(&key))
            .with_text_label(label.to_owned())
            .with_style(UiComponentStyles {
                font_size: Some(CONTENT_FONT_SIZE),
                padding: Some(Coords::uniform(4.).left(8.).right(8.)),
                ..Default::default()
            });
        if self.busy {
            button = button.disabled();
        }
        button
            .build()
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action(action.clone()))
            .finish()
    }

    fn delete_buttons(
        &self,
        appearance: &Appearance,
        key: &str,
        target: DeleteTarget,
    ) -> Vec<Box<dyn Element>> {
        if self.pending_delete == Some(target) {
            vec![
                self.button(
                    appearance,
                    format!("{key}:confirm"),
                    "Confirmar eliminación",
                    ClaudeCodePageAction::ConfirmDelete(target),
                    ButtonVariant::Error,
                ),
                self.button(
                    appearance,
                    format!("{key}:cancel"),
                    "Cancelar",
                    ClaudeCodePageAction::CancelDelete,
                    ButtonVariant::Text,
                ),
            ]
        } else {
            vec![self.button(
                appearance,
                format!("{key}:delete"),
                "Eliminar",
                ClaudeCodePageAction::RequestDelete(target),
                ButtonVariant::Text,
            )]
        }
    }

    fn render_form_if(
        &self,
        section: FormSection,
        appearance: &Appearance,
    ) -> Option<Box<dyn Element>> {
        let form = self.form.as_ref()?;
        (form.kind.section() == section).then(|| self.render_form(form, appearance))
    }

    fn render_form(&self, form: &ActiveForm, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let mut column = Flex::column().with_child(
            Container::new(
                Text::new(
                    form.title.clone(),
                    appearance.ui_font_family(),
                    TITLE_FONT_SIZE,
                )
                .with_color(theme.active_ui_text_color().into_solid())
                .with_style(Properties::default().weight(Weight::Bold))
                .finish(),
            )
            .with_margin_bottom(8.)
            .finish(),
        );

        if let FormKind::Mcp { .. } = form.kind {
            let mut transports = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
            transports.add_child(small_text(appearance, "Tipo:", sub_color(appearance)));
            for transport in ["stdio", "http", "sse"] {
                let variant = if form.transport == transport {
                    ButtonVariant::Accent
                } else {
                    ButtonVariant::Secondary
                };
                transports.add_child(
                    Container::new(self.button(
                        appearance,
                        format!("form:transport:{transport}"),
                        transport,
                        ClaudeCodePageAction::SetFormTransport(transport),
                        variant,
                    ))
                    .with_margin_left(6.)
                    .finish(),
                );
            }
            column.add_child(
                Container::new(transports.finish())
                    .with_margin_bottom(8.)
                    .finish(),
            );
        }

        for field in form
            .fields
            .iter()
            .filter(|field| form.is_field_visible(field))
        {
            column.add_child(
                Container::new(small_text(appearance, field.label, sub_color(appearance)))
                    .with_margin_bottom(4.)
                    .finish(),
            );
            let editor = ChildView::new(&field.editor).finish();
            let editor = if field.multiline {
                ConstrainedBox::new(
                    ClippedScrollable::vertical(
                        field.scroll_state.clone(),
                        ConstrainedBox::new(editor)
                            .with_min_height(BODY_EDITOR_MIN_HEIGHT)
                            .finish(),
                        ScrollbarWidth::Auto,
                        theme.nonactive_ui_detail().into(),
                        theme.active_ui_detail().into(),
                        Fill::None,
                    )
                    .finish(),
                )
                .with_max_height(BODY_EDITOR_MAX_HEIGHT)
                .finish()
            } else {
                Clipped::new(editor).finish()
            };
            column.add_child(
                Container::new(editor)
                    .with_background(theme.surface_2())
                    .with_border(Border::all(1.).with_border_fill(theme.outline()))
                    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
                    .with_horizontal_padding(8.)
                    .with_vertical_padding(6.)
                    .with_margin_bottom(8.)
                    .finish(),
            );
        }
        if let FormKind::Hook { .. } = form.kind {
            column.add_child(
                Container::new(small_text(
                    appearance,
                    &format!("Eventos: {HOOK_EVENTS_HINT}"),
                    sub_color(appearance),
                ))
                .with_margin_bottom(8.)
                .finish(),
            );
        }
        // MCP, hook and settings forms rewrite JSON files.
        if !matches!(form.kind, FormKind::Markdown { .. }) {
            column.add_child(
                Container::new(small_text(
                    appearance,
                    KEY_ORDER_NOTE,
                    sub_color(appearance),
                ))
                .with_margin_bottom(8.)
                .finish(),
            );
        }
        if let Some(error) = &form.error {
            column.add_child(
                Container::new(small_text(appearance, error, theme.ui_error_color()))
                    .with_margin_bottom(8.)
                    .finish(),
            );
        }
        let mut buttons = Wrap::row().with_spacing(6.).with_run_spacing(6.);
        buttons.add_child(self.button(
            appearance,
            "form:save".to_owned(),
            if self.busy { "Guardando…" } else { "Guardar" },
            ClaudeCodePageAction::SaveForm,
            ButtonVariant::Accent,
        ));
        buttons.add_child(self.button(
            appearance,
            "form:cancel".to_owned(),
            "Cancelar",
            ClaudeCodePageAction::CancelForm,
            ButtonVariant::Secondary,
        ));
        column.add_child(buttons.finish());

        Container::new(column.finish())
            .with_background(theme.surface_1())
            .with_border(Border::all(1.).with_border_fill(theme.accent()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
            .with_uniform_padding(12.)
            .with_margin_bottom(HEADER_PADDING)
            .finish()
    }

    fn switch_state(&self, key: &str) -> SwitchStateHandle {
        self.switch_states
            .borrow_mut()
            .entry(key.to_owned())
            .or_default()
            .clone()
    }

    /// The page's search query, lowercased; empty when the search box is empty.
    fn search_query(&self, app: &AppContext) -> String {
        self.search_editor
            .as_ref(app)
            .buffer_text(app)
            .trim()
            .to_lowercase()
    }

    /// The options of the header's "+ Nuevo" button: one entry per scope that can be written.
    fn new_options(&self) -> Vec<MenuEntry> {
        let Some(snapshot) = &self.snapshot else {
            return Vec::new();
        };
        let has_project = snapshot.paths.project.is_some();
        match self.subpage {
            ClaudeSubpage::Skills | ClaudeSubpage::Agents | ClaudeSubpage::Commands => {
                let Some(kind) = self.subpage.kind() else {
                    return Vec::new();
                };
                let mut options = vec![MenuEntry::new(
                    "Usuario",
                    ClaudeCodePageAction::NewItem {
                        kind,
                        project: false,
                    },
                )];
                if has_project {
                    options.push(MenuEntry::new(
                        "Proyecto",
                        ClaudeCodePageAction::NewItem {
                            kind,
                            project: true,
                        },
                    ));
                }
                options
            }
            ClaudeSubpage::Mcp => {
                let mut options = vec![MenuEntry::new(
                    "Usuario",
                    ClaudeCodePageAction::NewMcp(NewMcpScope::User),
                )];
                if has_project {
                    options.push(MenuEntry::new(
                        "Local (solo este proyecto)",
                        ClaudeCodePageAction::NewMcp(NewMcpScope::Local),
                    ));
                    options.push(MenuEntry::new(
                        "Proyecto (.mcp.json)",
                        ClaudeCodePageAction::NewMcp(NewMcpScope::Project),
                    ));
                }
                options
            }
            ClaudeSubpage::Hooks => [
                SettingsScope::User,
                SettingsScope::Project,
                SettingsScope::Local,
            ]
            .into_iter()
            .filter(|scope| snapshot.paths.settings_path(*scope).is_some())
            .map(|scope| MenuEntry::new(scope.label(), ClaudeCodePageAction::NewHook(scope)))
            .collect(),
            ClaudeSubpage::Plugins | ClaudeSubpage::Settings => Vec::new(),
        }
    }

    /// The "⋯" menu of a card, in the order Editar, Abrir archivo, Copiar ruta, Eliminar. Only
    /// actions the entry supports are listed; read-only entries (plugins, managed files) get no
    /// edit or delete.
    fn card_menu_entries(&self, card: &CardRef) -> Vec<MenuEntry> {
        let Some(snapshot) = &self.snapshot else {
            return Vec::new();
        };
        let mut entries = Vec::new();
        match card {
            CardRef::Item(kind, index) => {
                let (kind, index) = (*kind, *index);
                let Some(item) = snapshot.items(kind).get(index) else {
                    return entries;
                };
                let writable = !item.origin.is_read_only();
                if writable && item.doc.is_some() {
                    entries.push(MenuEntry::edit(ClaudeCodePageAction::EditItem {
                        kind,
                        index,
                    }));
                }
                // A partial override (`name-only`, `user-invocable-only`…) shows the switch as on;
                // removing it is a separate, explicit action so the switch never drops it silently.
                if skill_toggle_allowed(item, snapshot)
                    && let Some(state) = item
                        .override_state
                        .as_deref()
                        .filter(|state| *state != "off")
                {
                    entries.push(
                        MenuEntry::new(
                            format!("Activar (quita \"{state}\")"),
                            ClaudeCodePageAction::SetSkillEnabled {
                                index,
                                enabled: true,
                            },
                        )
                        .with_icon(Icon::Check),
                    );
                }
                entries.extend(file_entries(&item.path, true));
                if writable {
                    entries.push(MenuEntry::delete(DeleteTarget::Item(kind, index)));
                }
            }
            CardRef::Mcp(index) => {
                let Some(server) = snapshot.mcp_servers.get(*index) else {
                    return entries;
                };
                let writable = !server.scope.is_read_only();
                if writable && mcp_form_transport(&server.raw).is_some() {
                    entries.push(MenuEntry::edit(ClaudeCodePageAction::EditMcp(*index)));
                }
                entries.extend(file_entries(&server.path, true));
                if writable {
                    entries.push(MenuEntry::delete(DeleteTarget::Mcp(*index)));
                }
            }
            CardRef::Plugin(index) => {
                if let Some(path) = snapshot
                    .plugins
                    .get(*index)
                    .and_then(|plugin| plugin.install_path.as_ref())
                {
                    entries.extend(file_entries(path, false));
                }
            }
            CardRef::Marketplace(index) => {
                if let Some(path) = snapshot
                    .marketplaces
                    .get(*index)
                    .and_then(|marketplace| marketplace.install_location.as_ref())
                {
                    entries.extend(file_entries(path, false));
                }
            }
            CardRef::Hook(index) => {
                let Some(hook) = snapshot.hooks.get(*index) else {
                    return entries;
                };
                let editable = hook_is_editable(hook);
                if editable {
                    entries.push(MenuEntry::edit(ClaudeCodePageAction::EditHook(*index)));
                }
                entries.extend(file_entries(&hook.path, true));
                if editable {
                    entries.push(MenuEntry::delete(DeleteTarget::Hook(*index)));
                }
            }
            CardRef::SettingsFile(scope) => {
                let Some(file) = snapshot.settings.iter().find(|file| file.scope == *scope) else {
                    return entries;
                };
                if file.scope.is_writable() && file.error.is_none() {
                    entries.push(
                        MenuEntry::new(
                            "Editar valores comunes",
                            ClaudeCodePageAction::EditSettings(file.scope),
                        )
                        .with_icon(Icon::Pencil),
                    );
                }
                entries.extend(file_entries(&file.path, file.exists));
            }
            CardRef::Memory(index) => {
                if let Some(file) = snapshot.memory_files.get(*index) {
                    entries.extend(file_entries(&file.path, file.exists));
                }
            }
        }
        entries
    }

    fn show_menu(
        &mut self,
        anchor: MenuAnchor,
        entries: Vec<MenuEntry>,
        ctx: &mut ViewContext<Self>,
    ) {
        let error_color = Appearance::as_ref(ctx).theme().ui_error_color();
        let items: Vec<MenuItem<ClaudeCodePageAction>> = entries
            .into_iter()
            .map(|entry| {
                let mut fields =
                    MenuItemFields::new(entry.label).with_on_select_action(entry.action);
                if let Some(icon) = entry.icon {
                    fields = fields.with_icon(icon);
                }
                if entry.danger {
                    fields = fields.with_override_text_color(error_color);
                }
                MenuItem::Item(fields)
            })
            .collect();
        self.menu
            .update(ctx, |menu, ctx| menu.set_items(items, ctx));
        self.open_menu = Some(anchor);
        ctx.notify();
    }

    fn close_menu(&mut self, ctx: &mut ViewContext<Self>) {
        if self.open_menu.take().is_some() {
            ctx.notify();
        }
    }

    /// Wraps `anchor_element` so the page's menu drops down from it when `anchor` is open.
    fn with_menu_overlay(
        &self,
        anchor_element: Box<dyn Element>,
        anchor: &MenuAnchor,
    ) -> Box<dyn Element> {
        let mut stack = Stack::new();
        stack.add_child(anchor_element);
        if self.open_menu.as_ref() == Some(anchor) {
            stack.add_positioned_overlay_child(
                ChildView::new(&self.menu).finish(),
                OffsetPositioning::offset_from_parent(
                    vec2f(0., 4.),
                    ParentOffsetBounds::WindowByPosition,
                    ParentAnchor::BottomRight,
                    ChildAnchor::TopRight,
                ),
            );
        }
        stack.finish()
    }

    /// The title's trailing controls: the search box and, where the subpage creates entries, the
    /// "+ Nuevo" button.
    fn render_header_controls(&self) -> Box<dyn Element> {
        let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
        row.add_child(
            ConstrainedBox::new(ChildView::new(&self.search_bar).finish())
                .with_width(SEARCH_BAR_WIDTH)
                .finish(),
        );
        if let Some(button) = &self.new_button {
            row.add_child(
                Container::new(
                    self.with_menu_overlay(ChildView::new(button).finish(), &MenuAnchor::New),
                )
                .with_margin_left(8.)
                .finish(),
            );
        }
        row.finish()
    }

    /// The open form (when it belongs here), then the cards, or the loading / empty / no-match
    /// text.
    fn render_list(
        &self,
        appearance: &Appearance,
        form_section: Option<FormSection>,
        cards: Vec<Box<dyn Element>>,
        total: usize,
        empty_text: &str,
    ) -> Box<dyn Element> {
        let mut column = Flex::column();
        if let Some(form) =
            form_section.and_then(|section| self.render_form_if(section, appearance))
        {
            column.add_child(form);
        }
        if self.snapshot.is_none() {
            column.add_child(small_text(appearance, "Cargando…", sub_color(appearance)));
        } else if total == 0 {
            column.add_child(small_text(appearance, empty_text, sub_color(appearance)));
        } else if cards.is_empty() {
            column.add_child(small_text(
                appearance,
                "Nada coincide con la búsqueda.",
                sub_color(appearance),
            ));
        } else {
            for card in cards {
                column.add_child(card);
            }
        }
        Container::new(column.finish())
            .with_margin_bottom(HEADER_PADDING)
            .finish()
    }

    fn render_card(&self, card: Card, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let mut title_row = Wrap::row()
            .with_spacing(CARD_INTERIOR_SPACING)
            .with_run_spacing(CARD_INTERIOR_SPACING)
            .with_cross_axis_alignment(CrossAxisAlignment::Center);
        title_row.add_child(
            Text::new(
                card.title,
                appearance.ui_font_family(),
                appearance.ui_builder().ui_font_size(),
            )
            .with_color(blended_colors::text_main(theme, theme.surface_1()))
            .with_style(Properties::default().weight(Weight::Bold))
            .finish(),
        );
        for chip in &card.chips {
            title_row.add_child(render_chip(appearance, chip));
        }

        let mut info = Flex::column()
            .with_spacing(CARD_INTERIOR_SPACING)
            .with_child(title_row.finish());
        if let Some(description) = card
            .description
            .filter(|description| !description.trim().is_empty())
        {
            info.add_child(if card.full_text {
                small_text(appearance, &description, sub_color(appearance))
            } else {
                render_clamped_text(appearance, &description)
            });
        }
        if let Some(detail) = card.detail {
            info.add_child(small_text(appearance, &detail, sub_color(appearance)));
        }
        if let Some(error) = card.error {
            info.add_child(small_text(
                appearance,
                &format!("Error: {error}"),
                theme.ui_error_color(),
            ));
        }

        let mut actions = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_alignment(MainAxisAlignment::End)
            .with_spacing(8.);
        if let Some((checked, action)) = card.toggle {
            let mut switch = appearance
                .ui_builder()
                .switch(self.switch_state(&card.key))
                .check(checked);
            if self.busy {
                switch = switch.disable();
            }
            actions.add_child(
                switch
                    .build()
                    .on_click(move |ctx, _, _| ctx.dispatch_typed_action(action.clone()))
                    .finish(),
            );
        }
        if let Some(card_ref) = card
            .menu
            .filter(|card_ref| !self.card_menu_entries(card_ref).is_empty())
        {
            let anchor = MenuAnchor::Card(card_ref.clone());
            let is_open = self.open_menu.as_ref() == Some(&anchor);
            let button = icon_button(
                appearance,
                Icon::DotsHorizontal,
                is_open,
                self.mouse_state(&format!("{}:menu", card.key)),
            )
            .build()
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(if is_open {
                    ClaudeCodePageAction::CloseMenu
                } else {
                    ClaudeCodePageAction::OpenCardMenu(card_ref.clone())
                })
            })
            .finish();
            actions.add_child(self.with_menu_overlay(button, &anchor));
        }

        let mut body = Flex::column()
            .with_spacing(CARD_INTERIOR_SPACING)
            .with_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_spacing(CARD_INTERIOR_SPACING)
                    .with_child(Expanded::new(1., info.finish()).finish())
                    .with_child(actions.finish())
                    .finish(),
            );
        if let Some(target) = card
            .delete_target
            .filter(|target| self.pending_delete == Some(*target))
        {
            let mut confirm = Wrap::row()
                .with_spacing(6.)
                .with_run_spacing(6.)
                .with_cross_axis_alignment(CrossAxisAlignment::Center);
            confirm.add_child(small_text(
                appearance,
                "¿Eliminar? Antes se guarda una copia de respaldo.",
                theme.ui_warning_color(),
            ));
            for button in self.delete_buttons(appearance, &card.key, target) {
                confirm.add_child(button);
            }
            body.add_child(
                Container::new(confirm.finish())
                    .with_margin_top(4.)
                    .finish(),
            );
        }

        Container::new(body.finish())
            .with_uniform_padding(CARD_PADDING)
            .with_background(theme.surface_1())
            .with_border(Border::all(1.).with_border_fill(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CARD_CORNER_RADIUS)))
            .with_margin_bottom(CARD_SPACING)
            .finish()
    }

    /// Cards for the skills, subagents or slash commands that match `query`, plus the total.
    fn item_cards(
        &self,
        kind: MdKind,
        query: &str,
        appearance: &Appearance,
    ) -> (Vec<Box<dyn Element>>, usize) {
        let Some(snapshot) = &self.snapshot else {
            return (Vec::new(), 0);
        };
        let items = snapshot.items(kind);
        let cards = items
            .iter()
            .enumerate()
            .filter(|(_, item)| matches_query(query, &item.name, item.description().as_deref()))
            .map(|(index, item)| {
                let mut card = Card::new(format!("{}:{index}", kind.subdir()), item.name.clone());
                card.menu = Some(CardRef::Item(kind, index));
                card.chips.push(item.origin.label());
                if let Some(state) = &item.override_state {
                    card.chips.push(format!("skillOverrides: {state}"));
                }
                card.description = item.description();
                card.error = item.error.clone();
                if skill_toggle_allowed(item, snapshot) {
                    let enabled = item.override_state.as_deref() != Some("off");
                    card.toggle = Some((
                        enabled,
                        ClaudeCodePageAction::SetSkillEnabled {
                            index,
                            enabled: !enabled,
                        },
                    ));
                }
                if !item.origin.is_read_only() {
                    card.delete_target = Some(DeleteTarget::Item(kind, index));
                }
                self.render_card(card, appearance)
            })
            .collect();
        (cards, items.len())
    }
}

/// One entry of the page's dropdown menu.
struct MenuEntry {
    label: String,
    action: ClaudeCodePageAction,
    icon: Option<Icon>,
    danger: bool,
}

impl MenuEntry {
    fn new(label: impl Into<String>, action: ClaudeCodePageAction) -> Self {
        Self {
            label: label.into(),
            action,
            icon: None,
            danger: false,
        }
    }

    fn with_icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    fn edit(action: ClaudeCodePageAction) -> Self {
        Self::new("Editar", action).with_icon(Icon::Pencil)
    }

    fn delete(target: DeleteTarget) -> Self {
        Self {
            danger: true,
            ..Self::new("Eliminar", ClaudeCodePageAction::RequestDelete(target))
                .with_icon(Icon::Trash)
        }
    }
}

/// "Abrir archivo" (when `can_open`) and "Copiar ruta". Existence comes from the snapshot so
/// rendering never touches the disk.
fn file_entries(path: &Path, can_open: bool) -> Vec<MenuEntry> {
    let mut entries = Vec::new();
    if can_open {
        entries.push(
            MenuEntry::new(
                "Abrir archivo",
                ClaudeCodePageAction::OpenFile(path.to_path_buf()),
            )
            .with_icon(Icon::File),
        );
    }
    entries.push(
        MenuEntry::new(
            "Copiar ruta",
            ClaudeCodePageAction::CopyPath(path.to_path_buf()),
        )
        .with_icon(Icon::Copy),
    );
    entries
}

/// Everything a card shows. Cards without a `menu` (informational ones) have no "⋯" button.
struct Card {
    /// Stable id for the card's mouse and switch states.
    key: String,
    title: String,
    chips: Vec<String>,
    description: Option<String>,
    /// An extra, unclamped line under the description.
    detail: Option<String>,
    error: Option<String>,
    /// The switch state and the action that flips it.
    toggle: Option<(bool, ClaudeCodePageAction)>,
    menu: Option<CardRef>,
    /// Shows the inline delete confirmation while this target is pending.
    delete_target: Option<DeleteTarget>,
    /// Shows the whole description instead of cutting it to two lines.
    full_text: bool,
}

impl Card {
    fn new(key: String, title: String) -> Self {
        Self {
            key,
            title,
            chips: Vec::new(),
            description: None,
            detail: None,
            error: None,
            toggle: None,
            menu: None,
            delete_target: None,
            full_text: false,
        }
    }
}

impl ClaudeSubpage {
    /// The markdown kind listed by the skills, subagents and commands subpages.
    fn kind(self) -> Option<MdKind> {
        match self {
            ClaudeSubpage::Skills => Some(MdKind::Skill),
            ClaudeSubpage::Agents => Some(MdKind::Agent),
            ClaudeSubpage::Commands => Some(MdKind::Command),
            ClaudeSubpage::Mcp
            | ClaudeSubpage::Plugins
            | ClaudeSubpage::Hooks
            | ClaudeSubpage::Settings => None,
        }
    }
}

/// Whether every word of `query` (already lowercased) appears in `name` or `description`.
fn matches_query(query: &str, name: &str, description: Option<&str>) -> bool {
    if query.is_empty() {
        return true;
    }
    let haystack = format!("{name} {}", description.unwrap_or_default()).to_lowercase();
    query.split_whitespace().all(|word| haystack.contains(word))
}

/// Skills can be switched on and off through `skillOverrides` (user skills always, project skills
/// when a project is open); plugin skills and other kinds cannot.
fn skill_toggle_allowed(item: &MdItem, snapshot: &ClaudeSnapshot) -> bool {
    item.kind == MdKind::Skill
        && !item.origin.is_read_only()
        && (item.origin == ItemOrigin::User || snapshot.paths.project.is_some())
}

/// Only `command` hooks in a writable settings file can be edited or deleted here.
fn hook_is_editable(hook: &HookItem) -> bool {
    matches!(hook.source, HookSource::Settings(scope) if scope.is_writable())
        && hook.hook_type == "command"
}

fn render_chip(appearance: &Appearance, text: &str) -> Box<dyn Element> {
    let theme = appearance.theme();
    Container::new(
        Text::new(text.to_owned(), appearance.ui_font_family(), CHIP_FONT_SIZE)
            .with_color(theme.sub_text_color(theme.surface_3()).into_solid())
            .finish(),
    )
    .with_background(theme.surface_3())
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.)))
    .with_horizontal_padding(4.)
    .with_vertical_padding(1.)
    .finish()
}

/// `text` on at most [`DESCRIPTION_LINES`] lines: whitespace is collapsed, long text is cut with an
/// ellipsis, and the element is clipped to the line height.
fn render_clamped_text(appearance: &Appearance, text: &str) -> Box<dyn Element> {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let shown = if collapsed.chars().count() > DESCRIPTION_MAX_CHARS {
        let cut: String = collapsed.chars().take(DESCRIPTION_MAX_CHARS).collect();
        format!("{}…", cut.trim_end())
    } else {
        collapsed
    };
    ConstrainedBox::new(
        Clipped::new(small_text(appearance, &shown, sub_color(appearance))).finish(),
    )
    .with_max_height(DESCRIPTION_LINES * CONTENT_FONT_SIZE * LINE_HEIGHT_RATIO + 1.)
    .finish()
}

fn hook_location(hook: &HookItem) -> HookLocation {
    HookLocation {
        event: hook.event.clone(),
        group_index: hook.group_index,
        hook_index: hook.hook_index,
        expected_command: hook.command.clone(),
    }
}

fn sub_color(appearance: &Appearance) -> pathfinder_color::ColorU {
    blended_colors::text_sub(appearance.theme(), appearance.theme().surface_1())
}

fn small_text(
    appearance: &Appearance,
    text: &str,
    color: pathfinder_color::ColorU,
) -> Box<dyn Element> {
    Text::new(
        text.to_owned(),
        appearance.ui_font_family(),
        CONTENT_FONT_SIZE,
    )
    .with_color(color)
    .finish()
}

/// Shortens paths under the home directory to `~/…` for display.
fn display_path(path: &Path, home: Option<&Path>) -> String {
    if let Some(home) = home
        && let Ok(relative) = path.strip_prefix(home)
    {
        return format!("~/{}", relative.display());
    }
    path.display().to_string()
}

impl Entity for ClaudeCodePageView {
    type Event = ClaudeCodePageEvent;
}

impl TypedActionView for ClaudeCodePageView {
    type Action = ClaudeCodePageAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            ClaudeCodePageAction::Reload => {
                self.status = None;
                self.reload(ctx);
            }
            ClaudeCodePageAction::OpenNewMenu => {
                if self.open_menu == Some(MenuAnchor::New) {
                    self.close_menu(ctx);
                    return;
                }
                let mut options = self.new_options();
                // A single scope needs no menu: create right away.
                if options.len() == 1 {
                    let action = options.remove(0).action;
                    self.close_menu(ctx);
                    self.handle_action(&action, ctx);
                } else if !options.is_empty() {
                    self.show_menu(MenuAnchor::New, options, ctx);
                }
            }
            ClaudeCodePageAction::OpenCardMenu(card) => {
                let entries = self.card_menu_entries(card);
                if entries.is_empty() {
                    self.close_menu(ctx);
                } else {
                    self.show_menu(MenuAnchor::Card(card.clone()), entries, ctx);
                }
            }
            ClaudeCodePageAction::CloseMenu => self.close_menu(ctx),
            ClaudeCodePageAction::OpenFile(path) => {
                ctx.emit(ClaudeCodePageEvent::OpenFile(path.clone()));
            }
            ClaudeCodePageAction::CopyPath(path) => {
                ctx.clipboard()
                    .write(ClipboardContent::plain_text(path.display().to_string()));
                ctx.emit(ClaudeCodePageEvent::ShowToast {
                    message: "Ruta copiada".to_owned(),
                    flavor: ToastFlavor::Default,
                });
            }
            ClaudeCodePageAction::NewItem { kind, project } => {
                self.open_item_form(*kind, *project, None, ctx);
            }
            ClaudeCodePageAction::EditItem { kind, index } => {
                let item = self
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.items(*kind).get(*index))
                    .cloned();
                if let Some(item) = item {
                    let project = item.origin == ItemOrigin::Project;
                    self.open_item_form(*kind, project, Some(&item), ctx);
                }
            }
            ClaudeCodePageAction::SetSkillEnabled { index, enabled } => {
                let Some(snapshot) = &self.snapshot else {
                    return;
                };
                let Some(skill) = snapshot.skills.get(*index) else {
                    return;
                };
                // Mirrors Claude Code's `/skills` menu, which writes project skill states to the
                // gitignored local settings file.
                let default_scope = match skill.origin {
                    ItemOrigin::User => SettingsScope::User,
                    ItemOrigin::Project => SettingsScope::Local,
                    ItemOrigin::Plugin(_) => return,
                };
                let edit = ClaudeEdit::SetSkillOverride {
                    name: skill.name.clone(),
                    off: !*enabled,
                    default_scope,
                };
                self.run_edit(edit, ctx);
            }
            ClaudeCodePageAction::NewMcp(scope) => {
                let Some(snapshot) = &self.snapshot else {
                    return;
                };
                let scope = match scope {
                    NewMcpScope::User => McpScope::User,
                    NewMcpScope::Local => match snapshot.project_key() {
                        Some(key) => McpScope::Local(key),
                        None => return,
                    },
                    NewMcpScope::Project => McpScope::Project,
                };
                self.open_mcp_form(scope, None, None, ctx);
            }
            ClaudeCodePageAction::EditMcp(index) => {
                let server = self
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.mcp_servers.get(*index))
                    .cloned();
                if let Some(server) = server {
                    self.open_mcp_form(server.scope, Some(server.name), Some(server.raw), ctx);
                }
            }
            ClaudeCodePageAction::ToggleMcp(index) => {
                let Some(snapshot) = &self.snapshot else {
                    return;
                };
                let Some(server) = snapshot.mcp_servers.get(*index) else {
                    return;
                };
                let edit = match &server.scope {
                    McpScope::Project => ClaudeEdit::SetMcpjsonServerEnabled {
                        name: server.name.clone(),
                        enabled: server.state != McpState::Enabled,
                    },
                    McpScope::User | McpScope::Local(_) => {
                        let Some(project_key) = snapshot.project_key() else {
                            return;
                        };
                        ClaudeEdit::SetMcpServerDisabled {
                            project_key,
                            name: server.name.clone(),
                            disabled: server.state == McpState::Enabled,
                        }
                    }
                    McpScope::Plugin(_) => return,
                };
                self.run_edit(edit, ctx);
            }
            ClaudeCodePageAction::TogglePlugin(index) => {
                let Some(snapshot) = &self.snapshot else {
                    return;
                };
                let Some(plugin) = snapshot.plugins.get(*index) else {
                    return;
                };
                let edit = ClaudeEdit::SetPluginEnabled {
                    scope: plugin_toggle_scope(&snapshot.paths, &plugin.mentioned_in),
                    id: plugin.id.clone(),
                    enabled: plugin.enabled != Some(true),
                };
                self.run_edit(edit, ctx);
            }
            ClaudeCodePageAction::NewHook(scope) => {
                let specs = hook_field_specs(None);
                self.open_form(
                    FormKind::Hook {
                        scope: *scope,
                        original: None,
                    },
                    format!("Nuevo hook ({})", scope.label()),
                    "stdio",
                    specs,
                    ctx,
                );
            }
            ClaudeCodePageAction::EditHook(index) => {
                let hook = self
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.hooks.get(*index))
                    .cloned();
                let Some(hook) = hook else {
                    return;
                };
                let HookSource::Settings(scope) = hook.source else {
                    return;
                };
                let specs = hook_field_specs(Some(&hook));
                self.open_form(
                    FormKind::Hook {
                        scope,
                        original: Some(hook_location(&hook)),
                    },
                    format!("Editar hook {} ({})", hook.event, scope.label()),
                    "stdio",
                    specs,
                    ctx,
                );
            }
            ClaudeCodePageAction::EditSettings(scope) => self.open_settings_form(*scope, ctx),
            ClaudeCodePageAction::SetFormTransport(transport) => {
                if let Some(form) = &mut self.form {
                    form.transport = *transport;
                    ctx.notify();
                }
            }
            ClaudeCodePageAction::SaveForm => self.save_form(ctx),
            ClaudeCodePageAction::CancelForm => {
                self.form = None;
                ctx.notify();
            }
            ClaudeCodePageAction::RequestDelete(target) => {
                self.pending_delete = Some(*target);
                ctx.notify();
            }
            ClaudeCodePageAction::ConfirmDelete(target) => {
                if self.pending_delete != Some(*target) {
                    return;
                }
                match self.delete_edit(*target) {
                    Some(edit) => self.run_edit(edit, ctx),
                    None => {
                        self.pending_delete = None;
                        ctx.notify();
                    }
                }
            }
            ClaudeCodePageAction::CancelDelete => {
                self.pending_delete = None;
                ctx.notify();
            }
        }
    }
}

fn hook_field_specs(hook: Option<&HookItem>) -> Vec<FieldSpec> {
    vec![
        FieldSpec::line(
            "event",
            "Evento",
            hook.map(|hook| hook.event.clone()).unwrap_or_default(),
            "PreToolUse",
        ),
        FieldSpec::line(
            "matcher",
            "Matcher (vacío = todas las herramientas)",
            hook.and_then(|hook| hook.matcher.clone())
                .unwrap_or_default(),
            "Bash|Edit",
        ),
        FieldSpec::line(
            "command",
            "Comando",
            hook.and_then(|hook| hook.command.clone())
                .unwrap_or_default(),
            "~/.claude/hooks/mi-hook.sh",
        ),
        FieldSpec::line(
            "timeout",
            "Timeout en segundos (opcional)",
            hook.and_then(|hook| hook.timeout)
                .map(|timeout| timeout.to_string())
                .unwrap_or_default(),
            "60",
        ),
    ]
}

impl View for ClaudeCodePageView {
    fn ui_name() -> &'static str {
        "ClaudeCodeSettingsPage"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        self.page.render(self, app)
    }
}

impl SettingsPageMeta for ClaudeCodePageView {
    fn section() -> SettingsSection {
        SettingsSection::ClaudeCode
    }

    fn on_page_selected(&mut self, _allow_steal_focus: bool, ctx: &mut ViewContext<Self>) {
        // The active pane's project may have changed since the page was last shown.
        if self.form.is_none() {
            self.reload(ctx);
        }
    }

    fn should_render(&self, _ctx: &AppContext) -> bool {
        cfg!(not(target_family = "wasm"))
    }

    fn update_filter(&mut self, query: &str, ctx: &mut ViewContext<Self>) -> MatchData {
        self.page.update_filter(query, ctx)
    }

    fn scroll_to_widget(&mut self, widget_id: &'static str) {
        self.page.scroll_to_widget(widget_id)
    }

    fn clear_highlighted_widget(&mut self) {
        self.page.clear_highlighted_widget();
    }
}

impl From<ViewHandle<ClaudeCodePageView>> for SettingsPageViewHandle {
    fn from(view_handle: ViewHandle<ClaudeCodePageView>) -> Self {
        SettingsPageViewHandle::ClaudeCode(view_handle)
    }
}

// ── Widgets ────────────────────────────────────────────────────────────

/// The discreet status area under the title: folders, backups, errors, the last result and the
/// reload buttons.
struct OverviewWidget {
    /// Only one subpage carries the overview's search terms, so a settings search for "backup"
    /// lists one page instead of all of them.
    searchable: bool,
}

impl SettingsWidget for OverviewWidget {
    type View = ClaudeCodePageView;

    fn search_terms(&self) -> &str {
        if self.searchable {
            "claude code configuración proyecto usuario carpeta config dir recargar copias \
             respaldo backup"
        } else {
            ""
        }
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        _app: &AppContext,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let muted = sub_color(appearance);
        let mut info = Flex::column().with_spacing(2.);
        match &view.snapshot {
            Some(snapshot) => {
                let paths = &snapshot.paths;
                let home = snapshot.home_dir.as_deref();
                let project = match &paths.project {
                    Some(project) => display_path(&project.root, home),
                    None => "ninguno (enfoca una pestaña de terminal dentro de un proyecto y \
                             pulsa Recargar)"
                        .to_owned(),
                };
                info.add_child(small_text(
                    appearance,
                    &format!(
                        "Usuario: {} · Proyecto: {project}",
                        display_path(&paths.config_dir, home)
                    ),
                    muted,
                ));
                info.add_child(small_text(
                    appearance,
                    &format!(
                        "Cada cambio guarda antes una copia en {}",
                        display_path(&paths.backups_dir(), home)
                    ),
                    muted,
                ));
                if let Some(error) = &snapshot.claude_json_error {
                    info.add_child(small_text(
                        appearance,
                        &format!("{}: {error}", display_path(&paths.claude_json, home)),
                        theme.ui_error_color(),
                    ));
                }
                for warning in &snapshot.warnings {
                    info.add_child(small_text(appearance, warning, theme.ui_warning_color()));
                }
            }
            None => info.add_child(small_text(appearance, "Cargando…", muted)),
        }
        match &view.status {
            Some(Status::Success(message)) => {
                info.add_child(small_text(appearance, message, theme.ui_green_color()))
            }
            Some(Status::Error(message)) => {
                info.add_child(small_text(appearance, message, theme.ui_error_color()))
            }
            None => {}
        }
        if view.show_reload_hint {
            info.add_child(small_text(
                appearance,
                RELOAD_HINT,
                theme.ui_warning_color(),
            ));
        }

        let small_button = |variant, key: &str, label: &str| {
            appearance
                .ui_builder()
                .button(variant, view.mouse_state(key))
                .with_text_label(label.to_owned())
                .with_style(UiComponentStyles {
                    font_size: Some(CONTENT_FONT_SIZE),
                    padding: Some(Coords::uniform(4.).left(8.).right(8.)),
                    ..Default::default()
                })
        };
        let mut reload = small_button(
            ButtonVariant::Text,
            "reload_snapshot",
            if view.loading {
                "Cargando…"
            } else {
                "Recargar"
            },
        );
        if view.loading || view.busy {
            reload = reload.disabled();
        }
        let reload_all_tabs = small_button(
            if view.show_reload_hint {
                ButtonVariant::Accent
            } else {
                ButtonVariant::Text
            },
            "reload_claude_in_all_tabs",
            RELOAD_ALL_TABS_LABEL,
        );
        let buttons = Wrap::row()
            .with_spacing(4.)
            .with_run_spacing(4.)
            .with_child(
                reload
                    .build()
                    .on_click(|ctx, _, _| ctx.dispatch_typed_action(ClaudeCodePageAction::Reload))
                    .finish(),
            )
            .with_child(
                reload_all_tabs
                    .build()
                    .on_click(|ctx, _, _| {
                        ctx.dispatch_typed_action(WorkspaceAction::ReloadClaudeInAllTabs)
                    })
                    .finish(),
            )
            .finish();

        Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Start)
                .with_spacing(8.)
                .with_child(Expanded::new(1., info.finish()).finish())
                .with_child(buttons)
                .finish(),
        )
        .with_padding_bottom(ROW_SPACING)
        .with_margin_bottom(HEADER_PADDING)
        .with_border(Border::bottom(1.).with_border_fill(theme.outline()))
        .finish()
    }
}

/// Skills, subagents or slash commands.
struct ItemsWidget {
    kind: MdKind,
    search_terms: &'static str,
    empty_text: &'static str,
}

impl ItemsWidget {
    fn skills() -> Self {
        Self {
            kind: MdKind::Skill,
            search_terms: "claude code skills skill.md habilidades crear editar eliminar \
                           desactivar skilloverrides",
            empty_text: "No hay skills.",
        }
    }

    fn agents() -> Self {
        Self {
            kind: MdKind::Agent,
            search_terms: "claude code subagentes subagents agents agentes crear editar eliminar",
            empty_text: "No hay subagentes.",
        }
    }

    fn commands() -> Self {
        Self {
            kind: MdKind::Command,
            search_terms: "claude code comandos slash commands crear editar eliminar",
            empty_text: "No hay comandos slash.",
        }
    }
}

impl SettingsWidget for ItemsWidget {
    type View = ClaudeCodePageView;

    fn widget_id(&self) -> &'static str {
        match self.kind {
            MdKind::Agent => "ClaudeCodeAgentsWidget",
            MdKind::Command => "ClaudeCodeCommandsWidget",
            MdKind::Skill => "ClaudeCodeSkillItemsWidget",
        }
    }

    fn search_terms(&self) -> &str {
        self.search_terms
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let (cards, total) = view.item_cards(self.kind, &view.search_query(app), appearance);
        view.render_list(
            appearance,
            Some(FormSection::Items(self.kind)),
            cards,
            total,
            self.empty_text,
        )
    }
}

struct McpWidget;

impl SettingsWidget for McpWidget {
    type View = ClaudeCodePageView;

    fn search_terms(&self) -> &str {
        "claude code mcp servers servidores stdio http sse env headers mcp.json claude.json"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let query = view.search_query(app);
        let mut cards = Vec::new();
        let mut total = 0;
        if let Some(snapshot) = &view.snapshot {
            total = snapshot.mcp_servers.len();
            for (index, server) in snapshot.mcp_servers.iter().enumerate() {
                let summary = server.summary();
                if !matches_query(&query, &server.name, Some(&summary)) {
                    continue;
                }
                let mut card = Card::new(format!("mcp:{index}"), server.name.clone());
                card.menu = Some(CardRef::Mcp(index));
                card.chips = vec![server.scope.label(), server.transport.clone()];
                card.description = Some(summary);
                let mut secrets = Vec::new();
                if !server.env_keys.is_empty() {
                    secrets.push(format!(
                        "env: {}",
                        server
                            .env_keys
                            .iter()
                            .map(|key| format!("{key}={MASK}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                if !server.header_keys.is_empty() {
                    secrets.push(format!(
                        "headers: {}",
                        server
                            .header_keys
                            .iter()
                            .map(|key| format!("{key}: {MASK}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                if !secrets.is_empty() {
                    card.detail = Some(secrets.join(" · "));
                }
                let writable = !server.scope.is_read_only();
                // User and local servers are switched off per project (`disabledMcpServers`), so
                // they need an open project; `.mcp.json` approval also lives in the project.
                let can_toggle = writable
                    && match server.scope {
                        McpScope::Project | McpScope::User | McpScope::Local(_) => {
                            snapshot.paths.project.is_some()
                        }
                        McpScope::Plugin(_) => false,
                    };
                let state = match server.state {
                    McpState::Enabled => "activo",
                    McpState::Disabled => "desactivado",
                    McpState::PendingApproval => "pendiente de aprobación (actívalo para aprobar)",
                };
                if can_toggle {
                    card.toggle = Some((
                        server.state == McpState::Enabled,
                        ClaudeCodePageAction::ToggleMcp(index),
                    ));
                    if server.state == McpState::PendingApproval {
                        card.chips.push(state.to_owned());
                    }
                } else {
                    card.chips.push(state.to_owned());
                }
                if writable {
                    card.delete_target = Some(DeleteTarget::Mcp(index));
                }
                cards.push(view.render_card(card, appearance));
            }
        }
        view.render_list(
            appearance,
            Some(FormSection::Mcp),
            cards,
            total,
            "No hay servidores MCP.",
        )
    }
}

struct PluginsWidget;

impl SettingsWidget for PluginsWidget {
    type View = ClaudeCodePageView;

    fn search_terms(&self) -> &str {
        "claude code plugins marketplaces enabledplugins activar desactivar"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let query = view.search_query(app);
        let mut cards = Vec::new();
        let mut total = 0;
        if let Some(snapshot) = &view.snapshot {
            total = snapshot.plugins.len() + snapshot.marketplaces.len();
            for (index, plugin) in snapshot.plugins.iter().enumerate() {
                let install_scope = plugin
                    .install_scope
                    .as_ref()
                    .map(|scope| format!("Instalado con scope {scope}"));
                if !matches_query(&query, &plugin.id, install_scope.as_deref()) {
                    continue;
                }
                let mut card = Card::new(format!("plugin:{index}"), plugin.id.clone());
                card.menu = Some(CardRef::Plugin(index));
                card.chips.push("Plugin".to_owned());
                if let Some(version) = &plugin.version {
                    card.chips.push(format!("v{version}"));
                }
                card.chips.push(
                    if plugin.installed {
                        "instalado"
                    } else {
                        "no instalado"
                    }
                    .to_owned(),
                );
                card.chips
                    .push(match (plugin.enabled, plugin.enabled_source) {
                        (Some(true), Some(source)) => format!("activado ({})", source.label()),
                        (Some(false), Some(source)) => {
                            format!("desactivado ({})", source.label())
                        }
                        (Some(_), None) | (None, Some(_)) | (None, None) => {
                            "no activado".to_owned()
                        }
                    });
                card.description = install_scope;
                let target = plugin_toggle_scope(&snapshot.paths, &plugin.mentioned_in);
                card.detail = Some(format!("El interruptor escribe en: {}", target.label()));
                card.toggle = Some((
                    plugin.enabled == Some(true),
                    ClaudeCodePageAction::TogglePlugin(index),
                ));
                cards.push(view.render_card(card, appearance));
            }
            let mut marketplace_cards = Vec::new();
            for (index, marketplace) in snapshot.marketplaces.iter().enumerate() {
                if !matches_query(&query, &marketplace.name, Some(&marketplace.source)) {
                    continue;
                }
                let mut card = Card::new(format!("marketplace:{index}"), marketplace.name.clone());
                card.menu = Some(CardRef::Marketplace(index));
                card.chips.push("Marketplace".to_owned());
                card.description = Some(marketplace.source.clone());
                marketplace_cards.push(view.render_card(card, appearance));
            }
            if !marketplace_cards.is_empty() {
                cards.push(render_sub_header(appearance, "Marketplaces", None));
                cards.extend(marketplace_cards);
            }
        }
        view.render_list(appearance, None, cards, total, "No hay plugins instalados.")
    }
}

struct HooksWidget;

impl SettingsWidget for HooksWidget {
    type View = ClaudeCodePageView;

    fn search_terms(&self) -> &str {
        "claude code hooks pretooluse posttooluse stop sessionstart matcher comando timeout"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let query = view.search_query(app);
        let mut cards = Vec::new();
        let mut total = 0;
        if let Some(snapshot) = &view.snapshot {
            total = snapshot.hooks.len();
            for (index, hook) in snapshot.hooks.iter().enumerate() {
                let title = match &hook.matcher {
                    Some(matcher) if !matcher.is_empty() => format!("{} · {matcher}", hook.event),
                    _ => hook.event.clone(),
                };
                if !matches_query(&query, &title, hook.command.as_deref()) {
                    continue;
                }
                let mut card = Card::new(format!("hook:{index}"), title);
                card.menu = Some(CardRef::Hook(index));
                card.chips = vec![hook.source.label(), hook.hook_type.clone()];
                if let Some(timeout) = hook.timeout {
                    card.chips.push(format!("{timeout}s"));
                }
                card.description = hook.command.clone();
                if hook_is_editable(hook) {
                    card.delete_target = Some(DeleteTarget::Hook(index));
                }
                cards.push(view.render_card(card, appearance));
            }
        }
        view.render_list(
            appearance,
            Some(FormSection::Hooks),
            cards,
            total,
            "No hay hooks.",
        )
    }
}

struct SettingsFilesWidget;

impl SettingsWidget for SettingsFilesWidget {
    type View = ClaudeCodePageView;

    fn search_terms(&self) -> &str {
        "claude code settings.json settings.local.json managed configuración modelo model \
         permisos permissions allow deny ask defaultmode env statusline outputstyle"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let query = view.search_query(app);
        let mut cards = Vec::new();
        let mut total = 0;
        if let Some(snapshot) = &view.snapshot {
            let home = snapshot.home_dir.as_deref();
            total = snapshot.settings.len();
            for file in &snapshot.settings {
                let path = display_path(&file.path, home);
                if !matches_query(&query, file.scope.label(), Some(&path)) {
                    continue;
                }
                let mut card = Card::new(
                    format!("settings:{:?}", file.scope),
                    file.scope.label().to_owned(),
                );
                card.menu = Some(CardRef::SettingsFile(file.scope));
                card.chips.push(
                    if file.error.is_some() {
                        "inválido"
                    } else if file.exists {
                        "existe"
                    } else {
                        "no existe"
                    }
                    .to_owned(),
                );
                if !file.scope.is_writable() {
                    card.chips.push("solo lectura".to_owned());
                }
                card.detail = Some(path);
                card.error = file.error.clone();
                cards.push(view.render_card(card, appearance));
            }
            if !snapshot.attributions.is_empty() {
                total += 1;
                let mut lines = Vec::new();
                for attribution in &snapshot.attributions {
                    let effective = attribution
                        .entries
                        .iter()
                        .find(|(scope, _)| *scope == attribution.effective)
                        .map(|(_, summary)| summary.as_str())
                        .unwrap_or_default();
                    let mut line = format!(
                        "{} = {effective} ({})",
                        attribution.key,
                        attribution.effective.label()
                    );
                    if attribution.entries.len() > 1 {
                        let others = attribution
                            .entries
                            .iter()
                            .filter(|(scope, _)| *scope != attribution.effective)
                            .map(|(scope, _)| scope.label())
                            .collect::<Vec<_>>()
                            .join(", ");
                        line.push_str(&format!(" · también en: {others}"));
                    }
                    lines.push(line);
                }
                let lines = lines.join("\n");
                if matches_query(&query, "Valores efectivos", Some(&lines)) {
                    let mut card = Card::new(
                        "settings:attribution".to_owned(),
                        "Valores efectivos".to_owned(),
                    );
                    card.chips
                        .push("archivo que gana entre paréntesis".to_owned());
                    card.description = Some(lines);
                    card.full_text = true;
                    cards.push(view.render_card(card, appearance));
                }
            }
            if !snapshot.permission_rules.is_empty() {
                total += 1;
                let lines = snapshot
                    .permission_rules
                    .iter()
                    .map(|rule| format!("{}: {} ({})", rule.kind, rule.rule, rule.scope.label()))
                    .collect::<Vec<_>>()
                    .join("\n");
                if matches_query(&query, "Reglas de permisos", Some(&lines)) {
                    let mut card = Card::new(
                        "settings:permissions".to_owned(),
                        "Reglas de permisos".to_owned(),
                    );
                    card.chips.push("se combinan entre archivos".to_owned());
                    card.description = Some(lines);
                    card.full_text = true;
                    cards.push(view.render_card(card, appearance));
                }
            }
        }
        Flex::column()
            .with_child(render_sub_header(
                appearance,
                "Archivos de configuración",
                None,
            ))
            .with_child(view.render_list(
                appearance,
                Some(FormSection::Settings),
                cards,
                total,
                "No hay archivos de configuración.",
            ))
            .finish()
    }
}

struct MemoryWidget;

impl SettingsWidget for MemoryWidget {
    type View = ClaudeCodePageView;

    fn search_terms(&self) -> &str {
        "claude code memoria memory claude.md claude.local.md"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let query = view.search_query(app);
        let mut cards = Vec::new();
        let mut total = 0;
        if let Some(snapshot) = &view.snapshot {
            total = snapshot.memory_files.len();
            for (index, file) in snapshot.memory_files.iter().enumerate() {
                let path = display_path(&file.path, snapshot.home_dir.as_deref());
                if !matches_query(&query, &file.label, Some(&path)) {
                    continue;
                }
                let mut card = Card::new(format!("memory:{index}"), file.label.clone());
                card.menu = Some(CardRef::Memory(index));
                card.chips
                    .push(if file.exists { "existe" } else { "no existe" }.to_owned());
                card.detail = Some(path);
                cards.push(view.render_card(card, appearance));
            }
        }
        Flex::column()
            .with_child(render_sub_header(appearance, "Archivos de memoria", None))
            .with_child(view.render_list(
                appearance,
                None,
                cards,
                total,
                "No hay archivos de memoria.",
            ))
            .finish()
    }
}
