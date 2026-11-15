//! Resolution of every on-disk location the Claude Code panel reads or writes.

use std::path::{Path, PathBuf};

use super::SettingsScope;

/// The project the panel is scoped to: the active pane's working directory and the repository
/// root above it (or the working directory itself outside a repository).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectPaths {
    pub root: PathBuf,
    pub cwd: PathBuf,
}

/// All Claude Code locations for the user scope and, when known, the current project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaudePaths {
    /// `$CLAUDE_CONFIG_DIR`, or `~/.claude`.
    pub config_dir: PathBuf,
    /// `$CLAUDE_CONFIG_DIR/.claude.json`, or `~/.claude.json`.
    pub claude_json: PathBuf,
    pub managed_settings: Option<PathBuf>,
    /// `$CLAUDE_CODE_PLUGIN_CACHE_DIR`, or `<config dir>/plugins`.
    pub plugins_dir: PathBuf,
    pub project: Option<ProjectPaths>,
}

impl ClaudePaths {
    /// Resolves paths from the process environment and the given working directory.
    pub fn from_env(cwd: Option<PathBuf>) -> Option<Self> {
        Self::resolve(
            |key| std::env::var(key).ok().filter(|value| !value.is_empty()),
            dirs::home_dir(),
            cwd,
        )
    }

    /// Resolves paths from an explicit environment lookup, home directory and working directory.
    pub fn resolve(
        env: impl Fn(&str) -> Option<String>,
        home: Option<PathBuf>,
        cwd: Option<PathBuf>,
    ) -> Option<Self> {
        let (config_dir, claude_json) = match env("CLAUDE_CONFIG_DIR") {
            Some(dir) => {
                let dir = PathBuf::from(dir);
                let claude_json = dir.join(".claude.json");
                (dir, claude_json)
            }
            None => {
                let home = home.as_ref()?;
                (home.join(".claude"), home.join(".claude.json"))
            }
        };
        let plugins_dir = env("CLAUDE_CODE_PLUGIN_CACHE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| config_dir.join("plugins"));
        let project = cwd.and_then(|cwd| {
            let root = find_project_root(&cwd);
            // A "project" rooted at home would alias the user-scope files (~/.claude/...).
            if home.as_deref() == Some(root.as_path()) || root.join(".claude") == config_dir {
                None
            } else {
                Some(ProjectPaths { root, cwd })
            }
        });
        Some(Self {
            config_dir,
            claude_json,
            managed_settings: managed_settings_path(),
            plugins_dir,
            project,
        })
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.config_dir.join("warp").join("backups")
    }

    pub fn settings_path(&self, scope: SettingsScope) -> Option<PathBuf> {
        match scope {
            SettingsScope::User => Some(self.config_dir.join("settings.json")),
            SettingsScope::Project => self
                .project_claude_dir()
                .map(|dir| dir.join("settings.json")),
            SettingsScope::Local => self
                .project_claude_dir()
                .map(|dir| dir.join("settings.local.json")),
            SettingsScope::Managed => self.managed_settings.clone(),
        }
    }

    pub fn project_claude_dir(&self) -> Option<PathBuf> {
        self.project
            .as_ref()
            .map(|project| project.root.join(".claude"))
    }

    pub fn project_mcp_json(&self) -> Option<PathBuf> {
        self.project
            .as_ref()
            .map(|project| project.root.join(".mcp.json"))
    }

    /// The directory holding skills, subagents or commands (`subdir`) for a writable origin.
    pub fn items_dir(&self, project: bool, subdir: &str) -> Option<PathBuf> {
        if project {
            self.project_claude_dir().map(|dir| dir.join(subdir))
        } else {
            Some(self.config_dir.join(subdir))
        }
    }

    /// Candidate keys under `projects` in `.claude.json` for the current project, best first.
    pub fn project_keys(&self) -> Vec<String> {
        let Some(project) = &self.project else {
            return Vec::new();
        };
        let mut keys = vec![project.root.to_string_lossy().into_owned()];
        let cwd = project.cwd.to_string_lossy().into_owned();
        if !keys.contains(&cwd) {
            keys.push(cwd);
        }
        keys
    }

    /// Memory files Claude Code loads, whether or not they exist yet.
    pub fn memory_files(&self) -> Vec<(String, PathBuf)> {
        let mut files = vec![("Usuario".to_owned(), self.config_dir.join("CLAUDE.md"))];
        if let Some(project) = &self.project {
            files.push(("Proyecto".to_owned(), project.root.join("CLAUDE.md")));
            files.push((
                "Proyecto (.claude)".to_owned(),
                project.root.join(".claude").join("CLAUDE.md"),
            ));
            files.push(("Local".to_owned(), project.root.join("CLAUDE.local.md")));
        }
        files
    }
}

/// Walks up from `cwd` to the nearest directory containing `.git` (a directory, or a file for
/// worktrees). Falls back to `cwd`.
pub fn find_project_root(cwd: &Path) -> PathBuf {
    cwd.ancestors()
        .find(|dir| dir.join(".git").exists())
        .unwrap_or(cwd)
        .to_path_buf()
}

fn managed_settings_path() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        Some(PathBuf::from(
            "/Library/Application Support/ClaudeCode/managed-settings.json",
        ))
    } else if cfg!(target_os = "linux") {
        Some(PathBuf::from("/etc/claude-code/managed-settings.json"))
    } else if cfg!(windows) {
        Some(PathBuf::from(
            r"C:\Program Files\ClaudeCode\managed-settings.json",
        ))
    } else {
        None
    }
}

#[cfg(test)]
#[path = "paths_tests.rs"]
mod tests;
