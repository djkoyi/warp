//! Write operations on the Claude Code configuration. Every edit re-reads its target right
//! before writing, changes only the subtree it owns, backs the file up first and writes it
//! atomically (see [`super::safe_write`]).

use std::fs;
use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value};

use super::frontmatter::{MarkdownDoc, parse_markdown, render_markdown};
use super::paths::ClaudePaths;
use super::safe_write::{
    backup_and_remove, object_entry, read_json_object, update_json_file, write_with_backup,
};
use super::snapshot::{McpScope, MdKind};
use super::{SettingsScope, validate_name};

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

/// The settings keys the panel edits visually. Blank values remove the key.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommonSettingsUpdate {
    pub model: String,
    pub default_mode: String,
    pub output_style: String,
    pub status_line_command: String,
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub ask: Vec<String>,
    pub env: Map<String, Value>,
}

#[derive(Clone, Debug)]
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
    DeleteItem { path: PathBuf },
    UpsertMcpServer {
        scope: McpScope,
        original_name: Option<String>,
        name: String,
        entry: Map<String, Value>,
    },
    DeleteMcpServer { scope: McpScope, name: String },
    /// Approves or rejects a project `.mcp.json` server in the local settings file.
    SetMcpjsonServerEnabled { name: String, enabled: bool },
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
    SetSkillOverride {
        scope: SettingsScope,
        name: String,
        off: bool,
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
            ensure_within_writable_roots(paths, &path)?;
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
                if original_name.as_deref() != Some(name.as_str()) && servers.contains_key(&name)
                {
                    return Err(format!("Ya existe un servidor MCP llamado \"{name}\""));
                }
                if let Some(original) = &original_name {
                    servers.remove(original);
                }
                servers.insert(name.clone(), Value::Object(entry));
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
        ClaudeEdit::SetSkillOverride { scope, name, off } => {
            let file = writable_settings_path(paths, scope)?;
            update_json_file(&backups, &file, |root| {
                let overrides = object_entry(root, "skillOverrides")?;
                if off {
                    overrides.insert(name.clone(), Value::String("off".to_owned()));
                } else {
                    overrides.remove(&name);
                    if overrides.is_empty() {
                        root.remove("skillOverrides");
                    }
                }
                Ok(())
            })?;
            Ok(format!(
                "Skill \"{name}\" {}",
                if off { "desactivada" } else { "activada" }
            ))
        }
        ClaudeEdit::UpsertHook {
            scope,
            original,
            hook,
        } => {
            validate_hook(&hook)?;
            let file = writable_settings_path(paths, scope)?;
            update_json_file(&backups, &file, |root| upsert_hook(root, original.as_ref(), &hook))?;
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
            validate_common_settings(&update)?;
            let file = writable_settings_path(paths, scope)?;
            update_json_file(&backups, &file, |root| {
                apply_common_settings(root, &update)
            })?;
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
            ensure_within_writable_roots(paths, &path)?;
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
                    && path.parent().is_some_and(|dir| fs::symlink_metadata(dir).is_ok()))
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

/// Rejects paths outside the `skills`, `agents` and `commands` folders of the user config dir
/// and the project's `.claude` dir, so a stale or crafted path can never make the panel write
/// or delete anything else.
fn ensure_within_writable_roots(paths: &ClaudePaths, path: &Path) -> Result<(), String> {
    let roots: Vec<PathBuf> = std::iter::once(paths.config_dir.clone())
        .chain(paths.project_claude_dir())
        .flat_map(|base| {
            [MdKind::Skill, MdKind::Agent, MdKind::Command].map(|kind| base.join(kind.subdir()))
        })
        .collect();
    // Checked lexically first: skills installed by `npx skills` are symlinks into
    // `~/.agents/skills`, and editing them through the link is intended.
    let has_parent_dir = path
        .components()
        .any(|component| matches!(component, Component::ParentDir));
    if !has_parent_dir
        && roots
            .iter()
            .any(|root| path.starts_with(root) && path != root.as_path())
    {
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or_else(|| format!("Ruta no válida: {}", path.display()))?;
    let parent = fs::canonicalize(parent)
        .map_err(|err| format!("No se pudo resolver {}: {err}", path.display()))?;
    for root in &roots {
        if let Ok(root) = fs::canonicalize(root)
            && parent.starts_with(&root)
        {
            return Ok(());
        }
    }
    Err(format!(
        "{} está fuera de las carpetas de Claude Code editables",
        path.display()
    ))
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
        McpScope::Local(key) => {
            object_entry(object_entry(object_entry(root, "projects")?, key)?, "mcpServers")
        }
        McpScope::Plugin(_) => Err("Los servidores de plugins son de solo lectura".to_owned()),
    }
}

/// Builds an MCP server entry from form values, starting from the original entry so keys the
/// form does not know about (e.g. `oauth`, `timeout`) are kept.
pub fn build_mcp_entry(
    original: Option<&Map<String, Value>>,
    transport: &str,
    command: &str,
    args_text: &str,
    url: &str,
    env: Map<String, Value>,
    headers: Map<String, Value>,
) -> Result<Map<String, Value>, String> {
    let mut entry = original.cloned().unwrap_or_default();
    let transport = transport.trim();
    match transport {
        "stdio" => {
            let command = command.trim();
            if command.is_empty() {
                return Err("Un servidor stdio necesita un comando".to_owned());
            }
            entry.insert("type".to_owned(), Value::String("stdio".to_owned()));
            entry.insert("command".to_owned(), Value::String(command.to_owned()));
            let args: Vec<Value> = args_text
                .lines()
                .map(str::trim)
                .filter(|arg| !arg.is_empty())
                .map(|arg| Value::String(arg.to_owned()))
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
            let url = url.trim();
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return Err("La URL debe empezar por https:// o http://".to_owned());
            }
            entry.insert("type".to_owned(), Value::String(transport.to_owned()));
            entry.insert("url".to_owned(), Value::String(url.to_owned()));
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

fn set_mcpjson_server_enabled(paths: &ClaudePaths, name: &str, enabled: bool) -> Result<(), String> {
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

fn remove_hook(hooks: &mut Map<String, Value>, location: &HookLocation) -> Result<(), String> {
    locate_hook(hooks, location)?;
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
    Ok(())
}

fn upsert_hook(
    root: &mut Map<String, Value>,
    original: Option<&HookLocation>,
    spec: &HookSpec,
) -> Result<(), String> {
    let hooks = object_entry(root, "hooks")?;
    let matcher = normalized_matcher(spec.matcher.as_deref());
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
            hook.entry("type".to_owned())
                .or_insert_with(|| Value::String("command".to_owned()));
            hook.insert("command".to_owned(), Value::String(spec.command.clone()));
            set_timeout(hook, spec.timeout);
            return Ok(());
        }
        remove_hook(hooks, location)?;
    }

    let mut hook = Map::new();
    hook.insert("type".to_owned(), Value::String("command".to_owned()));
    hook.insert("command".to_owned(), Value::String(spec.command.clone()));
    set_timeout(&mut hook, spec.timeout);

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
    let mode = update.default_mode.trim();
    if !mode.is_empty() && !mode.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(format!("defaultMode no válido: \"{mode}\""));
    }
    for rule in update.allow.iter().chain(&update.deny).chain(&update.ask) {
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

fn set_or_remove_list(map: &mut Map<String, Value>, key: &str, values: &[String]) {
    if values.is_empty() {
        map.remove(key);
    } else {
        map.insert(
            key.to_owned(),
            Value::Array(
                values
                    .iter()
                    .map(|value| Value::String(value.trim().to_owned()))
                    .collect(),
            ),
        );
    }
}

fn apply_common_settings(
    root: &mut Map<String, Value>,
    update: &CommonSettingsUpdate,
) -> Result<(), String> {
    set_or_remove_string(root, "model", &update.model);
    set_or_remove_string(root, "outputStyle", &update.output_style);

    let permissions = object_entry(root, "permissions")?;
    set_or_remove_string(permissions, "defaultMode", &update.default_mode);
    set_or_remove_list(permissions, "allow", &update.allow);
    set_or_remove_list(permissions, "deny", &update.deny);
    set_or_remove_list(permissions, "ask", &update.ask);
    if permissions.is_empty() {
        root.remove("permissions");
    }

    let command = update.status_line_command.trim();
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

    set_or_remove_object(root, "env", update.env.clone());
    Ok(())
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
