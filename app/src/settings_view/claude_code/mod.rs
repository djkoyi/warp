//! Reading and editing the user's Claude Code configuration (skills, MCP servers, plugins,
//! subagents, slash commands, hooks, settings files and memory files) for the user scope and
//! the current project. The UI lives in `claude_code_page.rs`; everything here is free of view
//! code so it can run on a background thread and be unit-tested against temp dirs.

pub mod edits;
pub mod frontmatter;
pub mod masking;
pub mod paths;
pub mod safe_write;
pub mod snapshot;

use std::fmt;

/// One of the Claude Code settings files, in increasing order of precedence.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SettingsScope {
    /// `<config dir>/settings.json`.
    User,
    /// `<project>/.claude/settings.json`, usually committed.
    Project,
    /// `<project>/.claude/settings.local.json`, usually gitignored.
    Local,
    /// The managed (enterprise) settings file. Never written.
    Managed,
}

impl SettingsScope {
    /// All scopes ordered from lowest to highest precedence.
    pub const ALL: [SettingsScope; 4] = [
        SettingsScope::User,
        SettingsScope::Project,
        SettingsScope::Local,
        SettingsScope::Managed,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SettingsScope::User => "Usuario",
            SettingsScope::Project => "Proyecto",
            SettingsScope::Local => "Local",
            SettingsScope::Managed => "Administrado",
        }
    }

    pub fn is_writable(self) -> bool {
        match self {
            SettingsScope::User | SettingsScope::Project | SettingsScope::Local => true,
            SettingsScope::Managed => false,
        }
    }
}

impl fmt::Display for SettingsScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Where a skill, subagent or slash command file was found.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ItemOrigin {
    User,
    Project,
    /// Shipped inside an installed plugin; the payload is the plugin id (`name@marketplace`).
    Plugin(String),
}

impl ItemOrigin {
    pub fn label(&self) -> String {
        match self {
            ItemOrigin::User => "Usuario".to_owned(),
            ItemOrigin::Project => "Proyecto".to_owned(),
            ItemOrigin::Plugin(id) => format!("Plugin {id}"),
        }
    }

    pub fn is_read_only(&self) -> bool {
        match self {
            ItemOrigin::User | ItemOrigin::Project => false,
            ItemOrigin::Plugin(_) => true,
        }
    }
}

/// Validates a user-supplied identifier (skill, subagent, command or MCP server name).
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("El nombre no puede estar vacío".to_owned());
    }
    if name.len() > 64 {
        return Err("El nombre no puede superar 64 caracteres".to_owned());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("El nombre solo admite letras, números, '-' y '_'".to_owned());
    }
    Ok(())
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
