//! Write operations on the Claude Code configuration. Every edit re-reads its target right
//! before writing, changes only the subtree it owns, backs the file up first and writes it
//! atomically (see [`super::safe_write`]).

use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value};

use super::frontmatter::{MarkdownDoc, parse_markdown, render_markdown};
use super::masking::{merge_masked_args, merge_masked_url};
use super::paths::ClaudePaths;
use super::safe_write::{
    backup_and_remove, object_entry, read_json_object, update_json_file, write_with_backup,
};
use super::snapshot::{MAX_SCAN_DEPTH, McpScope, MdKind};
use super::{SettingsScope, validate_name};

/// Deepest agent/command file accepted, in path components below its root: the folders the
/// snapshot scans plus the file itself.
const MAX_ITEM_DEPTH: usize = MAX_SCAN_DEPTH + 1;

/// Identifies a hook inside a settings file, plus the command it had when it was read so a
/// concurrent change to the file is detected instead of editing the wrong entry.
#[derive(Clone, Debug, PartialEq)]
pub struct HookLocation {
    pub event: String,
    pub group_index: usize,
    pub hook_index: usize,
    pub expected_command: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HookSpec {
    pub event: String,
    pub matcher: Option<String>,
    pub command: String,
    pub timeout: Option<u64>,
}

/// Rules to add to and remove from one permission list. Applied onto the list as it is on disk
/// at write time, so rules Claude Code added after the form was opened survive.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListDiff {
    pub add: Vec<String>,
    pub remove: Vec<String>,
}

impl ListDiff {
    pub fn between(initial: &[String], edited: &[String]) -> Self {
        Self {
            add: edited
                .iter()
                .filter(|rule| !initial.contains(rule))
                .cloned()
                .collect(),
            remove: initial
                .iter()
                .filter(|rule| !edited.contains(rule))
                .cloned()
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.add.is_empty() && self.remove.is_empty()
    }
}

/// The changes the "common settings" form makes. Only what the user changed is written; every
/// other key keeps whatever value the file has at write time.
#[derive(Clone, Default, PartialEq)]
pub struct CommonSettingsUpdate {
    /// `None` leaves the key untouched; a blank value removes it.
    pub model: Option<String>,
    /// `None` leaves the key untouched; a blank value removes it.
    pub default_mode: Option<String>,
    /// `None` leaves the key untouched; a blank value removes it.
    pub output_style: Option<String>,
    /// `None` leaves the key untouched; a blank value removes it.
    pub status_line_command: Option<String>,
    pub allow: ListDiff,
    pub deny: ListDiff,
    pub ask: ListDiff,
    /// New or changed `env` entries. Holds secrets.
    pub env_set: Map<String, Value>,
    pub env_remove: Vec<String>,
}

impl CommonSettingsUpdate {
    pub fn is_empty(&self) -> bool {
        self.model.is_none()
            && self.default_mode.is_none()
            && self.output_style.is_none()
            && self.status_line_command.is_none()
            && self.allow.is_empty()
            && self.deny.is_empty()
            && self.ask.is_empty()
            && self.env_set.is_empty()
            && self.env_remove.is_empty()
    }

    fn touches_permissions(&self) -> bool {
        self.default_mode.is_some()
            || !self.allow.is_empty()
            || !self.deny.is_empty()
            || !self.ask.is_empty()
    }
}

impl fmt::Debug for CommonSettingsUpdate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommonSettingsUpdate")
            .field("model", &self.model)
            .field("default_mode", &self.default_mode)
            .field("output_style", &self.output_style)
            .field("status_line_command", &self.status_line_command)
            .field("allow", &self.allow)
            .field("deny", &self.deny)
            .field("ask", &self.ask)
            .field("env_set_keys", &self.env_set.keys().collect::<Vec<_>>())
            .field("env_remove", &self.env_remove)
            .finish()
    }
}

#[derive(Clone)]
pub enum ClaudeEdit {
    /// Creates (when `original_path` is `None`) or updates a skill, subagent or command file.
    SaveMarkdown {
        kind: MdKind,
        project: bool,
        original_path: Option<PathBuf>,
        name: String,
        /// Frontmatter keys to set; blank values remove the key.
        fields: Vec<(String, String)>,
        body: String,
    },
    /// Deletes a skill directory, subagent or command file.
    DeleteItem {
        path: PathBuf,
    },
    UpsertMcpServer {
        scope: McpScope,
        original_name: Option<String>,
        name: String,
        entry: Map<String, Value>,
    },
    DeleteMcpServer {
        scope: McpScope,
        name: String,
    },
    /// Approves or rejects a project `.mcp.json` server in the local settings file.
    SetMcpjsonServerEnabled {
        name: String,
        enabled: bool,
    },
    /// Toggles a user/local server for one project via `.claude.json` `disabledMcpServers`.
    SetMcpServerDisabled {
        project_key: String,
        name: String,
        disabled: bool,
    },
    SetPluginEnabled {
        scope: SettingsScope,
        id: String,
        enabled: bool,
    },
    /// Turning a skill off writes `"off"` to the highest-precedence writable settings file that
    /// already has an override for it, else to `default_scope`. Turning it on removes the
    /// override from every writable file.
    SetSkillOverride {
        name: String,
        off: bool,
        default_scope: SettingsScope,
    },
    UpsertHook {
        scope: SettingsScope,
        original: Option<HookLocation>,
        hook: HookSpec,
    },
    RemoveHook {
        scope: SettingsScope,
        location: HookLocation,
    },
    SetCommonSettings {
        scope: SettingsScope,
        update: CommonSettingsUpdate,
    },
}

/// Secret-bearing payloads (MCP entries, env values, markdown bodies) are left out.
impl fmt::Debug for ClaudeEdit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClaudeEdit::SaveMarkdown {
                kind,
                project,
                original_path,
                name,
                ..
            } => f
                .debug_struct("SaveMarkdown")
                .field("kind", kind)
                .field("project", project)
                .field("original_path", original_path)
                .field("name", name)
                .finish_non_exhaustive(),
            ClaudeEdit::DeleteItem { path } => {
                f.debug_struct("DeleteItem").field("path", path).finish()
            }
            ClaudeEdit::UpsertMcpServer {
                scope,
                original_name,
                name,
                ..
            } => f
                .debug_struct("UpsertMcpServer")
                .field("scope", scope)
                .field("original_name", original_name)
                .field("name", name)
                .finish_non_exhaustive(),
            ClaudeEdit::DeleteMcpServer { scope, name } => f
                .debug_struct("DeleteMcpServer")
                .field("scope", scope)
                .field("name", name)
                .finish(),
            ClaudeEdit::SetMcpjsonServerEnabled { name, enabled } => f
                .debug_struct("SetMcpjsonServerEnabled")
                .field("name", name)
                .field("enabled", enabled)
                .finish(),
            ClaudeEdit::SetMcpServerDisabled {
                project_key,
                name,
                disabled,
            } => f
                .debug_struct("SetMcpServerDisabled")
                .field("project_key", project_key)
                .field("name", name)
                .field("disabled", disabled)
                .finish(),
            ClaudeEdit::SetPluginEnabled { scope, id, enabled } => f
                .debug_struct("SetPluginEnabled")
                .field("scope", scope)
                .field("id", id)
                .field("enabled", enabled)
                .finish(),
            ClaudeEdit::SetSkillOverride {
                name,
                off,
                default_scope,
            } => f
                .debug_struct("SetSkillOverride")
                .field("name", name)
                .field("off", off)
                .field("default_scope", default_scope)
                .finish(),
            ClaudeEdit::UpsertHook {
                scope,
                original,
                hook,
            } => f
                .debug_struct("UpsertHook")
                .field("scope", scope)
                .field("original", original)
                .field("hook", hook)
                .finish(),
            ClaudeEdit::RemoveHook { scope, location } => f
                .debug_struct("RemoveHook")
                .field("scope", scope)
                .field("location", location)
                .finish(),
            ClaudeEdit::SetCommonSettings { scope, update } => f
                .debug_struct("SetCommonSettings")
                .field("scope", scope)
                .field("update", update)
                .finish(),
        }
    }
}

impl ClaudeEdit {
    /// Whether running Claude Code sessions need a reload to pick the change up. Claude Code
    /// watches its settings files (hooks, permissions, model…), but MCP servers, plugins and
    /// skill/agent/command files are only read at startup or on `/reload-plugins`.
    pub fn needs_claude_reload(&self) -> bool {
        match self {
            ClaudeEdit::SaveMarkdown { .. }
            | ClaudeEdit::DeleteItem { .. }
            | ClaudeEdit::UpsertMcpServer { .. }
            | ClaudeEdit::DeleteMcpServer { .. }
            | ClaudeEdit::SetMcpjsonServerEnabled { .. }
            | ClaudeEdit::SetMcpServerDisabled { .. }
            | ClaudeEdit::SetPluginEnabled { .. }
            | ClaudeEdit::SetSkillOverride { .. } => true,
            ClaudeEdit::UpsertHook { .. }
            | ClaudeEdit::RemoveHook { .. }
            | ClaudeEdit::SetCommonSettings { .. } => false,
        }
    }
}

/// Applies one edit. Performs blocking I/O; call off the UI thread. Returns a user-facing
/// success message.
pub fn apply_edit(paths: &ClaudePaths, edit: ClaudeEdit) -> Result<String, String> {
    let backups = paths.backups_dir();
    match edit {
        ClaudeEdit::SaveMarkdown {
            kind,
            project,
            original_path,
            name,
            fields,
            body,
        } => save_markdown(paths, kind, project, original_path, &name, fields, body),
        ClaudeEdit::DeleteItem { path } => {
            ensure_within_writable_roots(paths, &path, PathPurpose::Delete)?;
            backup_and_remove(&backups, &path)
                .map_err(|err| format!("No se pudo eliminar {}: {err}", path.display()))?;
            Ok(format!("Eliminado {}", path.display()))
        }
        ClaudeEdit::UpsertMcpServer {
            scope,
            original_name,
            name,
            entry,
        } => {
            if original_name.as_deref() != Some(name.as_str()) {
                validate_name(&name)?;
            }
            let file = mcp_file(paths, &scope)?;
            update_json_file(&backups, &file, |root| {
                let servers = mcp_servers_object(root, &scope)?;
                if original_name.as_deref() != Some(name.as_str()) && servers.contains_key(&name) {
                    return Err(format!("Ya existe un servidor MCP llamado \"{name}\""));
                }
                if let Some(original) = &original_name {
                    servers.remove(original);
                }
                servers.insert(name.clone(), Value::Object(entry.clone()));
                Ok(())
            })?;
            Ok(format!("Servidor MCP \"{name}\" guardado"))
        }
        ClaudeEdit::DeleteMcpServer { scope, name } => {
            let file = mcp_file(paths, &scope)?;
            update_json_file(&backups, &file, |root| {
                let servers = mcp_servers_object(root, &scope)?;
                servers
                    .remove(&name)
                    .map(|_| ())
                    .ok_or_else(|| format!("El servidor \"{name}\" ya no existe; recarga"))
            })?;
            Ok(format!("Servidor MCP \"{name}\" eliminado"))
        }
        ClaudeEdit::SetMcpjsonServerEnabled { name, enabled } => {
            set_mcpjson_server_enabled(paths, &name, enabled)?;
            Ok(if enabled {
                format!("Servidor \"{name}\" aprobado")
            } else {
                format!("Servidor \"{name}\" desactivado")
            })
        }
        ClaudeEdit::SetMcpServerDisabled {
            project_key,
            name,
            disabled,
        } => {
            update_json_file(&backups, &paths.claude_json, |root| {
                let project = object_entry(object_entry(root, "projects")?, &project_key)?;
                if disabled {
                    list_insert(project, "disabledMcpServers", &name)
                } else {
                    list_remove(project, "disabledMcpServers", &name).map(|_| ())
                }
            })?;
            Ok(if disabled {
                format!("Servidor \"{name}\" desactivado en este proyecto")
            } else {
                format!("Servidor \"{name}\" activado en este proyecto")
            })
        }
        ClaudeEdit::SetPluginEnabled { scope, id, enabled } => {
            let file = writable_settings_path(paths, scope)?;
            update_json_file(&backups, &file, |root| {
                object_entry(root, "enabledPlugins")?.insert(id.clone(), Value::Bool(enabled));
                Ok(())
            })?;
            Ok(format!(
                "Plugin \"{id}\" {} en {}",
                if enabled { "activado" } else { "desactivado" },
                scope.label()
            ))
        }
        ClaudeEdit::SetSkillOverride {
            name,
            off,
            default_scope,
        } => set_skill_override(paths, &name, off, default_scope),
        ClaudeEdit::UpsertHook {
            scope,
            original,
            hook,
        } => {
            validate_hook(&hook)?;
            let file = writable_settings_path(paths, scope)?;
            update_json_file(&backups, &file, |root| {
                upsert_hook(root, original.as_ref(), &hook)
            })?;
            Ok(format!("Hook {} guardado", hook.event))
        }
        ClaudeEdit::RemoveHook { scope, location } => {
            let file = writable_settings_path(paths, scope)?;
            update_json_file(&backups, &file, |root| {
                let hooks = object_entry(root, "hooks")?;
                remove_hook(hooks, &location)?;
                if hooks.is_empty() {
                    root.remove("hooks");
                }
                Ok(())
            })?;
            Ok(format!("Hook {} eliminado", location.event))
        }
        ClaudeEdit::SetCommonSettings { scope, update } => {
            if update.is_empty() {
                return Ok("Sin cambios".to_owned());
            }
            validate_common_settings(&update)?;
            let file = writable_settings_path(paths, scope)?;
            update_json_file(&backups, &file, |root| apply_common_settings(root, &update))?;
            Ok(format!("Configuración de {} guardada", scope.label()))
        }
    }
}

fn save_markdown(
    paths: &ClaudePaths,
    kind: MdKind,
    project: bool,
    original_path: Option<PathBuf>,
    name: &str,
    fields: Vec<(String, String)>,
    body: String,
) -> Result<String, String> {
    let (path, mut doc) = match original_path {
        Some(path) => {
            ensure_within_writable_roots(paths, &path, PathPurpose::Edit)?;
            let text = fs::read_to_string(&path)
                .map_err(|err| format!("No se pudo leer {}: {err}", path.display()))?;
            let doc = parse_markdown(&text).map_err(|err| {
                format!("{err}. Corrige el archivo con \"Abrir archivo\" antes de editarlo aquí")
            })?;
            (path, doc)
        }
        None => {
            validate_name(name)?;
            let dir = paths
                .items_dir(project, kind.subdir())
                .ok_or_else(|| "No hay un proyecto activo".to_owned())?;
            let path = match kind {
                MdKind::Skill => dir.join(name).join("SKILL.md"),
                MdKind::Agent | MdKind::Command => dir.join(format!("{name}.md")),
            };
            if fs::symlink_metadata(&path).is_ok()
                || (kind == MdKind::Skill
                    && path
                        .parent()
                        .is_some_and(|dir| fs::symlink_metadata(dir).is_ok()))
            {
                return Err(format!("Ya existe {}", path.display()));
            }
            (path, MarkdownDoc::default())
        }
    };
    match kind {
        MdKind::Skill | MdKind::Agent => {
            if name.trim().is_empty() {
                return Err("El nombre no puede estar vacío".to_owned());
            }
            doc.set_string_if_changed("name", name);
        }
        MdKind::Command => {}
    }
    for (key, value) in fields {
        doc.set_string_if_changed(&key, &value);
    }
    if kind == MdKind::Skill && doc.get("description").is_none_or(|d| d.trim().is_empty()) {
        return Err("La skill necesita una descripción".to_owned());
    }
    doc.body = body;
    let text = render_markdown(&doc)?;
    write_with_backup(&paths.backups_dir(), &path, text.as_bytes())
        .map_err(|err| format!("No se pudo escribir {}: {err}", path.display()))?;
    Ok(format!("Guardado {}", path.display()))
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum PathPurpose {
    Edit,
    Delete,
}

/// Rejects anything but the exact item shapes inside the `skills`, `agents` and `commands`
/// folders of the user config dir and the project's `.claude` dir, so a stale or crafted path
/// can never make the panel write or delete anything else:
/// - skills: `<root>/<name>` (delete) or `<root>/<name>/SKILL.md` (edit);
/// - agents and commands: a `.md` file under `<root>`, possibly in subfolders (namespaced
///   commands such as `/frontend:review`).
fn ensure_within_writable_roots(
    paths: &ClaudePaths,
    path: &Path,
    purpose: PathPurpose,
) -> Result<(), String> {
    let outside = || {
        format!(
            "{} está fuera de las carpetas de Claude Code editables",
            path.display()
        )
    };
    if path.file_name().is_none()
        || !path.components().all(|component| {
            matches!(
                component,
                Component::Normal(_) | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(outside());
    }
    let roots = std::iter::once(paths.config_dir.clone())
        .chain(paths.project_claude_dir())
        .flat_map(|base| {
            [MdKind::Skill, MdKind::Agent, MdKind::Command]
                .map(|kind| (kind, base.join(kind.subdir())))
        });
    for (kind, root) in roots {
        let canonical_root = fs::canonicalize(&root).ok();
        let Some(relative) = path.strip_prefix(&root).ok().or_else(|| {
            canonical_root
                .as_deref()
                .and_then(|canonical| path.strip_prefix(canonical).ok())
        }) else {
            continue;
        };
        let parts: Vec<&std::ffi::OsStr> =
            relative.components().map(Component::as_os_str).collect();
        let shape_ok = match kind {
            MdKind::Skill => match (purpose, parts.as_slice()) {
                (PathPurpose::Delete, [_name]) => true,
                (PathPurpose::Edit, [_name, file]) => *file == "SKILL.md",
                (PathPurpose::Delete | PathPurpose::Edit, _) => false,
            },
            MdKind::Agent | MdKind::Command => {
                !parts.is_empty()
                    && parts.len() <= MAX_ITEM_DEPTH
                    && Path::new(parts[parts.len() - 1])
                        .extension()
                        .is_some_and(|ext| ext == "md")
            }
        };
        if !shape_ok {
            return Err(outside());
        }
        // Skills installed by `npx skills` are symlinked directories pointing elsewhere: editing
        // through the link is intended, and deleting removes only the link. Agents and commands
        // have no such convention, so a symlinked subfolder or file must not lead outside.
        let Some(canonical_root) = canonical_root else {
            return Err(outside());
        };
        let parent = path.parent().ok_or_else(outside)?;
        let check = match (kind, purpose) {
            (MdKind::Skill, PathPurpose::Delete) => Some(parent.to_path_buf()),
            (MdKind::Skill, PathPurpose::Edit) => parent.parent().map(Path::to_path_buf),
            (MdKind::Agent | MdKind::Command, PathPurpose::Delete) => Some(parent.to_path_buf()),
            (MdKind::Agent | MdKind::Command, PathPurpose::Edit) => Some(path.to_path_buf()),
        };
        let resolved = check
            .map(fs::canonicalize)
            .transpose()
            .map_err(|err| format!("No se pudo resolver {}: {err}", path.display()))?
            .ok_or_else(outside)?;
        if resolved.starts_with(&canonical_root) {
            return Ok(());
        }
        return Err(outside());
    }
    Err(outside())
}

fn writable_settings_path(paths: &ClaudePaths, scope: SettingsScope) -> Result<PathBuf, String> {
    if !scope.is_writable() {
        return Err("La configuración administrada es de solo lectura".to_owned());
    }
    paths
        .settings_path(scope)
        .ok_or_else(|| "No hay un proyecto activo".to_owned())
}

fn mcp_file(paths: &ClaudePaths, scope: &McpScope) -> Result<PathBuf, String> {
    match scope {
        McpScope::User | McpScope::Local(_) => Ok(paths.claude_json.clone()),
        McpScope::Project => paths
            .project_mcp_json()
            .ok_or_else(|| "No hay un proyecto activo".to_owned()),
        McpScope::Plugin(_) => Err("Los servidores de plugins son de solo lectura".to_owned()),
    }
}

fn mcp_servers_object<'a>(
    root: &'a mut Map<String, Value>,
    scope: &McpScope,
) -> Result<&'a mut Map<String, Value>, String> {
    match scope {
        McpScope::User | McpScope::Project => object_entry(root, "mcpServers"),
        McpScope::Local(key) => object_entry(
            object_entry(object_entry(root, "projects")?, key)?,
            "mcpServers",
        ),
        McpScope::Plugin(_) => Err("Los servidores de plugins son de solo lectura".to_owned()),
    }
}

/// The form transport (`stdio`, `http` or `sse`) for an MCP entry, or `None` when its `type`
/// is one the form cannot edit without losing information (e.g. `ws`).
pub fn mcp_form_transport(entry: &Map<String, Value>) -> Option<&'static str> {
    match entry.get("type") {
        None if entry.contains_key("url") && !entry.contains_key("command") => Some("http"),
        None => Some("stdio"),
        Some(Value::String(kind)) => match kind.as_str() {
            "stdio" => Some("stdio"),
            "http" | "streamable-http" => Some("http"),
            "sse" => Some("sse"),
            _ => None,
        },
        Some(
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) | Value::Object(_),
        ) => None,
    }
}

/// Builds an MCP server entry from form values, starting from the original entry so keys the
/// form does not know about (e.g. `oauth`, `timeout`) are kept. `args_text` and `url` may hold
/// the masked values the form showed (see [`super::masking`]); those keep the stored secrets.
pub fn build_mcp_entry(
    original: Option<&Map<String, Value>>,
    transport: &str,
    command: &str,
    args_text: &str,
    url: &str,
    env: Map<String, Value>,
    headers: Map<String, Value>,
) -> Result<Map<String, Value>, String> {
    let original_transport = match original {
        Some(original) => Some(mcp_form_transport(original).ok_or_else(|| {
            "Este tipo de servidor MCP no se puede editar aquí; usa \"Abrir archivo\"".to_owned()
        })?),
        None => None,
    };
    let mut entry = original.cloned().unwrap_or_default();
    let transport = transport.trim();
    // An unchanged transport keeps the stored `type` verbatim (absent, or e.g.
    // `streamable-http`), so saving does not rewrite what the user did not touch.
    let set_type = |entry: &mut Map<String, Value>| {
        if original_transport != Some(transport) {
            entry.insert("type".to_owned(), Value::String(transport.to_owned()));
        }
    };
    match transport {
        "stdio" => {
            let command = command.trim();
            if command.is_empty() {
                return Err("Un servidor stdio necesita un comando".to_owned());
            }
            set_type(&mut entry);
            entry.insert("command".to_owned(), Value::String(command.to_owned()));
            let original_args = original
                .and_then(|original| original.get("args"))
                .and_then(Value::as_array)
                .map(|args| {
                    args.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let args: Vec<Value> = merge_masked_args(&original_args, args_text)?
                .into_iter()
                .map(Value::String)
                .collect();
            if args.is_empty() {
                entry.remove("args");
            } else {
                entry.insert("args".to_owned(), Value::Array(args));
            }
            set_or_remove_object(&mut entry, "env", env);
            entry.remove("url");
            entry.remove("headers");
        }
        "http" | "sse" => {
            let original_url = original
                .and_then(|original| original.get("url"))
                .and_then(Value::as_str);
            let url = merge_masked_url(original_url, url)?;
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return Err("La URL debe empezar por https:// o http://".to_owned());
            }
            set_type(&mut entry);
            entry.insert("url".to_owned(), Value::String(url));
            set_or_remove_object(&mut entry, "headers", headers);
            entry.remove("command");
            entry.remove("args");
            entry.remove("env");
        }
        other => return Err(format!("Tipo de servidor no soportado: \"{other}\"")),
    }
    Ok(entry)
}

fn set_or_remove_object(entry: &mut Map<String, Value>, key: &str, value: Map<String, Value>) {
    if value.is_empty() {
        entry.remove(key);
    } else {
        entry.insert(key.to_owned(), Value::Object(value));
    }
}

fn list_insert(map: &mut Map<String, Value>, key: &str, item: &str) -> Result<(), String> {
    let list = map
        .entry(key.to_owned())
        .or_insert_with(|| Value::Array(Vec::new()));
    let Value::Array(items) = list else {
        return Err(format!("La clave \"{key}\" no es una lista"));
    };
    if !items.iter().any(|value| value.as_str() == Some(item)) {
        items.push(Value::String(item.to_owned()));
    }
    Ok(())
}

/// Returns whether `item` was present.
fn list_remove(map: &mut Map<String, Value>, key: &str, item: &str) -> Result<bool, String> {
    let Some(list) = map.get_mut(key) else {
        return Ok(false);
    };
    let Value::Array(items) = list else {
        return Err(format!("La clave \"{key}\" no es una lista"));
    };
    let before = items.len();
    items.retain(|value| value.as_str() != Some(item));
    Ok(items.len() != before)
}

fn list_contains(map: &Map<String, Value>, key: &str, item: &str) -> bool {
    map.get(key)
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().any(|value| value.as_str() == Some(item)))
}

fn set_mcpjson_server_enabled(
    paths: &ClaudePaths,
    name: &str,
    enabled: bool,
) -> Result<(), String> {
    let backups = paths.backups_dir();
    let local = writable_settings_path(paths, SettingsScope::Local)?;
    if enabled {
        // A rejection in any settings file wins, so it has to go everywhere it appears.
        for scope in [SettingsScope::User, SettingsScope::Project] {
            let Some(path) = paths.settings_path(scope) else {
                continue;
            };
            if list_contains(&read_json_object(&path)?, "disabledMcpjsonServers", name) {
                update_json_file(&backups, &path, |root| {
                    list_remove(root, "disabledMcpjsonServers", name).map(|_| ())
                })?;
            }
        }
    }
    update_json_file(&backups, &local, |root| {
        if enabled {
            list_remove(root, "disabledMcpjsonServers", name)?;
            list_insert(root, "enabledMcpjsonServers", name)
        } else {
            list_remove(root, "enabledMcpjsonServers", name)?;
            list_insert(root, "disabledMcpjsonServers", name)
        }
    })
}

fn validate_hook(hook: &HookSpec) -> Result<(), String> {
    if hook.event.is_empty() || !hook.event.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("El evento del hook debe ser un nombre como PreToolUse".to_owned());
    }
    if hook.command.trim().is_empty() {
        return Err("El hook necesita un comando".to_owned());
    }
    Ok(())
}

fn hook_command(hook: &Value) -> Option<String> {
    hook.get("command")
        .or_else(|| hook.get("prompt"))
        .or_else(|| hook.get("url"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn normalized_matcher(matcher: Option<&str>) -> Option<&str> {
    matcher.map(str::trim).filter(|matcher| !matcher.is_empty())
}

/// Finds the hook object at `location`, checking it still has the command it was read with.
fn locate_hook<'a>(
    hooks: &'a mut Map<String, Value>,
    location: &HookLocation,
) -> Result<&'a mut Value, String> {
    let stale = || "El archivo cambió desde que se leyó; recarga e inténtalo de nuevo".to_owned();
    let hook = hooks
        .get_mut(&location.event)
        .and_then(Value::as_array_mut)
        .and_then(|groups| groups.get_mut(location.group_index))
        .and_then(|group| group.get_mut("hooks"))
        .and_then(Value::as_array_mut)
        .and_then(|list| list.get_mut(location.hook_index))
        .ok_or_else(stale)?;
    if hook_command(hook) != location.expected_command {
        return Err(stale());
    }
    Ok(hook)
}

/// Removes the hook at `location` and returns it, with every field it had.
fn remove_hook(hooks: &mut Map<String, Value>, location: &HookLocation) -> Result<Value, String> {
    let removed = locate_hook(hooks, location)?.take();
    let mut remove_event = false;
    if let Some(groups) = hooks.get_mut(&location.event).and_then(Value::as_array_mut) {
        let mut remove_group = false;
        if let Some(list) = groups
            .get_mut(location.group_index)
            .and_then(|group| group.get_mut("hooks"))
            .and_then(Value::as_array_mut)
        {
            list.remove(location.hook_index);
            remove_group = list.is_empty();
        }
        if remove_group {
            groups.remove(location.group_index);
        }
        remove_event = groups.is_empty();
    }
    if remove_event {
        hooks.remove(&location.event);
    }
    Ok(removed)
}

fn upsert_hook(
    root: &mut Map<String, Value>,
    original: Option<&HookLocation>,
    spec: &HookSpec,
) -> Result<(), String> {
    let hooks = object_entry(root, "hooks")?;
    let matcher = normalized_matcher(spec.matcher.as_deref());
    let mut hook = Map::new();
    if let Some(location) = original {
        let same_group = location.event == spec.event
            && hooks
                .get(&location.event)
                .and_then(|groups| groups.get(location.group_index))
                .map(|group| normalized_matcher(group.get("matcher").and_then(Value::as_str)))
                == Some(matcher);
        if same_group {
            let hook = locate_hook(hooks, location)?;
            let Value::Object(hook) = hook else {
                return Err("El hook no es un objeto JSON".to_owned());
            };
            apply_hook_spec(hook, spec);
            return Ok(());
        }
        // Moving keeps the fields the form does not show (`if`, `async`, `statusMessage`…).
        hook = match remove_hook(hooks, location)? {
            Value::Object(hook) => hook,
            Value::Null
            | Value::Bool(_)
            | Value::Number(_)
            | Value::String(_)
            | Value::Array(_) => {
                return Err("El hook no es un objeto JSON".to_owned());
            }
        };
    }
    apply_hook_spec(&mut hook, spec);

    let groups = hooks
        .entry(spec.event.clone())
        .or_insert_with(|| Value::Array(Vec::new()));
    let Value::Array(groups) = groups else {
        return Err(format!("hooks.{} no es una lista", spec.event));
    };
    let existing_group = groups.iter_mut().find(|group| {
        normalized_matcher(group.get("matcher").and_then(Value::as_str)) == matcher
            && group.get("hooks").is_some_and(Value::is_array)
    });
    match existing_group {
        Some(group) => {
            if let Some(list) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                list.push(Value::Object(hook));
            }
        }
        None => {
            let mut group = Map::new();
            if let Some(matcher) = matcher {
                group.insert("matcher".to_owned(), Value::String(matcher.to_owned()));
            }
            group.insert("hooks".to_owned(), Value::Array(vec![Value::Object(hook)]));
            groups.push(Value::Object(group));
        }
    }
    Ok(())
}

fn apply_hook_spec(hook: &mut Map<String, Value>, spec: &HookSpec) {
    hook.entry("type".to_owned())
        .or_insert_with(|| Value::String("command".to_owned()));
    hook.insert("command".to_owned(), Value::String(spec.command.clone()));
    set_timeout(hook, spec.timeout);
}

fn set_timeout(hook: &mut Map<String, Value>, timeout: Option<u64>) {
    match timeout {
        Some(timeout) => {
            hook.insert("timeout".to_owned(), Value::from(timeout));
        }
        None => {
            hook.remove("timeout");
        }
    }
}

fn validate_common_settings(update: &CommonSettingsUpdate) -> Result<(), String> {
    if let Some(mode) = &update.default_mode {
        let mode = mode.trim();
        if !mode.is_empty() && !mode.chars().all(|c| c.is_ascii_alphabetic()) {
            return Err(format!("defaultMode no válido: \"{mode}\""));
        }
    }
    for rule in update
        .allow
        .add
        .iter()
        .chain(&update.deny.add)
        .chain(&update.ask.add)
    {
        if rule.trim().is_empty() {
            return Err("Las reglas de permisos no pueden estar vacías".to_owned());
        }
    }
    Ok(())
}

fn set_or_remove_string(map: &mut Map<String, Value>, key: &str, value: &str) {
    let value = value.trim();
    if value.is_empty() {
        map.remove(key);
    } else {
        map.insert(key.to_owned(), Value::String(value.to_owned()));
    }
}

/// Applies `diff` to the list at `key`. Entries that are not strings are kept untouched.
fn apply_list_diff(map: &mut Map<String, Value>, key: &str, diff: &ListDiff) -> Result<(), String> {
    if diff.is_empty() {
        return Ok(());
    }
    for rule in &diff.remove {
        list_remove(map, key, rule)?;
    }
    for rule in &diff.add {
        list_insert(map, key, rule.trim())?;
    }
    if map
        .get(key)
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
    {
        map.remove(key);
    }
    Ok(())
}

fn apply_common_settings(
    root: &mut Map<String, Value>,
    update: &CommonSettingsUpdate,
) -> Result<(), String> {
    if let Some(model) = &update.model {
        set_or_remove_string(root, "model", model);
    }
    if let Some(output_style) = &update.output_style {
        set_or_remove_string(root, "outputStyle", output_style);
    }

    if update.touches_permissions() {
        let permissions = object_entry(root, "permissions")?;
        if let Some(mode) = &update.default_mode {
            set_or_remove_string(permissions, "defaultMode", mode);
        }
        apply_list_diff(permissions, "allow", &update.allow)?;
        apply_list_diff(permissions, "deny", &update.deny)?;
        apply_list_diff(permissions, "ask", &update.ask)?;
        if permissions.is_empty() {
            root.remove("permissions");
        }
    }

    if let Some(command) = &update.status_line_command {
        let command = command.trim();
        if command.is_empty() {
            root.remove("statusLine");
        } else {
            let status_line = root
                .entry("statusLine".to_owned())
                .or_insert_with(|| Value::Object(Map::new()));
            if !status_line.is_object() {
                *status_line = Value::Object(Map::new());
            }
            if let Value::Object(status_line) = status_line {
                status_line.insert("type".to_owned(), Value::String("command".to_owned()));
                status_line.insert("command".to_owned(), Value::String(command.to_owned()));
            }
        }
    }

    if !update.env_set.is_empty() || !update.env_remove.is_empty() {
        let env = object_entry(root, "env")?;
        for key in &update.env_remove {
            env.remove(key);
        }
        for (key, value) in &update.env_set {
            env.insert(key.clone(), value.clone());
        }
        if env.is_empty() {
            root.remove("env");
        }
    }
    Ok(())
}

/// Writable settings scopes, highest precedence first.
const WRITABLE_SCOPES_BY_PRECEDENCE: [SettingsScope; 3] = [
    SettingsScope::Local,
    SettingsScope::Project,
    SettingsScope::User,
];

fn skill_override_in(root: &Map<String, Value>, name: &str) -> Option<Value> {
    root.get("skillOverrides")
        .and_then(Value::as_object)
        .and_then(|overrides| overrides.get(name))
        .cloned()
}

fn set_skill_override(
    paths: &ClaudePaths,
    name: &str,
    off: bool,
    default_scope: SettingsScope,
) -> Result<String, String> {
    let backups = paths.backups_dir();
    let mut defining = Vec::new();
    for scope in WRITABLE_SCOPES_BY_PRECEDENCE {
        let Some(path) = paths.settings_path(scope) else {
            continue;
        };
        if skill_override_in(&read_json_object(&path)?, name).is_some() {
            defining.push((scope, path));
        }
    }
    if off {
        let path = match defining.into_iter().next() {
            Some((_, path)) => path,
            None => writable_settings_path(paths, default_scope)?,
        };
        update_json_file(&backups, &path, |root| {
            object_entry(root, "skillOverrides")?
                .insert(name.to_owned(), Value::String("off".to_owned()));
            Ok(())
        })?;
        return Ok(format!("Skill \"{name}\" desactivada"));
    }

    for (_, path) in &defining {
        update_json_file(&backups, path, |root| {
            let overrides = object_entry(root, "skillOverrides")?;
            overrides.remove(name);
            if overrides.is_empty() {
                root.remove("skillOverrides");
            }
            Ok(())
        })?;
    }
    // Unreadable managed settings are not an error here: the snapshot already reports them.
    let managed = paths
        .settings_path(SettingsScope::Managed)
        .and_then(|path| read_json_object(&path).ok())
        .and_then(|root| skill_override_in(&root, name));
    Ok(match managed {
        Some(state) => format!(
            "Skill \"{name}\" activada en tus archivos, pero la configuración administrada la \
             mantiene en {state}"
        ),
        None => format!("Skill \"{name}\" activada"),
    })
}

/// Which settings file a plugin toggle should go to so it actually takes effect: the local
/// file when a project-level file already mentions the plugin, otherwise the user file.
pub fn plugin_toggle_scope(paths: &ClaudePaths, mentioned_in: &[SettingsScope]) -> SettingsScope {
    let project_mentions = mentioned_in
        .iter()
        .any(|scope| matches!(scope, SettingsScope::Project | SettingsScope::Local));
    if project_mentions && paths.project.is_some() {
        SettingsScope::Local
    } else {
        SettingsScope::User
    }
}

#[cfg(test)]
#[path = "edits_tests.rs"]
mod tests;
