use std::path::{Path, PathBuf};

use chrono::TimeZone as _;

use super::*;

#[test]
fn worktree_name_uses_local_timestamp() {
    let now = Local.with_ymd_and_hms(2026, 11, 15, 14, 30, 12).unwrap();
    assert_eq!(worktree_name_for_time(now), "claude-20261115-143012");
    assert_eq!(
        branch_for_worktree("claude-20261115-143012"),
        "claude/claude-20261115-143012"
    );
}

#[test]
fn unique_worktree_name_appends_first_free_suffix() {
    assert_eq!(unique_worktree_name("a", |_| false), "a");
    let taken = ["a", "a-2"];
    assert_eq!(
        unique_worktree_name("a", |name| taken.contains(&name)),
        "a-3"
    );
}

#[test]
fn parses_worktree_porcelain() {
    let output = "worktree /repo\nHEAD abc\nbranch refs/heads/main\n\n\
                  worktree /repo/.claude/worktrees/one\nHEAD def\nbranch refs/heads/claude/one\nlocked reason\n\n\
                  worktree /repo/.claude/worktrees/two\nHEAD 123\ndetached\nprunable gitdir file points to non-existent location\n";
    let entries = parse_worktree_porcelain(output);
    assert_eq!(
        entries,
        vec![
            WorktreeEntry {
                path: PathBuf::from("/repo"),
                branch: Some("main".to_string()),
                locked: false,
                prunable: false,
            },
            WorktreeEntry {
                path: PathBuf::from("/repo/.claude/worktrees/one"),
                branch: Some("claude/one".to_string()),
                locked: true,
                prunable: false,
            },
            WorktreeEntry {
                path: PathBuf::from("/repo/.claude/worktrees/two"),
                branch: None,
                locked: false,
                prunable: true,
            },
        ]
    );
}

#[test]
fn filter_keeps_only_entries_inside_worktrees_root() {
    let entries = parse_worktree_porcelain(
        "worktree /nonexistent-repo\n\nworktree /nonexistent-repo/.claude/worktrees\n\n\
         worktree /nonexistent-repo/.claude/worktrees/one\n\nworktree /elsewhere/two\n",
    );
    let filtered =
        filter_claude_worktrees(entries, &worktrees_root(Path::new("/nonexistent-repo")));
    assert_eq!(filtered.len(), 1);
    assert_eq!(
        filtered[0].path,
        PathBuf::from("/nonexistent-repo/.claude/worktrees/one")
    );
}

#[test]
fn main_root_comes_from_dot_git_common_dir() {
    assert_eq!(
        main_worktree_root_from_common_dir(Path::new("/repo/.git")),
        Some(PathBuf::from("/repo"))
    );
    assert_eq!(
        main_worktree_root_from_common_dir(Path::new("/srv/repo.git")),
        None
    );
}

#[test]
fn detects_existing_exclude_entry() {
    assert!(exclude_has_worktrees_entry(
        "# comment\n/.claude/worktrees/\n"
    ));
    assert!(exclude_has_worktrees_entry("  .claude/worktrees  \n"));
    assert!(!exclude_has_worktrees_entry(
        "/.claude/\n.claude/worktrees/sub\n"
    ));
}

#[test]
fn summaries_mention_each_worktree() {
    let status = |name: &str, is_dirty| ClaudeWorktreeStatus {
        entry: WorktreeEntry {
            path: PathBuf::from(format!("/repo/.claude/worktrees/{name}")),
            ..Default::default()
        },
        is_dirty,
    };
    assert_eq!(
        summarize_worktrees(&[status("a", false), status("b", true)]),
        "Worktrees de Claude (2): a (limpio), b (con cambios)"
    );
    let report = CleanupReport {
        removed: vec!["a".to_string()],
        kept_dirty: vec!["b".to_string()],
        ..Default::default()
    };
    assert_eq!(
        summarize_cleanup(&report),
        "Limpieza de worktrees de Claude. Eliminados: 1. Con cambios (conservados): b. \
         Las ramas claude/* se conservan."
    );
}
