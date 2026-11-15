//! Parallel, isolated Claude Code sessions backed by git worktrees under
//! `<repo>/.claude/worktrees/<name>`, the same location Claude Code's own `--worktree` flag uses.
//!
//! Every git invocation goes through an argument vector; no value derived from the filesystem
//! or the user ever reaches a shell command line.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow};
use chrono::{DateTime, Local};
use warp_util::git::run_git_command_with_env;

#[cfg(test)]
#[path = "claude_worktrees_tests.rs"]
mod tests;

/// Command started in the new tab. A constant: it never interpolates paths or names.
pub const CLAUDE_WORKTREE_SESSION_COMMAND: &str = "claude --dangerously-skip-permissions";

const WORKTREES_DIR: &str = ".claude/worktrees";
const BRANCH_PREFIX: &str = "claude/";
const EXCLUDE_PATTERN: &str = "/.claude/worktrees/";

/// A worktree created for a new Claude session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedClaudeWorktree {
    pub name: String,
    pub branch: String,
    pub path: PathBuf,
}

/// One entry of `git worktree list --porcelain`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    /// Short branch name, `None` for a detached HEAD.
    pub branch: Option<String>,
    pub locked: bool,
    pub prunable: bool,
}

/// A Claude worktree and whether it has uncommitted changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeWorktreeStatus {
    pub entry: WorktreeEntry,
    pub is_dirty: bool,
}

/// Outcome of [`cleanup_claude_worktrees`]. Branches are never deleted, so commits made in a
/// removed worktree stay reachable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanupReport {
    pub removed: Vec<String>,
    pub kept_dirty: Vec<String>,
    pub kept_in_use: Vec<String>,
    pub kept_locked: Vec<String>,
    pub failed: Vec<String>,
}

/// Directory that holds the Claude worktrees of `main_repo_root`.
pub fn worktrees_root(main_repo_root: &Path) -> PathBuf {
    main_repo_root.join(WORKTREES_DIR)
}

/// Default worktree name for a session started at `now`, e.g. `claude-20261115-143012`.
pub fn worktree_name_for_time(now: DateTime<Local>) -> String {
    now.format("claude-%Y%m%d-%H%M%S").to_string()
}

/// Returns `base`, or `base-2`, `base-3`, ... — the first name for which `is_taken` is false.
pub fn unique_worktree_name(base: &str, is_taken: impl Fn(&str) -> bool) -> String {
    if !is_taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|suffix| format!("{base}-{suffix}"))
        .find(|candidate| !is_taken(candidate))
        .expect("an unbounded range always yields an untaken name")
}

/// Branch created for a worktree named `name`.
pub fn branch_for_worktree(name: &str) -> String {
    format!("{BRANCH_PREFIX}{name}")
}

/// Parses `git worktree list --porcelain` output.
pub fn parse_worktree_porcelain(output: &str) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut current: Option<WorktreeEntry> = None;
    for line in output.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            entries.extend(current.take());
            current = Some(WorktreeEntry {
                path: PathBuf::from(path),
                ..Default::default()
            });
            continue;
        }
        let Some(entry) = current.as_mut() else {
            continue;
        };
        if let Some(branch) = line.strip_prefix("branch ") {
            entry.branch = Some(
                branch
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch)
                    .to_string(),
            );
        } else if line == "locked" || line.starts_with("locked ") {
            entry.locked = true;
        } else if line == "prunable" || line.starts_with("prunable ") {
            entry.prunable = true;
        }
    }
    entries.extend(current);
    entries
}

/// Keeps the entries located inside `worktrees_root`. Both sides are compared canonicalized when
/// possible so `/var` vs `/private/var` style aliases on macOS don't hide worktrees.
pub fn filter_claude_worktrees(
    entries: Vec<WorktreeEntry>,
    worktrees_root: &Path,
) -> Vec<WorktreeEntry> {
    let root = canonicalize_or_self(worktrees_root);
    entries
        .into_iter()
        .filter(|entry| {
            let path = canonicalize_or_self(&entry.path);
            path != root && path.starts_with(&root)
        })
        .collect()
}

/// The main worktree root for a `--git-common-dir` value: the common dir's parent when it is a
/// `.git` directory, otherwise `None` (bare repositories have no main worktree).
pub fn main_worktree_root_from_common_dir(common_dir: &Path) -> Option<PathBuf> {
    if common_dir.file_name()? != ".git" {
        return None;
    }
    common_dir.parent().map(Path::to_path_buf)
}

/// Whether `info/exclude` contents already ignore the Claude worktrees directory.
pub fn exclude_has_worktrees_entry(contents: &str) -> bool {
    contents.lines().map(str::trim).any(|line| {
        matches!(
            line,
            "/.claude/worktrees/"
                | "/.claude/worktrees"
                | ".claude/worktrees/"
                | ".claude/worktrees"
        )
    })
}

/// Whether `path` is `root` or inside it, comparing canonicalized paths when possible.
pub fn is_path_within(path: &Path, root: &Path) -> bool {
    canonicalize_or_self(path).starts_with(canonicalize_or_self(root))
}

/// Display name of a worktree: its directory name.
pub fn worktree_display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// One-line summary of the Claude worktrees for a toast.
pub fn summarize_worktrees(statuses: &[ClaudeWorktreeStatus]) -> String {
    if statuses.is_empty() {
        return "No hay worktrees de Claude en este repositorio.".to_string();
    }
    let items = statuses
        .iter()
        .map(|status| {
            let name = worktree_display_name(&status.entry.path);
            let state = if status.is_dirty {
                "con cambios"
            } else {
                "limpio"
            };
            format!("{name} ({state})")
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("Worktrees de Claude ({}): {items}", statuses.len())
}

/// One-line summary of a cleanup run for a toast.
pub fn summarize_cleanup(report: &CleanupReport) -> String {
    let mut parts = vec![format!("Eliminados: {}", report.removed.len())];
    let mut push_kept = |label: &str, names: &[String]| {
        if !names.is_empty() {
            parts.push(format!("{label}: {}", names.join(", ")));
        }
    };
    push_kept("Con cambios (conservados)", &report.kept_dirty);
    push_kept("Abiertos en una pestaña", &report.kept_in_use);
    push_kept("Bloqueados", &report.kept_locked);
    push_kept("Fallaron", &report.failed);
    format!(
        "Limpieza de worktrees de Claude. {}. Las ramas claude/* se conservan.",
        parts.join(". ")
    )
}

fn canonicalize_or_self(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn path_arg(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| anyhow!("La ruta no es UTF-8: {}", path.display()))
}

async fn git(dir: &Path, args: &[&str], path_env: Option<&str>) -> Result<String> {
    run_git_command_with_env(dir, args, path_env).await
}

/// Resolves the main worktree root of the repository containing `start`, so a session started
/// from inside a Claude worktree doesn't nest a new worktree inside it.
pub async fn resolve_main_repo_root(start: &Path, path_env: Option<&str>) -> Result<PathBuf> {
    let common_dir = git(
        start,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        path_env,
    )
    .await
    .map_err(|_| anyhow!("La pestaña activa no está dentro de un repositorio git"))?;
    if let Some(root) = main_worktree_root_from_common_dir(Path::new(common_dir.trim())) {
        return Ok(root);
    }
    let toplevel = git(start, &["rev-parse", "--show-toplevel"], path_env).await?;
    Ok(PathBuf::from(toplevel.trim()))
}

/// Creates `<main repo>/.claude/worktrees/<name>` on a new `claude/<name>` branch from HEAD and
/// makes sure the directory is ignored locally via `info/exclude`.
pub async fn create_claude_worktree(
    start: PathBuf,
    path_env: Option<String>,
) -> Result<CreatedClaudeWorktree> {
    let path_env = path_env.as_deref();
    let main_root = resolve_main_repo_root(&start, path_env).await?;
    let root = worktrees_root(&main_root);
    std::fs::create_dir_all(&root)
        .with_context(|| format!("No se pudo crear {}", root.display()))?;

    let existing_branches = git(
        &main_root,
        &["branch", "--list", "--format=%(refname:short)", "claude/*"],
        path_env,
    )
    .await
    .unwrap_or_default();
    let base_name = worktree_name_for_time(Local::now());
    let name = unique_worktree_name(&base_name, |candidate| {
        let branch = branch_for_worktree(candidate);
        root.join(candidate).exists() || existing_branches.lines().any(|b| b.trim() == branch)
    });
    let branch = branch_for_worktree(&name);
    let path = root.join(&name);

    git(
        &main_root,
        &["worktree", "add", "-b", &branch, path_arg(&path)?],
        path_env,
    )
    .await
    .map_err(|err| anyhow!("git worktree add falló: {err}"))?;

    if let Err(err) = ensure_worktrees_excluded(&main_root, path_env).await {
        log::warn!("Could not add Claude worktrees to info/exclude: {err:#}");
    }

    Ok(CreatedClaudeWorktree { name, branch, path })
}

async fn ensure_worktrees_excluded(main_root: &Path, path_env: Option<&str>) -> Result<()> {
    let exclude = git(
        main_root,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "info/exclude",
        ],
        path_env,
    )
    .await?;
    let exclude = PathBuf::from(exclude.trim());
    let contents = std::fs::read_to_string(&exclude).unwrap_or_default();
    if exclude_has_worktrees_entry(&contents) {
        return Ok(());
    }
    if let Some(parent) = exclude.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let separator = if contents.is_empty() || contents.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let updated = format!("{contents}{separator}{EXCLUDE_PATTERN}\n");
    std::fs::write(&exclude, updated)?;
    Ok(())
}

async fn is_worktree_dirty(path: &Path, path_env: Option<&str>) -> Result<bool> {
    let status = git(path, &["status", "--porcelain"], path_env).await?;
    Ok(!status.trim().is_empty())
}

/// Lists the Claude worktrees of the repository containing `start`.
pub async fn list_claude_worktrees(
    start: PathBuf,
    path_env: Option<String>,
) -> Result<Vec<ClaudeWorktreeStatus>> {
    let path_env = path_env.as_deref();
    let main_root = resolve_main_repo_root(&start, path_env).await?;
    let porcelain = git(&main_root, &["worktree", "list", "--porcelain"], path_env).await?;
    let entries = filter_claude_worktrees(
        parse_worktree_porcelain(&porcelain),
        &worktrees_root(&main_root),
    );
    let mut statuses = Vec::with_capacity(entries.len());
    for entry in entries {
        // A prunable worktree's directory is gone, so there is nothing uncommitted to lose.
        let is_dirty = if entry.prunable {
            false
        } else {
            is_worktree_dirty(&entry.path, path_env)
                .await
                .unwrap_or(true)
        };
        statuses.push(ClaudeWorktreeStatus { entry, is_dirty });
    }
    Ok(statuses)
}

/// Removes the clean Claude worktrees of the repository containing `start`. Worktrees with
/// uncommitted changes, locked ones, and ones that contain any path in `in_use_paths` (e.g. an
/// open tab's working directory) are kept. Removal never passes `--force`.
pub async fn cleanup_claude_worktrees(
    start: PathBuf,
    path_env: Option<String>,
    in_use_paths: Vec<PathBuf>,
) -> Result<CleanupReport> {
    let statuses = list_claude_worktrees(start.clone(), path_env.clone()).await?;
    let path_env = path_env.as_deref();
    let main_root = resolve_main_repo_root(&start, path_env).await?;
    let mut report = CleanupReport::default();
    let mut prunable = Vec::new();
    for status in statuses {
        let name = worktree_display_name(&status.entry.path);
        if status.entry.locked {
            report.kept_locked.push(name);
        } else if in_use_paths
            .iter()
            .any(|in_use| is_path_within(in_use, &status.entry.path))
        {
            report.kept_in_use.push(name);
        } else if status.is_dirty {
            report.kept_dirty.push(name);
        } else if status.entry.prunable {
            // Pruned in bulk below.
            prunable.push(name);
        } else {
            let removed = match path_arg(&status.entry.path) {
                Ok(path) => git(&main_root, &["worktree", "remove", path], path_env)
                    .await
                    .map(|_| ()),
                Err(err) => Err(err),
            };
            match removed {
                Ok(()) => report.removed.push(name),
                Err(err) => {
                    log::warn!("git worktree remove failed for {name}: {err:#}");
                    report.failed.push(name);
                }
            }
        }
    }
    // `git worktree prune` is repository-wide, so it only runs when it would clear one of ours.
    if !prunable.is_empty() {
        match git(&main_root, &["worktree", "prune"], path_env).await {
            Ok(_) => report.removed.extend(prunable),
            Err(err) => {
                log::warn!("git worktree prune failed: {err:#}");
                report.failed.extend(prunable);
            }
        }
    }
    Ok(report)
}
