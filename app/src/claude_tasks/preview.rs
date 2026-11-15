//! App preview: detects local dev-server URLs printed in the focused pane and resolves the
//! per-project command that starts the dev server.
//!
//! WarpUI has no embedded web view, so previews open in the system browser.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ::settings::Setting as _;
use lazy_static::lazy_static;
use regex::Regex;
use warpui::r#async::Timer;
use warpui::{AppContext, Entity, EntityId, ModelContext, SingletonEntity, ViewHandle, WindowId};

use super::command::expand_dir;
use super::settings::{ClaudeTasksSettings, PreviewCommand};
use super::show_toast;
use crate::terminal::TerminalView;
use crate::view_components::{DismissibleToast, ToastLink};
use crate::workspace::WorkspaceRegistry;

const POLL_INTERVAL: Duration = Duration::from_secs(2);
const SUMMARY_HEAD_ROWS: usize = 80;
const SUMMARY_TAIL_ROWS: usize = 60;
const MAX_REMEMBERED: usize = 512;

/// Ports of common databases and caches: a bare `localhost:5432` is almost never a web app.
const NON_HTTP_PORTS: &[u16] = &[1433, 1521, 3306, 5432, 6379, 9200, 11211, 27017];

lazy_static! {
    static ref LOCAL_URL: Regex = Regex::new(
        r"(?i)(https?://)?(localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1\]):(\d{2,5})\b(/[^\s'\x22<>()\[\]{}|\\^`]*)?"
    )
    .expect("valid regex");
}

/// Returns the distinct local dev-server URLs in `text`, normalized to an openable form, in order
/// of first appearance.
pub fn extract_dev_server_urls(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut urls = Vec::new();
    for captures in LOCAL_URL.captures_iter(text) {
        let whole = captures.get(0).expect("group 0 always exists");
        let scheme = captures.get(1).map(|m| m.as_str().to_ascii_lowercase());
        if scheme.is_none() {
            // A scheme-less match glued to a preceding word or `://` belongs to another URL
            // (e.g. `postgres://localhost:5432`).
            let preceding = text[..whole.start()].chars().next_back();
            if preceding.is_some_and(|c| c.is_alphanumeric() || matches!(c, '/' | '@' | '.')) {
                continue;
            }
        }
        let Ok(port) = captures[3].parse::<u16>() else {
            continue;
        };
        if port == 0 || (scheme.is_none() && NON_HTTP_PORTS.contains(&port)) {
            continue;
        }
        let host = match captures[2].to_ascii_lowercase().as_str() {
            "0.0.0.0" => "localhost".to_owned(),
            other => other.to_owned(),
        };
        let path = captures
            .get(4)
            .map(|m| m.as_str().trim_end_matches(['.', ',', ';', ':', '!', '?']))
            .unwrap_or("");
        let url = format!(
            "{}{host}:{port}{path}",
            scheme.as_deref().unwrap_or("http://")
        );
        if seen.insert(url.clone()) {
            urls.push(url);
        }
    }
    urls
}

/// `scheme://host:port` of a URL produced by [`extract_dev_server_urls`].
pub fn url_origin(url: &str) -> &str {
    let after_scheme = url.find("://").map_or(0, |index| index + 3);
    match url[after_scheme..].find('/') {
        Some(index) => &url[..after_scheme + index],
        None => url,
    }
}

/// The command that starts the dev server for `cwd`, and the directory to run it in.
///
/// A configured entry whose `project_dir` contains `cwd` wins (the deepest one if several do).
/// Otherwise the nearest `package.json` with a `dev` (or else `start`) script is used.
pub fn resolve_preview_command(
    configured: &[PreviewCommand],
    cwd: &Path,
) -> Option<(PathBuf, String)> {
    let configured_match = configured
        .iter()
        .filter(|entry| !entry.command.trim().is_empty())
        .map(|entry| (expand_dir(&entry.project_dir), entry))
        .filter(|(dir, _)| cwd.starts_with(dir))
        .max_by_key(|(dir, _)| dir.components().count())
        .map(|(dir, entry)| (dir, entry.command.trim().to_owned()));
    configured_match.or_else(|| detect_package_json_command(cwd))
}

fn detect_package_json_command(cwd: &Path) -> Option<(PathBuf, String)> {
    let dir = cwd
        .ancestors()
        .find(|dir| dir.join("package.json").is_file())?;
    let contents = std::fs::read_to_string(dir.join("package.json")).ok()?;
    let script = package_json_dev_script(&contents)?;
    Some((dir.to_path_buf(), package_manager_run(dir, script)))
}

/// Name of the script that starts the dev server, from `package.json` contents.
pub fn package_json_dev_script(contents: &str) -> Option<&'static str> {
    let json: serde_json::Value = serde_json::from_str(contents).ok()?;
    let scripts = json.get("scripts")?.as_object()?;
    ["dev", "start"]
        .into_iter()
        .find(|name| scripts.contains_key(*name))
}

fn package_manager_run(dir: &Path, script: &str) -> String {
    let manager = if dir.join("pnpm-lock.yaml").is_file() {
        "pnpm"
    } else if dir.join("yarn.lock").is_file() {
        "yarn"
    } else if dir.join("bun.lockb").is_file() || dir.join("bun.lock").is_file() {
        "bun run"
    } else {
        "npm run"
    };
    format!("{manager} {script}")
}

/// Plain text of the part of the pane where a dev server prints its URL: the visible alt screen,
/// or the start and end of the running block. Returns `None` when the model is busy, so polling
/// never blocks the UI thread on the PTY.
pub fn recent_output(view: &ViewHandle<TerminalView>, ctx: &AppContext) -> Option<String> {
    let model = view.as_ref(ctx).model.try_lock()?;
    if model.is_alt_screen_active() {
        return Some(model.alt_screen().output_to_string());
    }
    let block = model.block_list().active_block();
    if !block.is_executing() {
        return None;
    }
    Some(
        block
            .output_grid()
            .content_summary(SUMMARY_HEAD_ROWS, SUMMARY_TAIL_ROWS, false),
    )
}

/// The URL to open for the pane: the most recently printed one.
pub fn latest_dev_server_url(view: &ViewHandle<TerminalView>, ctx: &AppContext) -> Option<String> {
    extract_dev_server_urls(&recent_output(view, ctx)?).pop()
}

/// Polls the focused pane of every window and offers to open newly printed dev-server URLs.
pub struct PreviewDetector {
    offered: HashSet<(EntityId, String)>,
}

impl PreviewDetector {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        Self::schedule_poll(ctx);
        Self {
            offered: HashSet::new(),
        }
    }

    fn schedule_poll(ctx: &mut ModelContext<Self>) {
        ctx.spawn(async { Timer::after(POLL_INTERVAL).await }, |me, _, ctx| {
            me.poll(ctx);
            Self::schedule_poll(ctx);
        });
    }

    fn poll(&mut self, ctx: &mut ModelContext<Self>) {
        if !*ClaudeTasksSettings::as_ref(ctx)
            .preview_detection_enabled
            .value()
        {
            return;
        }
        let focused_views: Vec<(WindowId, ViewHandle<TerminalView>)> =
            WorkspaceRegistry::as_ref(ctx)
                .all_workspaces(ctx)
                .into_iter()
                .filter_map(|(window_id, workspace)| {
                    // Polling can land while a window is mid-teardown with no tabs left.
                    let workspace = workspace.as_ref(ctx);
                    let view = workspace
                        .get_pane_group_view(workspace.active_tab_index())?
                        .as_ref(ctx)
                        .focused_session_view(ctx)?;
                    Some((window_id, view))
                })
                .collect();

        let mut offers = Vec::new();
        for (window_id, view) in focused_views {
            let Some(text) = recent_output(&view, ctx) else {
                continue;
            };
            for url in extract_dev_server_urls(&text) {
                if self
                    .offered
                    .insert((view.id(), url_origin(&url).to_owned()))
                {
                    offers.push((window_id, url));
                }
            }
        }
        if self.offered.len() > MAX_REMEMBERED {
            self.offered.clear();
        }
        for (window_id, url) in offers {
            let toast = DismissibleToast::default(format!("Servidor local detectado: {url}"))
                .with_link(ToastLink::new("Abrir vista previa".to_owned()).with_href(url.clone()))
                .with_object_id(format!("claude_tasks_preview:{url}"));
            show_toast(toast, window_id, ctx);
        }
    }
}

impl Entity for PreviewDetector {
    type Event = ();
}

impl SingletonEntity for PreviewDetector {}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod tests;
