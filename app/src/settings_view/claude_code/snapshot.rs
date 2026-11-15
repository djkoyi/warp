//! A read-only, point-in-time view of every Claude Code configuration source. Loading never
//! fails as a whole: unreadable or invalid files are reported per item.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::frontmatter::{MarkdownDoc, parse_markdown};
use super::masking::{mask_args, mask_url};
use super::paths::ClaudePaths;
use super::{ItemOrigin, SettingsScope};

/// Directory names more than this deep below a commands/agents root are not scanned.
pub const MAX_SCAN_DEPTH: usize = 4;
/// Files larger than this are not parsed (they are still listed).
const MAX_MARKDOWN_BYTES: u64 = 512 * 1024;

#[derive(Clone)]
pub struct SettingsFileInfo {
    pub scope: SettingsScope,
    pub path: PathBuf,
    pub exists: bool,
    /// The parsed file. May hold secrets (`env`, `apiKeyHelper`…).
    pub value: Option<Map<String, Value>>,
    pub error: Option<String>,
}

impl fmt::Debug for SettingsFileInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SettingsFileInfo")
            .field("scope", &self.scope)
            .field("path", &self.path)
            .field("exists", &self.exists)
            .field(
                "value_keys",
                &self
                    .value
                    .as_ref()
                    .map(|value| value.keys().collect::<Vec<_>>()),
            )
            .field("error", &self.error)
            .finish()
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum MdKind {
    Skill,
    Agent,
    Command,
}

impl MdKind {
    pub fn subdir(self) -> &'static str {
        match self {
            MdKind::Skill => "skills",
            MdKind::Agent => "agents",
            MdKind::Command => "commands",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MdKind::Skill => "skill",
            MdKind::Agent => "subagente",
            MdKind::Command => "comando",
        }
    }
}

/// A skill (`<dir>/SKILL.md`), subagent or slash command markdown file.
#[derive(Clone, Debug)]
pub struct MdItem {
    pub kind: MdKind,
    pub name: String,
    pub origin: ItemOrigin,
    pub path: PathBuf,
    pub doc: Option<MarkdownDoc>,
    pub error: Option<String>,
    /// For skills: the effective `skillOverrides` state, if any settings file sets one.
    pub override_state: Option<String>,
}

impl MdItem {
    pub fn description(&self) -> Option<String> {
        self.doc.as_ref().and_then(|doc| doc.get("description"))
    }

    /// The path to delete: the skill directory for skills, the file otherwise.
    pub fn removal_path(&self) -> PathBuf {
        match self.kind {
            MdKind::Skill => self
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.path.clone()),
            MdKind::Agent | MdKind::Command => self.path.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum McpScope {
    /// Top-level `mcpServers` in `.claude.json`.
    User,
    /// `projects[<path>].mcpServers` in `.claude.json`; the payload is the project key.
    Local(String),
    /// The project's `.mcp.json`.
    Project,
    Plugin(String),
}

impl McpScope {
    pub fn label(&self) -> String {
        match self {
            McpScope::User => "Usuario".to_owned(),
            McpScope::Local(_) => "Local".to_owned(),
            McpScope::Project => "Proyecto (.mcp.json)".to_owned(),
            McpScope::Plugin(id) => format!("Plugin {id}"),
        }
    }

    pub fn is_read_only(&self) -> bool {
        match self {
            McpScope::User | McpScope::Local(_) | McpScope::Project => false,
            McpScope::Plugin(_) => true,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum McpState {
    Enabled,
    Disabled,
    /// A `.mcp.json` server no settings file has approved or rejected yet.
    PendingApproval,
}

#[derive(Clone)]
pub struct McpServer {
    pub name: String,
    pub scope: McpScope,
    pub path: PathBuf,
    /// `stdio`, `http`, `sse`, or whatever `type` says.
    pub transport: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub env_keys: Vec<String>,
    pub header_keys: Vec<String>,
    /// The full entry. Holds secrets: never log or display it.
    pub raw: Map<String, Value>,
    pub state: McpState,
}

impl McpServer {
    /// The URL or command line, with likely secrets masked.
    pub fn summary(&self) -> String {
        match (&self.url, &self.command) {
            (Some(url), _) => mask_url(url),
            (None, Some(command)) if self.args.is_empty() => command.clone(),
            (None, Some(command)) => format!("{command} {}", mask_args(&self.args).join(" ")),
            (None, None) => String::new(),
        }
    }
}

impl fmt::Debug for McpServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpServer")
            .field("name", &self.name)
            .field("scope", &self.scope)
            .field("path", &self.path)
            .field("transport", &self.transport)
            .field("summary", &self.summary())
            .field("env_keys", &self.env_keys)
            .field("header_keys", &self.header_keys)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub struct PluginInfo {
    /// `name@marketplace`.
    pub id: String,
    pub installed: bool,
    pub version: Option<String>,
    pub install_path: Option<PathBuf>,
    pub install_scope: Option<String>,
    /// The effective `enabledPlugins` value, if any settings file mentions the plugin.
    pub enabled: Option<bool>,
    pub enabled_source: Option<SettingsScope>,
    /// Every settings scope that mentions the plugin.
    pub mentioned_in: Vec<SettingsScope>,
}

#[derive(Clone, Debug)]
pub struct MarketplaceInfo {
    pub name: String,
    pub source: String,
    pub install_location: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookSource {
    Settings(SettingsScope),
    Plugin(String),
}

impl HookSource {
    pub fn label(&self) -> String {
        match self {
            HookSource::Settings(scope) => scope.label().to_owned(),
            HookSource::Plugin(id) => format!("Plugin {id}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct HookItem {
    pub source: HookSource,
    pub path: PathBuf,
    pub event: String,
    pub group_index: usize,
    pub hook_index: usize,
    pub matcher: Option<String>,
    pub hook_type: String,
    pub command: Option<String>,
    pub timeout: Option<u64>,
}

/// Which settings files set a top-level key, and which one wins.
#[derive(Clone, Debug)]
pub struct Attribution {
    pub key: String,
    pub entries: Vec<(SettingsScope, String)>,
    pub effective: SettingsScope,
}

#[derive(Clone, Debug)]
pub struct PermissionRule {
    /// `allow`, `deny` or `ask`.
    pub kind: &'static str,
    pub rule: String,
    pub scope: SettingsScope,
}

#[derive(Clone, Debug)]
pub struct MemoryFile {
    pub label: String,
    pub path: PathBuf,
    pub exists: bool,
}

#[derive(Clone, Debug)]
pub struct ClaudeSnapshot {
    pub paths: ClaudePaths,
    /// Cached so rendering can shorten paths to `~/…` without querying the environment.
    pub home_dir: Option<PathBuf>,
    pub settings: Vec<SettingsFileInfo>,
    pub claude_json_error: Option<String>,
    /// The key under `projects` in `.claude.json` that holds the current project, if any.
    pub claude_json_project_key: Option<String>,
    pub skills: Vec<MdItem>,
    pub agents: Vec<MdItem>,
    pub commands: Vec<MdItem>,
    pub mcp_servers: Vec<McpServer>,
    pub plugins: Vec<PluginInfo>,
    pub marketplaces: Vec<MarketplaceInfo>,
    pub hooks: Vec<HookItem>,
    pub attributions: Vec<Attribution>,
    pub permission_rules: Vec<PermissionRule>,
    pub memory_files: Vec<MemoryFile>,
    /// Non-fatal problems with files that have no item of their own to show them on.
    pub warnings: Vec<String>,
}

impl ClaudeSnapshot {
    /// The `.claude.json` project key to write per-project state under.
    pub fn project_key(&self) -> Option<String> {
        self.claude_json_project_key
            .clone()
            .or_else(|| self.paths.project_keys().into_iter().next())
    }

    pub fn settings_file(&self, scope: SettingsScope) -> Option<&SettingsFileInfo> {
        self.settings.iter().find(|file| file.scope == scope)
    }

    pub fn settings_value(&self, scope: SettingsScope) -> Option<&Map<String, Value>> {
        self.settings_file(scope)?.value.as_ref()
    }

    pub fn items(&self, kind: MdKind) -> &[MdItem] {
        match kind {
            MdKind::Skill => &self.skills,
            MdKind::Agent => &self.agents,
            MdKind::Command => &self.commands,
        }
    }
}

/// Reads every source. Performs blocking I/O; call off the UI thread.
pub fn load_snapshot(paths: &ClaudePaths) -> ClaudeSnapshot {
    let mut warnings = Vec::new();
    let settings = load_settings_files(paths);
    let settings_in_order: Vec<(SettingsScope, &Map<String, Value>)> = settings
        .iter()
        .filter_map(|file| file.value.as_ref().map(|value| (file.scope, value)))
        .collect();

    let (claude_json, claude_json_error) = match fs::read_to_string(&paths.claude_json) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(map)) => (map, None),
            Ok(_) => (Map::new(), Some("no contiene un objeto JSON".to_owned())),
            Err(err) => (Map::new(), Some(format!("JSON inválido: {err}"))),
        },
        Err(err) if err.kind() == io::ErrorKind::NotFound => (Map::new(), None),
        Err(err) => (Map::new(), Some(err.to_string())),
    };
    let project_entry = paths.project_keys().into_iter().find_map(|key| {
        claude_json
            .get("projects")
            .and_then(|projects| projects.get(&key))
            .and_then(Value::as_object)
            .map(|entry| (key, entry.clone()))
    });

    let skill_overrides = merged_string_map(&settings_in_order, "skillOverrides");
    let plugins = load_plugins(paths, &settings_in_order, &mut warnings);
    let marketplaces = load_marketplaces(paths, &mut warnings);

    let mut skills = Vec::new();
    let mut agents = Vec::new();
    let mut commands = Vec::new();
    for (origin, base) in writable_item_roots(paths) {
        skills.extend(scan_skills(&base.join("skills"), &origin));
        agents.extend(scan_markdown_tree(
            &base.join("agents"),
            MdKind::Agent,
            &origin,
        ));
        commands.extend(scan_markdown_tree(
            &base.join("commands"),
            MdKind::Command,
            &origin,
        ));
    }
    for plugin in &plugins {
        let Some(root) = plugin.install_path.as_ref().filter(|_| plugin.installed) else {
            continue;
        };
        let origin = ItemOrigin::Plugin(plugin.id.clone());
        skills.extend(scan_skills(&root.join("skills"), &origin));
        agents.extend(scan_markdown_tree(
            &root.join("agents"),
            MdKind::Agent,
            &origin,
        ));
        commands.extend(scan_markdown_tree(
            &root.join("commands"),
            MdKind::Command,
            &origin,
        ));
    }
    for skill in &mut skills {
        if !matches!(skill.origin, ItemOrigin::Plugin(_)) {
            skill.override_state = skill_overrides.get(&skill.name).cloned();
        }
    }

    let mcp_servers = load_mcp_servers(
        paths,
        &claude_json,
        project_entry.as_ref(),
        &settings_in_order,
        &plugins,
        &mut warnings,
    );

    let mut hooks = Vec::new();
    for file in &settings {
        if let Some(value) = &file.value {
            hooks.extend(parse_hooks(
                value,
                HookSource::Settings(file.scope),
                &file.path,
            ));
        }
    }
    for plugin in plugins.iter().filter(|plugin| plugin.installed) {
        let Some(root) = &plugin.install_path else {
            continue;
        };
        let hooks_path = root.join("hooks").join("hooks.json");
        match read_optional_json_object(&hooks_path) {
            Ok(Some(value)) => hooks.extend(parse_hooks(
                &value,
                HookSource::Plugin(plugin.id.clone()),
                &hooks_path,
            )),
            Ok(None) => {}
            Err(err) => warnings.push(format!("{}: {err}", hooks_path.display())),
        }
    }

    let memory_files = paths
        .memory_files()
        .into_iter()
        .map(|(label, path)| MemoryFile {
            exists: path.is_file(),
            label,
            path,
        })
        .collect();

    ClaudeSnapshot {
        attributions: attributions(&settings_in_order),
        permission_rules: permission_rules(&settings_in_order),
        paths: paths.clone(),
        home_dir: dirs::home_dir(),
        settings,
        claude_json_error,
        claude_json_project_key: project_entry.as_ref().map(|(key, _)| key.clone()),
        skills,
        agents,
        commands,
        mcp_servers,
        plugins,
        marketplaces,
        hooks,
        memory_files,
        warnings,
    }
}

fn load_settings_files(paths: &ClaudePaths) -> Vec<SettingsFileInfo> {
    SettingsScope::ALL
        .into_iter()
        .filter_map(|scope| {
            let path = paths.settings_path(scope)?;
            let (exists, value, error) = match read_optional_json_object(&path) {
                Ok(Some(value)) => (true, Some(value), None),
                Ok(None) => (false, None, None),
                Err(err) => (true, None, Some(err)),
            };
            Some(SettingsFileInfo {
                scope,
                path,
                exists,
                value,
                error,
            })
        })
        .collect()
}

/// `Ok(None)` when the file does not exist.
pub fn read_optional_json_object(path: &Path) -> Result<Option<Map<String, Value>>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.to_string()),
    };
    if text.trim().is_empty() {
        return Ok(Some(Map::new()));
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => Ok(Some(map)),
        Ok(_) => Err("no contiene un objeto JSON".to_owned()),
        Err(err) => Err(format!("JSON inválido: {err}")),
    }
}

fn writable_item_roots(paths: &ClaudePaths) -> Vec<(ItemOrigin, PathBuf)> {
    let mut roots = vec![(ItemOrigin::User, paths.config_dir.clone())];
    if let Some(dir) = paths.project_claude_dir() {
        roots.push((ItemOrigin::Project, dir));
    }
    roots
}

fn sorted_entries(dir: &Path) -> Vec<fs::DirEntry> {
    let Ok(read_dir) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<_> = read_dir.filter_map(Result::ok).collect();
    entries.sort_by_key(|entry| entry.file_name());
    entries
}

fn read_markdown(path: &Path) -> (Option<MarkdownDoc>, Option<String>) {
    match fs::metadata(path) {
        Ok(metadata) if metadata.len() > MAX_MARKDOWN_BYTES => {
            return (
                None,
                Some("archivo demasiado grande para editar".to_owned()),
            );
        }
        Ok(_) => {}
        Err(err) => return (None, Some(err.to_string())),
    }
    match fs::read_to_string(path) {
        Ok(text) => match parse_markdown(&text) {
            Ok(doc) => (Some(doc), None),
            Err(err) => (None, Some(err)),
        },
        Err(err) => (None, Some(err.to_string())),
    }
}

/// Scans `<root>/<name>/SKILL.md`. The claude.ai `synced/` folder is scanned one level deeper
/// and listed read-only, since sync overwrites local edits.
fn scan_skills(root: &Path, origin: &ItemOrigin) -> Vec<MdItem> {
    let mut items = Vec::new();
    for entry in sorted_entries(root) {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let skill_file = dir.join("SKILL.md");
        if skill_file.is_file() {
            items.push(skill_item(&dir, skill_file, origin.clone()));
        } else if entry.file_name() == "synced" && *origin == ItemOrigin::User {
            let synced_origin = ItemOrigin::Plugin("claude.ai (sync)".to_owned());
            for synced in sorted_entries(&dir) {
                let synced_dir = synced.path();
                let synced_file = synced_dir.join("SKILL.md");
                if synced_file.is_file() {
                    items.push(skill_item(&synced_dir, synced_file, synced_origin.clone()));
                }
            }
        }
    }
    items
}

fn skill_item(dir: &Path, path: PathBuf, origin: ItemOrigin) -> MdItem {
    let (doc, error) = read_markdown(&path);
    let name = doc
        .as_ref()
        .and_then(|doc| doc.get("name"))
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| file_name_string(dir));
    MdItem {
        kind: MdKind::Skill,
        name,
        origin,
        path,
        doc,
        error,
        override_state: None,
    }
}

fn scan_markdown_tree(root: &Path, kind: MdKind, origin: &ItemOrigin) -> Vec<MdItem> {
    let mut items = Vec::new();
    scan_markdown_dir(root, root, kind, origin, 0, &mut items);
    items
}

fn scan_markdown_dir(
    root: &Path,
    dir: &Path,
    kind: MdKind,
    origin: &ItemOrigin,
    depth: usize,
    items: &mut Vec<MdItem>,
) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }
    for entry in sorted_entries(dir) {
        let path = entry.path();
        if path.is_dir() {
            scan_markdown_dir(root, &path, kind, origin, depth + 1, items);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let (doc, error) = read_markdown(&path);
        let relative_name = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .with_extension("")
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(":");
        let name = match kind {
            MdKind::Agent => doc
                .as_ref()
                .and_then(|doc| doc.get("name"))
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(relative_name),
            MdKind::Command | MdKind::Skill => relative_name,
        };
        items.push(MdItem {
            kind,
            name,
            origin: origin.clone(),
            path,
            doc,
            error,
            override_state: None,
        });
    }
}

fn file_name_string(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Merges a string-valued object (e.g. `skillOverrides`) across settings files; higher
/// precedence wins per key.
fn merged_string_map(
    settings: &[(SettingsScope, &Map<String, Value>)],
    key: &str,
) -> BTreeMap<String, String> {
    let mut merged = BTreeMap::new();
    for (_, value) in settings {
        if let Some(map) = value.get(key).and_then(Value::as_object) {
            for (name, state) in map {
                if let Some(state) = state.as_str() {
                    merged.insert(name.clone(), state.to_owned());
                }
            }
        }
    }
    merged
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn load_plugins(
    paths: &ClaudePaths,
    settings: &[(SettingsScope, &Map<String, Value>)],
    warnings: &mut Vec<String>,
) -> Vec<PluginInfo> {
    let mut plugins: BTreeMap<String, PluginInfo> = BTreeMap::new();
    let installed_path = paths.plugins_dir.join("installed_plugins.json");
    match read_optional_json_object(&installed_path) {
        Ok(Some(root)) => {
            if let Some(entries) = root.get("plugins").and_then(Value::as_object) {
                for (id, record) in entries {
                    // v2 stores a list of installs per plugin, v1 a single object.
                    let record = match record {
                        Value::Array(records) => records.first().and_then(Value::as_object),
                        Value::Object(record) => Some(record),
                        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => None,
                    };
                    plugins.insert(
                        id.clone(),
                        PluginInfo {
                            id: id.clone(),
                            installed: true,
                            version: record
                                .and_then(|r| r.get("version"))
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            install_path: record
                                .and_then(|r| r.get("installPath"))
                                .and_then(Value::as_str)
                                .map(PathBuf::from),
                            install_scope: record
                                .and_then(|r| r.get("scope"))
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            enabled: None,
                            enabled_source: None,
                            mentioned_in: Vec::new(),
                        },
                    );
                }
            }
        }
        Ok(None) => {}
        Err(err) => warnings.push(format!("{}: {err}", installed_path.display())),
    }
    for (scope, value) in settings {
        let Some(enabled) = value.get("enabledPlugins").and_then(Value::as_object) else {
            continue;
        };
        for (id, flag) in enabled {
            let Some(flag) = flag.as_bool() else {
                continue;
            };
            let plugin = plugins.entry(id.clone()).or_insert_with(|| PluginInfo {
                id: id.clone(),
                installed: false,
                version: None,
                install_path: None,
                install_scope: None,
                enabled: None,
                enabled_source: None,
                mentioned_in: Vec::new(),
            });
            plugin.enabled = Some(flag);
            plugin.enabled_source = Some(*scope);
            plugin.mentioned_in.push(*scope);
        }
    }
    plugins.into_values().collect()
}

fn load_marketplaces(paths: &ClaudePaths, warnings: &mut Vec<String>) -> Vec<MarketplaceInfo> {
    let path = paths.plugins_dir.join("known_marketplaces.json");
    let root = match read_optional_json_object(&path) {
        Ok(Some(root)) => root,
        Ok(None) => return Vec::new(),
        Err(err) => {
            warnings.push(format!("{}: {err}", path.display()));
            return Vec::new();
        }
    };
    root.iter()
        .map(|(name, entry)| {
            let source = entry.get("source");
            let source_text = source
                .and_then(|source| {
                    ["repo", "url", "path", "source"]
                        .iter()
                        .find_map(|key| source.get(*key).and_then(Value::as_str))
                        .or_else(|| source.as_str())
                })
                .unwrap_or_default()
                .to_owned();
            MarketplaceInfo {
                name: name.clone(),
                source: source_text,
                install_location: entry
                    .get("installLocation")
                    .and_then(Value::as_str)
                    .map(PathBuf::from),
            }
        })
        .collect()
}

/// Parses one `mcpServers`-style object.
pub fn parse_mcp_servers(
    servers: &Map<String, Value>,
    scope: &McpScope,
    path: &Path,
    state: impl Fn(&str) -> McpState,
) -> Vec<McpServer> {
    servers
        .iter()
        .filter_map(|(name, entry)| {
            let entry = entry.as_object()?;
            let str_field = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_owned);
            let keys = |key: &str| -> Vec<String> {
                entry
                    .get(key)
                    .and_then(Value::as_object)
                    .map(|map| map.keys().cloned().collect())
                    .unwrap_or_default()
            };
            Some(McpServer {
                name: name.clone(),
                scope: scope.clone(),
                path: path.to_path_buf(),
                transport: str_field("type").unwrap_or_else(|| "stdio".to_owned()),
                command: str_field("command"),
                args: string_list(entry.get("args")),
                url: str_field("url"),
                env_keys: keys("env"),
                header_keys: keys("headers"),
                raw: entry.clone(),
                state: state(name),
            })
        })
        .collect()
}

fn load_mcp_servers(
    paths: &ClaudePaths,
    claude_json: &Map<String, Value>,
    project_entry: Option<&(String, Map<String, Value>)>,
    settings: &[(SettingsScope, &Map<String, Value>)],
    plugins: &[PluginInfo],
    warnings: &mut Vec<String>,
) -> Vec<McpServer> {
    let disabled: BTreeSet<String> = project_entry
        .map(|(_, entry)| string_list(entry.get("disabledMcpServers")))
        .unwrap_or_default()
        .into_iter()
        .collect();
    let user_state = |name: &str| {
        if disabled.contains(name) {
            McpState::Disabled
        } else {
            McpState::Enabled
        }
    };

    let mut servers = Vec::new();
    if let Some(user) = claude_json.get("mcpServers").and_then(Value::as_object) {
        servers.extend(parse_mcp_servers(
            user,
            &McpScope::User,
            &paths.claude_json,
            user_state,
        ));
    }
    if let Some((key, entry)) = project_entry
        && let Some(local) = entry.get("mcpServers").and_then(Value::as_object)
    {
        servers.extend(parse_mcp_servers(
            local,
            &McpScope::Local(key.clone()),
            &paths.claude_json,
            user_state,
        ));
    }

    if let Some(mcp_json) = paths.project_mcp_json() {
        match read_optional_json_object(&mcp_json) {
            Ok(Some(root)) => {
                // Older Claude Code versions kept approvals in `.claude.json` per project.
                let mut sources: Vec<&Map<String, Value>> =
                    settings.iter().map(|(_, value)| *value).collect();
                if let Some((_, entry)) = project_entry {
                    sources.push(entry);
                }
                let rejected: BTreeSet<String> = sources
                    .iter()
                    .flat_map(|value| string_list(value.get("disabledMcpjsonServers")))
                    .collect();
                let approved: BTreeSet<String> = sources
                    .iter()
                    .flat_map(|value| string_list(value.get("enabledMcpjsonServers")))
                    .collect();
                let approve_all = sources.iter().any(|value| {
                    value
                        .get("enableAllProjectMcpServers")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                });
                let project_state = |name: &str| {
                    if rejected.contains(name) {
                        McpState::Disabled
                    } else if approve_all || approved.contains(name) {
                        McpState::Enabled
                    } else {
                        McpState::PendingApproval
                    }
                };
                if let Some(project) = root.get("mcpServers").and_then(Value::as_object) {
                    servers.extend(parse_mcp_servers(
                        project,
                        &McpScope::Project,
                        &mcp_json,
                        project_state,
                    ));
                }
            }
            Ok(None) => {}
            Err(err) => warnings.push(format!("{}: {err}", mcp_json.display())),
        }
    }

    for plugin in plugins.iter().filter(|plugin| plugin.installed) {
        let Some(root) = &plugin.install_path else {
            continue;
        };
        let path = root.join(".mcp.json");
        let plugin_state = |_: &str| {
            if plugin.enabled == Some(true) {
                McpState::Enabled
            } else {
                McpState::Disabled
            }
        };
        match read_optional_json_object(&path) {
            Ok(Some(root)) => {
                let servers_map = root
                    .get("mcpServers")
                    .and_then(Value::as_object)
                    .unwrap_or(&root);
                servers.extend(parse_mcp_servers(
                    servers_map,
                    &McpScope::Plugin(plugin.id.clone()),
                    &path,
                    plugin_state,
                ));
            }
            Ok(None) => {}
            Err(err) => warnings.push(format!("{}: {err}", path.display())),
        }
    }
    servers
}

/// Parses the `hooks` object of a settings or `hooks.json` file.
pub fn parse_hooks(root: &Map<String, Value>, source: HookSource, path: &Path) -> Vec<HookItem> {
    let Some(events) = root.get("hooks").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for (event, groups) in events {
        let Some(groups) = groups.as_array() else {
            continue;
        };
        for (group_index, group) in groups.iter().enumerate() {
            let matcher = group
                .get("matcher")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let Some(hooks) = group.get("hooks").and_then(Value::as_array) else {
                continue;
            };
            for (hook_index, hook) in hooks.iter().enumerate() {
                items.push(HookItem {
                    source: source.clone(),
                    path: path.to_path_buf(),
                    event: event.clone(),
                    group_index,
                    hook_index,
                    matcher: matcher.clone(),
                    hook_type: hook
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("command")
                        .to_owned(),
                    command: hook
                        .get("command")
                        .or_else(|| hook.get("prompt"))
                        .or_else(|| hook.get("url"))
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    timeout: hook.get("timeout").and_then(Value::as_u64),
                });
            }
        }
    }
    items
}

fn attributions(settings: &[(SettingsScope, &Map<String, Value>)]) -> Vec<Attribution> {
    let mut by_key: BTreeMap<String, Vec<(SettingsScope, String)>> = BTreeMap::new();
    for (scope, value) in settings {
        for (key, entry) in value.iter() {
            by_key
                .entry(key.clone())
                .or_default()
                .push((*scope, super::masking::summarize_value(key, entry)));
        }
    }
    by_key
        .into_iter()
        .filter_map(|(key, entries)| {
            let effective = entries.iter().map(|(scope, _)| *scope).max()?;
            Some(Attribution {
                key,
                entries,
                effective,
            })
        })
        .collect()
}

fn permission_rules(settings: &[(SettingsScope, &Map<String, Value>)]) -> Vec<PermissionRule> {
    let mut rules = Vec::new();
    for (scope, value) in settings {
        let Some(permissions) = value.get("permissions") else {
            continue;
        };
        for kind in ["allow", "deny", "ask"] {
            for rule in string_list(permissions.get(kind)) {
                rules.push(PermissionRule {
                    kind,
                    rule,
                    scope: *scope,
                });
            }
        }
    }
    rules
}

#[cfg(test)]
#[path = "snapshot_tests.rs"]
mod tests;
