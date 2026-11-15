//! The "Claude Code" settings page: views and edits the user's Claude Code configuration
//! (skills, MCP servers, plugins, subagents, slash commands, hooks, settings files and memory
//! files) for the user scope and the project of the active terminal pane.
//!
//! All disk I/O runs on the background executor through [`claude_code::snapshot`] and
//! [`claude_code::edits`]; this view only holds the latest snapshot and the open form.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use warpui::clipboard::ClipboardContent;
use warpui::elements::{
    Border, ChildView, Clipped, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox,
    Container, CornerRadius, CrossAxisAlignment, Element, Fill, Flex, MainAxisSize,
    MouseStateHandle, ParentElement, Radius, ScrollbarWidth, Shrinkable, Text, Wrap,
};
use warpui::fonts::{Properties, Weight};
use warpui::ui_components::button::ButtonVariant;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
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
    SettingsPageViewHandle, SettingsWidget, render_separator, render_sub_header,
};
use super::{SettingsSection, editor_text_colors};
use crate::appearance::Appearance;
use crate::editor::{
    EditorOptions, EditorView, EnterAction, EnterSettings, PropagateAndNoOpNavigationKeys,
    SingleLineEditorOptions, TextOptions,
};
use crate::ui_components::blended_colors;
use crate::view_components::ToastFlavor;
use crate::workspace::{ActiveSession, WorkspaceAction};

const PAGE_TITLE: &str = "Claude Code";
const RELOAD_HINT: &str = "Recarga Claude en las pestañas para aplicar el cambio";
const RELOAD_ALL_TABS_LABEL: &str = "Recargar Claude en todas las pestañas";
const TITLE_FONT_SIZE: f32 = 13.;
const ROW_SPACING: f32 = 10.;
const BODY_EDITOR_MIN_HEIGHT: f32 = 120.;
const BODY_EDITOR_MAX_HEIGHT: f32 = 360.;
const KEY_ORDER_NOTE: &str = "Al guardar, las claves JSON se ordenan alfabéticamente (los valores \
                              no cambian).";
const DEFAULT_MODE_PLACEHOLDER: &str =
    "default, acceptEdits, plan, dontAsk, bypassPermissions, auto";
const HOOK_EVENTS_HINT: &str = "PreToolUse, PostToolUse, UserPromptSubmit, Notification, Stop, \
                                SubagentStop, PreCompact, SessionStart, SessionEnd";

#[derive(Clone, Debug, PartialEq)]
pub enum ClaudeCodePageAction {
    Reload,
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
    reload_mouse_state: MouseStateHandle,
}

impl ClaudeCodePageView {
    pub fn new() -> Self {
        let widgets: Vec<Box<dyn SettingsWidget<View = Self>>> = vec![
            Box::new(OverviewWidget),
            Box::new(SkillsWidget),
            Box::new(McpWidget),
            Box::new(PluginsWidget),
            Box::new(ItemsWidget::agents()),
            Box::new(ItemsWidget::commands()),
            Box::new(HooksWidget),
            Box::new(SettingsFilesWidget),
            Box::new(MemoryWidget),
        ];
        let title =
            PageTitle::new(PAGE_TITLE).with_trailing_element(|view: &Self, appearance, _| {
                let label = if view.loading {
                    "Cargando…"
                } else {
                    "Recargar"
                };
                let mut button = appearance
                    .ui_builder()
                    .button(ButtonVariant::Secondary, view.reload_mouse_state.clone())
                    .with_text_label(label.to_owned());
                if view.loading || view.busy {
                    button = button.disabled();
                }
                button
                    .build()
                    .on_click(|ctx, _, _| {
                        ctx.dispatch_typed_action(ClaudeCodePageAction::Reload);
                    })
                    .finish()
            });
        Self {
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
            reload_mouse_state: Default::default(),
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

    /// "Abrir archivo" (when `can_open`) and "Copiar ruta". Existence comes from the snapshot so
    /// rendering never touches the disk.
    fn file_buttons(
        &self,
        appearance: &Appearance,
        key: &str,
        path: &Path,
        can_open: bool,
    ) -> Vec<Box<dyn Element>> {
        let mut buttons = Vec::new();
        if can_open {
            buttons.push(self.button(
                appearance,
                format!("{key}:open"),
                "Abrir archivo",
                ClaudeCodePageAction::OpenFile(path.to_path_buf()),
                ButtonVariant::Text,
            ));
        }
        buttons.push(self.button(
            appearance,
            format!("{key}:copy"),
            "Copiar ruta",
            ClaudeCodePageAction::CopyPath(path.to_path_buf()),
            ButtonVariant::Text,
        ));
        buttons
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

    fn render_section(
        &self,
        appearance: &Appearance,
        title: &'static str,
        actions: Vec<Box<dyn Element>>,
        form_section: Option<FormSection>,
        rows: Vec<Box<dyn Element>>,
        empty_text: &str,
    ) -> Box<dyn Element> {
        let mut column = Flex::column().with_child(render_sub_header(appearance, title, None));
        if !actions.is_empty() {
            let mut wrap = Wrap::row().with_spacing(6.).with_run_spacing(6.);
            for action in actions {
                wrap.add_child(action);
            }
            column.add_child(
                Container::new(wrap.finish())
                    .with_margin_bottom(ROW_SPACING)
                    .finish(),
            );
        }
        if let Some(form) =
            form_section.and_then(|section| self.render_form_if(section, appearance))
        {
            column.add_child(form);
        }
        if self.snapshot.is_none() {
            column.add_child(small_text(appearance, "Cargando…", sub_color(appearance)));
        } else if rows.is_empty() {
            column.add_child(small_text(appearance, empty_text, sub_color(appearance)));
        } else {
            for row in rows {
                column.add_child(row);
            }
        }
        column.add_child(
            Container::new(render_separator(appearance))
                .with_margin_top(HEADER_PADDING)
                .finish(),
        );
        column.finish()
    }

    fn render_item_rows(&self, kind: MdKind, appearance: &Appearance) -> Vec<Box<dyn Element>> {
        let Some(snapshot) = &self.snapshot else {
            return Vec::new();
        };
        snapshot
            .items(kind)
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let key = format!("{}:{index}", kind.subdir());
                let mut badges = vec![item.origin.label()];
                if let Some(state) = &item.override_state {
                    badges.push(format!("skillOverrides: {state}"));
                }
                let mut buttons = Vec::new();
                if !item.origin.is_read_only() {
                    if item.doc.is_some() {
                        buttons.push(self.button(
                            appearance,
                            format!("{key}:edit"),
                            "Editar",
                            ClaudeCodePageAction::EditItem { kind, index },
                            ButtonVariant::Secondary,
                        ));
                    }
                    if kind == MdKind::Skill
                        && (item.origin == ItemOrigin::User || snapshot.paths.project.is_some())
                    {
                        // Partial overrides (`name-only`, `user-invocable-only`…) get explicit
                        // labels so neither button silently replaces them.
                        let (enable_label, disable_label) = match item.override_state.as_deref() {
                            None => (None, Some("Desactivar".to_owned())),
                            Some("off") => (Some("Activar".to_owned()), None),
                            Some(state) => (
                                Some(format!("Activar (quita \"{state}\")")),
                                Some(format!("Desactivar (reemplaza \"{state}\")")),
                            ),
                        };
                        if let Some(label) = enable_label {
                            buttons.push(self.button(
                                appearance,
                                format!("{key}:enable"),
                                &label,
                                ClaudeCodePageAction::SetSkillEnabled {
                                    index,
                                    enabled: true,
                                },
                                ButtonVariant::Secondary,
                            ));
                        }
                        if let Some(label) = disable_label {
                            buttons.push(self.button(
                                appearance,
                                format!("{key}:disable"),
                                &label,
                                ClaudeCodePageAction::SetSkillEnabled {
                                    index,
                                    enabled: false,
                                },
                                ButtonVariant::Secondary,
                            ));
                        }
                    }
                    buttons.extend(self.delete_buttons(
                        appearance,
                        &key,
                        DeleteTarget::Item(kind, index),
                    ));
                }
                buttons.extend(self.file_buttons(appearance, &key, &item.path, true));
                render_row(
                    appearance,
                    &item.name,
                    &badges,
                    item.description(),
                    item.error.as_deref(),
                    buttons,
                )
            })
            .collect()
    }

    fn new_item_buttons(&self, kind: MdKind, appearance: &Appearance) -> Vec<Box<dyn Element>> {
        let mut buttons = vec![self.button(
            appearance,
            format!("{}:new:user", kind.subdir()),
            &format!("Nuevo {} (usuario)", kind.label()),
            ClaudeCodePageAction::NewItem {
                kind,
                project: false,
            },
            ButtonVariant::Secondary,
        )];
        if self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.paths.project.is_some())
        {
            buttons.push(self.button(
                appearance,
                format!("{}:new:project", kind.subdir()),
                &format!("Nuevo {} (proyecto)", kind.label()),
                ClaudeCodePageAction::NewItem {
                    kind,
                    project: true,
                },
                ButtonVariant::Secondary,
            ));
        }
        buttons
    }
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

fn render_row(
    appearance: &Appearance,
    title: &str,
    badges: &[String],
    subtitle: Option<String>,
    error: Option<&str>,
    buttons: Vec<Box<dyn Element>>,
) -> Box<dyn Element> {
    let theme = appearance.theme();
    let mut header = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Shrinkable::new(
                1.,
                Text::new(
                    title.to_owned(),
                    appearance.ui_font_family(),
                    TITLE_FONT_SIZE,
                )
                .with_color(theme.active_ui_text_color().into_solid())
                .with_style(Properties::default().weight(Weight::Semibold))
                .finish(),
            )
            .finish(),
        );
    if !badges.is_empty() {
        header.add_child(
            Container::new(small_text(
                appearance,
                &badges.join(" · "),
                sub_color(appearance),
            ))
            .with_margin_left(8.)
            .finish(),
        );
    }
    let mut column = Flex::column().with_child(header.finish());
    if let Some(subtitle) = subtitle.filter(|subtitle| !subtitle.trim().is_empty()) {
        column.add_child(
            Container::new(small_text(appearance, &subtitle, sub_color(appearance)))
                .with_margin_top(2.)
                .finish(),
        );
    }
    if let Some(error) = error {
        column.add_child(
            Container::new(small_text(
                appearance,
                &format!("Error: {error}"),
                theme.ui_error_color(),
            ))
            .with_margin_top(2.)
            .finish(),
        );
    }
    if !buttons.is_empty() {
        let mut wrap = Wrap::row().with_spacing(4.).with_run_spacing(4.);
        for button in buttons {
            wrap.add_child(button);
        }
        column.add_child(Container::new(wrap.finish()).with_margin_top(4.).finish());
    }
    Container::new(column.finish())
        .with_padding_bottom(ROW_SPACING)
        .with_margin_bottom(ROW_SPACING)
        .with_border(Border::bottom(1.).with_border_fill(theme.outline()))
        .finish()
}

impl Default for ClaudeCodePageView {
    fn default() -> Self {
        Self::new()
    }
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

struct OverviewWidget;

impl SettingsWidget for OverviewWidget {
    type View = ClaudeCodePageView;

    fn search_terms(&self) -> &str {
        "claude code configuración proyecto usuario carpeta config dir recargar copias respaldo backup"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        _app: &AppContext,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let mut column = Flex::column();
        match &view.snapshot {
            Some(snapshot) => {
                let paths = &snapshot.paths;
                let home = snapshot.home_dir.as_deref();
                column.add_child(small_text(
                    appearance,
                    &format!(
                        "Carpeta de usuario: {}",
                        display_path(&paths.config_dir, home)
                    ),
                    sub_color(appearance),
                ));
                let project = match &paths.project {
                    Some(project) => format!("Proyecto: {}", display_path(&project.root, home)),
                    None => "Proyecto: ninguno (enfoca una pestaña de terminal dentro de un \
                             proyecto y pulsa Recargar)"
                        .to_owned(),
                };
                column.add_child(small_text(appearance, &project, sub_color(appearance)));
                column.add_child(small_text(
                    appearance,
                    &format!(
                        "Cada cambio guarda antes una copia en {}",
                        display_path(&paths.backups_dir(), home)
                    ),
                    sub_color(appearance),
                ));
                column.add_child(small_text(
                    appearance,
                    KEY_ORDER_NOTE,
                    sub_color(appearance),
                ));
                if let Some(error) = &snapshot.claude_json_error {
                    column.add_child(small_text(
                        appearance,
                        &format!("{}: {error}", display_path(&paths.claude_json, home)),
                        theme.ui_error_color(),
                    ));
                }
                for warning in &snapshot.warnings {
                    column.add_child(small_text(appearance, warning, theme.ui_warning_color()));
                }
            }
            None => column.add_child(small_text(appearance, "Cargando…", sub_color(appearance))),
        }
        match &view.status {
            Some(Status::Success(message)) => {
                column.add_child(small_text(appearance, message, theme.ui_green_color()))
            }
            Some(Status::Error(message)) => {
                column.add_child(small_text(appearance, message, theme.ui_error_color()))
            }
            None => {}
        }
        if view.show_reload_hint {
            column.add_child(
                Container::new(small_text(
                    appearance,
                    RELOAD_HINT,
                    theme.ui_warning_color(),
                ))
                .with_margin_top(4.)
                .finish(),
            );
            column.add_child(
                Container::new(
                    appearance
                        .ui_builder()
                        .button(
                            ButtonVariant::Accent,
                            view.mouse_state("reload_claude_in_all_tabs"),
                        )
                        .with_text_label(RELOAD_ALL_TABS_LABEL.to_owned())
                        .with_style(UiComponentStyles {
                            font_size: Some(CONTENT_FONT_SIZE),
                            padding: Some(Coords::uniform(4.).left(8.).right(8.)),
                            ..Default::default()
                        })
                        .build()
                        .on_click(|ctx, _, _| {
                            ctx.dispatch_typed_action(WorkspaceAction::ReloadClaudeInAllTabs)
                        })
                        .finish(),
                )
                .with_margin_top(4.)
                .finish(),
            );
        }
        Container::new(column.finish())
            .with_margin_bottom(HEADER_PADDING)
            .finish()
    }
}

struct SkillsWidget;

impl SettingsWidget for SkillsWidget {
    type View = ClaudeCodePageView;

    fn search_terms(&self) -> &str {
        "claude code skills skill.md habilidades crear editar eliminar desactivar skilloverrides"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        _app: &AppContext,
    ) -> Box<dyn Element> {
        view.render_section(
            appearance,
            "Skills",
            view.new_item_buttons(MdKind::Skill, appearance),
            Some(FormSection::Items(MdKind::Skill)),
            view.render_item_rows(MdKind::Skill, appearance),
            "No hay skills.",
        )
    }
}

/// Subagents or slash commands.
struct ItemsWidget {
    kind: MdKind,
    title: &'static str,
    search_terms: &'static str,
    empty_text: &'static str,
}

impl ItemsWidget {
    fn agents() -> Self {
        Self {
            kind: MdKind::Agent,
            title: "Subagentes",
            search_terms: "claude code subagentes subagents agents agentes crear editar eliminar",
            empty_text: "No hay subagentes.",
        }
    }

    fn commands() -> Self {
        Self {
            kind: MdKind::Command,
            title: "Comandos slash",
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
        _app: &AppContext,
    ) -> Box<dyn Element> {
        view.render_section(
            appearance,
            self.title,
            view.new_item_buttons(self.kind, appearance),
            Some(FormSection::Items(self.kind)),
            view.render_item_rows(self.kind, appearance),
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
        _app: &AppContext,
    ) -> Box<dyn Element> {
        let mut actions = vec![view.button(
            appearance,
            "mcp:new:user".to_owned(),
            "Nuevo servidor (usuario)",
            ClaudeCodePageAction::NewMcp(NewMcpScope::User),
            ButtonVariant::Secondary,
        )];
        let mut rows = Vec::new();
        if let Some(snapshot) = &view.snapshot {
            if snapshot.paths.project.is_some() {
                actions.push(view.button(
                    appearance,
                    "mcp:new:local".to_owned(),
                    "Nuevo servidor (local)",
                    ClaudeCodePageAction::NewMcp(NewMcpScope::Local),
                    ButtonVariant::Secondary,
                ));
                actions.push(view.button(
                    appearance,
                    "mcp:new:project".to_owned(),
                    "Nuevo servidor (proyecto, .mcp.json)",
                    ClaudeCodePageAction::NewMcp(NewMcpScope::Project),
                    ButtonVariant::Secondary,
                ));
            }
            for (index, server) in snapshot.mcp_servers.iter().enumerate() {
                let key = format!("mcp:{index}");
                let state = match server.state {
                    McpState::Enabled => "activo",
                    McpState::Disabled => "desactivado",
                    McpState::PendingApproval => "pendiente de aprobación",
                };
                let badges = vec![
                    server.scope.label(),
                    server.transport.clone(),
                    state.to_owned(),
                ];
                let mut subtitle = server.summary();
                if !server.env_keys.is_empty() {
                    subtitle.push_str(&format!(
                        "\nenv: {}",
                        server
                            .env_keys
                            .iter()
                            .map(|key| format!("{key}={MASK}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                if !server.header_keys.is_empty() {
                    subtitle.push_str(&format!(
                        "\nheaders: {}",
                        server
                            .header_keys
                            .iter()
                            .map(|key| format!("{key}: {MASK}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                let mut buttons = Vec::new();
                if !server.scope.is_read_only() {
                    if mcp_form_transport(&server.raw).is_some() {
                        buttons.push(view.button(
                            appearance,
                            format!("{key}:edit"),
                            "Editar",
                            ClaudeCodePageAction::EditMcp(index),
                            ButtonVariant::Secondary,
                        ));
                    }
                    let can_toggle = match server.scope {
                        McpScope::Project | McpScope::User | McpScope::Local(_) => {
                            snapshot.paths.project.is_some()
                        }
                        McpScope::Plugin(_) => false,
                    };
                    if can_toggle {
                        let label = match (&server.scope, server.state) {
                            (McpScope::Project, McpState::PendingApproval) => "Aprobar",
                            (_, McpState::Enabled) => "Desactivar",
                            (_, McpState::Disabled | McpState::PendingApproval) => "Activar",
                        };
                        buttons.push(view.button(
                            appearance,
                            format!("{key}:toggle"),
                            label,
                            ClaudeCodePageAction::ToggleMcp(index),
                            ButtonVariant::Secondary,
                        ));
                    }
                    buttons.extend(view.delete_buttons(appearance, &key, DeleteTarget::Mcp(index)));
                }
                buttons.extend(view.file_buttons(appearance, &key, &server.path, true));
                rows.push(render_row(
                    appearance,
                    &server.name,
                    &badges,
                    Some(subtitle),
                    None,
                    buttons,
                ));
            }
        }
        view.render_section(
            appearance,
            "Servidores MCP",
            actions,
            Some(FormSection::Mcp),
            rows,
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
        _app: &AppContext,
    ) -> Box<dyn Element> {
        let mut rows = Vec::new();
        if let Some(snapshot) = &view.snapshot {
            for (index, plugin) in snapshot.plugins.iter().enumerate() {
                let key = format!("plugin:{index}");
                let mut badges = Vec::new();
                if let Some(version) = &plugin.version {
                    badges.push(format!("v{version}"));
                }
                badges.push(
                    if plugin.installed {
                        "instalado"
                    } else {
                        "no instalado"
                    }
                    .to_owned(),
                );
                badges.push(match (plugin.enabled, plugin.enabled_source) {
                    (Some(true), Some(source)) => format!("activado ({})", source.label()),
                    (Some(false), Some(source)) => format!("desactivado ({})", source.label()),
                    (Some(_), None) | (None, Some(_)) | (None, None) => "no activado".to_owned(),
                });
                let target = plugin_toggle_scope(&snapshot.paths, &plugin.mentioned_in);
                let mut buttons = vec![view.button(
                    appearance,
                    format!("{key}:toggle"),
                    &if plugin.enabled == Some(true) {
                        format!("Desactivar (en {})", target.label())
                    } else {
                        format!("Activar (en {})", target.label())
                    },
                    ClaudeCodePageAction::TogglePlugin(index),
                    ButtonVariant::Secondary,
                )];
                if let Some(path) = &plugin.install_path {
                    buttons.extend(view.file_buttons(appearance, &key, path, false));
                }
                rows.push(render_row(
                    appearance,
                    &plugin.id,
                    &badges,
                    plugin
                        .install_scope
                        .as_ref()
                        .map(|scope| format!("Instalado con scope {scope}")),
                    None,
                    buttons,
                ));
            }
            for (index, marketplace) in snapshot.marketplaces.iter().enumerate() {
                let key = format!("marketplace:{index}");
                let buttons = marketplace
                    .install_location
                    .as_ref()
                    .map(|path| view.file_buttons(appearance, &key, path, false))
                    .unwrap_or_default();
                rows.push(render_row(
                    appearance,
                    &marketplace.name,
                    &["marketplace".to_owned()],
                    Some(marketplace.source.clone()),
                    None,
                    buttons,
                ));
            }
        }
        view.render_section(
            appearance,
            "Plugins y marketplaces",
            Vec::new(),
            None,
            rows,
            "No hay plugins instalados.",
        )
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
        _app: &AppContext,
    ) -> Box<dyn Element> {
        let mut actions = Vec::new();
        let mut rows = Vec::new();
        if let Some(snapshot) = &view.snapshot {
            for scope in [
                SettingsScope::User,
                SettingsScope::Project,
                SettingsScope::Local,
            ] {
                if snapshot.paths.settings_path(scope).is_some() {
                    actions.push(view.button(
                        appearance,
                        format!("hook:new:{scope:?}"),
                        &format!("Nuevo hook ({})", scope.label()),
                        ClaudeCodePageAction::NewHook(scope),
                        ButtonVariant::Secondary,
                    ));
                }
            }
            for (index, hook) in snapshot.hooks.iter().enumerate() {
                let key = format!("hook:{index}");
                let mut badges = vec![hook.source.label(), hook.hook_type.clone()];
                if let Some(timeout) = hook.timeout {
                    badges.push(format!("{timeout}s"));
                }
                let title = match &hook.matcher {
                    Some(matcher) if !matcher.is_empty() => format!("{} · {matcher}", hook.event),
                    _ => hook.event.clone(),
                };
                let editable = matches!(hook.source, HookSource::Settings(scope) if scope.is_writable())
                    && hook.hook_type == "command";
                let mut buttons = Vec::new();
                if editable {
                    buttons.push(view.button(
                        appearance,
                        format!("{key}:edit"),
                        "Editar",
                        ClaudeCodePageAction::EditHook(index),
                        ButtonVariant::Secondary,
                    ));
                    buttons.extend(view.delete_buttons(
                        appearance,
                        &key,
                        DeleteTarget::Hook(index),
                    ));
                }
                buttons.extend(view.file_buttons(appearance, &key, &hook.path, true));
                rows.push(render_row(
                    appearance,
                    &title,
                    &badges,
                    hook.command.clone(),
                    None,
                    buttons,
                ));
            }
        }
        view.render_section(
            appearance,
            "Hooks",
            actions,
            Some(FormSection::Hooks),
            rows,
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
        _app: &AppContext,
    ) -> Box<dyn Element> {
        let mut rows = Vec::new();
        if let Some(snapshot) = &view.snapshot {
            for file in &snapshot.settings {
                let key = format!("settings:{:?}", file.scope);
                let state = if file.error.is_some() {
                    "inválido"
                } else if file.exists {
                    "existe"
                } else {
                    "no existe"
                };
                let mut buttons = Vec::new();
                if file.scope.is_writable() && file.error.is_none() {
                    buttons.push(view.button(
                        appearance,
                        format!("{key}:edit"),
                        "Editar valores comunes",
                        ClaudeCodePageAction::EditSettings(file.scope),
                        ButtonVariant::Secondary,
                    ));
                }
                buttons.extend(view.file_buttons(appearance, &key, &file.path, file.exists));
                let mut badges = vec![state.to_owned()];
                if !file.scope.is_writable() {
                    badges.push("solo lectura".to_owned());
                }
                rows.push(render_row(
                    appearance,
                    &format!(
                        "{} — {}",
                        file.scope.label(),
                        display_path(&file.path, snapshot.home_dir.as_deref())
                    ),
                    &badges,
                    None,
                    file.error.as_deref(),
                    buttons,
                ));
            }
            if !snapshot.attributions.is_empty() {
                let mut lines =
                    vec!["Valores efectivos (archivo que gana entre paréntesis):".to_owned()];
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
                rows.push(render_row(
                    appearance,
                    "Atribución",
                    &[],
                    Some(lines.join("\n")),
                    None,
                    Vec::new(),
                ));
            }
            if !snapshot.permission_rules.is_empty() {
                let lines = snapshot
                    .permission_rules
                    .iter()
                    .map(|rule| format!("{}: {} ({})", rule.kind, rule.rule, rule.scope.label()))
                    .collect::<Vec<_>>()
                    .join("\n");
                rows.push(render_row(
                    appearance,
                    "Reglas de permisos (se combinan entre archivos)",
                    &[],
                    Some(lines),
                    None,
                    Vec::new(),
                ));
            }
        }
        view.render_section(
            appearance,
            "Archivos de configuración",
            Vec::new(),
            Some(FormSection::Settings),
            rows,
            "No hay archivos de configuración.",
        )
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
        _app: &AppContext,
    ) -> Box<dyn Element> {
        let mut rows = Vec::new();
        if let Some(snapshot) = &view.snapshot {
            for (index, file) in snapshot.memory_files.iter().enumerate() {
                let key = format!("memory:{index}");
                rows.push(render_row(
                    appearance,
                    &format!(
                        "{} — {}",
                        file.label,
                        display_path(&file.path, snapshot.home_dir.as_deref())
                    ),
                    &[if file.exists { "existe" } else { "no existe" }.to_owned()],
                    None,
                    None,
                    view.file_buttons(appearance, &key, &file.path, file.exists),
                ));
            }
        }
        view.render_section(
            appearance,
            "Archivos de memoria",
            Vec::new(),
            None,
            rows,
            "No hay archivos de memoria.",
        )
    }
}
