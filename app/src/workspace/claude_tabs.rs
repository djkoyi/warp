//! Pure helpers for Claude Code tabs: automatic tab names and colors per project, the tab
//! launched in the current directory, and named workspaces.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use settings::Setting as _;
use warp_core::ui::theme::AnsiColorIdentifier;
use warpui::{AppContext, SingletonEntity};

use crate::launch_configs::launch_config::{
    CommandTemplate, LaunchConfig, PaneMode, PaneTemplateType, TabTemplate, WindowTemplate,
};
use crate::terminal::TerminalView;
use crate::ui_components::color_dot::TAB_COLOR_OPTIONS;
use crate::workspace::tab_settings::TabSettings;

pub(crate) const DEFAULT_CLAUDE_COMMAND: &str = "claude --dangerously-skip-permissions";

/// Prefix of the Command Palette entries generated for each configured workspace.
pub(crate) const WORKSPACE_PALETTE_PREFIX: &str = "Abrir espacio de trabajo: ";

/// The directory that identifies the project of `view`: the git repository root when one was
/// detected, otherwise the session's working directory. `None` for remote sessions.
pub(crate) fn project_dir(view: &TerminalView, ctx: &AppContext) -> Option<PathBuf> {
    view.current_local_repo_path()
        .map(Path::to_path_buf)
        .or_else(|| view.active_session_path_if_local(ctx))
}

/// Like [`project_dir`], but without filesystem access, so it is cheap enough to call while
/// rendering. The working directory is not checked for existence.
pub(crate) fn project_dir_without_io(view: &TerminalView, ctx: &AppContext) -> Option<PathBuf> {
    if let Some(repo) = view.current_local_repo_path() {
        return Some(repo.to_path_buf());
    }
    match view.active_session_is_local(ctx) {
        Some(true) => view.pwd().map(PathBuf::from),
        Some(false) | None => None,
    }
}

/// Tab title of the form `<folder> · <branch>`, or just `<folder>` outside of a branch.
pub(crate) fn auto_tab_title(project_dir: &Path, branch: Option<&str>) -> Option<String> {
    let folder = sanitize_title_part(&project_dir.file_name()?.to_string_lossy());
    if folder.is_empty() {
        return None;
    }
    match branch.map(sanitize_title_part).filter(|b| !b.is_empty()) {
        Some(branch) => Some(format!("{folder} · {branch}")),
        None => Some(folder),
    }
}

// Folder and branch names come from the filesystem and git, so they can hold control characters.
fn sanitize_title_part(part: &str) -> String {
    part.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .to_owned()
}

/// A stable color for `project_dir`, picked from the same palette as the manual tab colors.
pub(crate) fn project_color(project_dir: &Path) -> AnsiColorIdentifier {
    // FNV-1a rather than `DefaultHasher`, whose output is not guaranteed stable across releases.
    let hash = project_dir
        .to_string_lossy()
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        });
    TAB_COLOR_OPTIONS[(hash % TAB_COLOR_OPTIONS.len() as u64) as usize]
}

/// A single terminal pane that starts in `cwd` and runs `command`. The directory travels as a
/// path, never as part of the command line.
pub(crate) fn claude_pane_template(cwd: PathBuf, command: &str) -> PaneTemplateType {
    let command = command.trim();
    PaneTemplateType::PaneTemplate {
        cwd,
        commands: if command.is_empty() {
            Vec::new()
        } else {
            vec![CommandTemplate::from(command)]
        },
        is_focused: Some(true),
        pane_mode: PaneMode::Terminal,
        shell: None,
    }
}

/// One launch configuration per workspace, opening one Claude tab per directory that satisfies
/// `dir_exists`. Workspaces left with no usable directory are dropped.
pub(crate) fn workspace_launch_configs(
    workspaces: &HashMap<String, Vec<String>>,
    command: &str,
    dir_exists: impl Fn(&Path) -> bool,
) -> Vec<LaunchConfig> {
    let mut names: Vec<&String> = workspaces.keys().collect();
    names.sort();

    names
        .into_iter()
        .filter_map(|name| {
            let display_name = name.trim();
            if display_name.is_empty() {
                return None;
            }
            let tabs: Vec<TabTemplate> = workspaces[name]
                .iter()
                .map(|dir| PathBuf::from(shellexpand::tilde(dir.trim()).into_owned()))
                .filter(|dir| dir_exists(dir))
                .map(|dir| TabTemplate {
                    title: None,
                    layout: claude_pane_template(dir, command),
                    commands: Vec::new(),
                    color: None,
                    group: None,
                })
                .collect();
            if tabs.is_empty() {
                return None;
            }
            Some(LaunchConfig {
                name: format!("{WORKSPACE_PALETTE_PREFIX}{display_name}"),
                active_window_index: Some(0),
                windows: vec![WindowTemplate {
                    active_tab_index: Some(0),
                    tabs,
                    tab_groups: Vec::new(),
                }],
            })
        })
        .collect()
}

/// Launch configurations for the workspaces in the user's settings.
pub(crate) fn configured_workspace_launch_configs(ctx: &AppContext) -> Vec<LaunchConfig> {
    let tab_settings = TabSettings::as_ref(ctx);
    workspace_launch_configs(
        tab_settings.claude_workspaces.value(),
        tab_settings.claude_command.value(),
        Path::is_dir,
    )
}

#[cfg(test)]
#[path = "claude_tabs_tests.rs"]
mod tests;
