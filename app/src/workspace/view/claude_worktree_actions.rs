use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use futures::future::BoxFuture;
use warpui::{AppContext, SingletonEntity as _, ViewContext};

use super::Workspace;
use crate::launch_configs::launch_config::{CommandTemplate, PaneMode, PaneTemplateType};
use crate::pane_group::PanesLayout;
#[cfg(feature = "local_tty")]
use crate::terminal::local_shell::LocalShellState;
use crate::view_components::DismissibleToast;
use crate::workspace::claude_worktrees::{
    self, CLAUDE_WORKTREE_SESSION_COMMAND, CreatedClaudeWorktree,
};
use crate::workspace::{ToastStack, WorkspaceAction, WorkspaceRegistry};

const NO_LOCAL_REPO_MESSAGE: &str = "La pestaña activa no tiene un directorio local. Abre una pestaña dentro de un repositorio git.";

impl Workspace {
    pub(super) fn new_claude_worktree_session(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(start) = self.active_local_session_path(ctx) else {
            Self::show_claude_worktree_toast(
                DismissibleToast::error(NO_LOCAL_REPO_MESSAGE.into()),
                ctx,
            );
            return;
        };
        let path_env = Self::interactive_path_env(ctx);
        ctx.spawn(
            async move { claude_worktrees::create_claude_worktree(start, path_env.await).await },
            |me, result, ctx| match result {
                Ok(created) => me.open_claude_worktree_tab(created, ctx),
                Err(err) => Self::show_claude_worktree_toast(
                    DismissibleToast::error(format!("No se pudo crear el worktree: {err:#}")),
                    ctx,
                ),
            },
        );
    }

    pub(super) fn list_claude_worktrees(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(start) = self.active_local_session_path(ctx) else {
            Self::show_claude_worktree_toast(
                DismissibleToast::error(NO_LOCAL_REPO_MESSAGE.into()),
                ctx,
            );
            return;
        };
        let path_env = Self::interactive_path_env(ctx);
        ctx.spawn(
            async move { claude_worktrees::list_claude_worktrees(start, path_env.await).await },
            |_, result, ctx| {
                let toast = match result {
                    Ok(statuses) => {
                        DismissibleToast::default(claude_worktrees::summarize_worktrees(&statuses))
                    }
                    Err(err) => DismissibleToast::error(format!(
                        "No se pudieron listar los worktrees: {err:#}"
                    )),
                };
                Self::show_claude_worktree_toast(toast, ctx);
            },
        );
    }

    pub(super) fn cleanup_claude_worktrees(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(start) = self.active_local_session_path(ctx) else {
            Self::show_claude_worktree_toast(
                DismissibleToast::error(NO_LOCAL_REPO_MESSAGE.into()),
                ctx,
            );
            return;
        };
        let in_use_paths = self.open_local_session_paths(ctx);
        let path_env = Self::interactive_path_env(ctx);
        ctx.spawn(
            async move {
                claude_worktrees::cleanup_claude_worktrees(start, path_env.await, in_use_paths)
                    .await
            },
            |_, result, ctx| {
                let toast = match result {
                    Ok(report) => {
                        DismissibleToast::success(claude_worktrees::summarize_cleanup(&report))
                    }
                    Err(err) => DismissibleToast::error(format!(
                        "No se pudieron limpiar los worktrees: {err:#}"
                    )),
                };
                Self::show_claude_worktree_toast(toast, ctx);
            },
        );
    }

    fn open_claude_worktree_tab(
        &mut self,
        created: CreatedClaudeWorktree,
        ctx: &mut ViewContext<Self>,
    ) {
        let template = PaneTemplateType::PaneTemplate {
            cwd: created.path,
            commands: vec![CommandTemplate::from(CLAUDE_WORKTREE_SESSION_COMMAND)],
            is_focused: Some(true),
            pane_mode: PaneMode::Terminal,
            shell: None,
        };
        self.add_tab_with_pane_layout(
            PanesLayout::Template(template),
            Arc::new(HashMap::new()),
            Some(format!("Claude · {}", created.name)),
            ctx,
        );
        ctx.dispatch_global_action("workspace:save_app", ());
        Self::show_claude_worktree_toast(
            DismissibleToast::success(format!(
                "Worktree {} creado en la rama {}",
                created.name, created.branch
            )),
            ctx,
        );
    }

    /// Working directory of the active tab's focused terminal, when it is local.
    fn active_local_session_path(&self, ctx: &AppContext) -> Option<PathBuf> {
        self.active_tab_pane_group()
            .as_ref(ctx)
            .active_session_view(ctx)
            .and_then(|terminal_view| terminal_view.as_ref(ctx).active_session_path_if_local(ctx))
    }

    /// Local working directories of every terminal in every window's tabs.
    fn open_local_session_paths(&self, ctx: &ViewContext<Self>) -> Vec<PathBuf> {
        let current_window_id = ctx.window_id();
        // This workspace is mid-update, so it is read through `self`: reading it through its
        // handle from the registry would panic.
        let mut paths = self.window_local_session_paths(ctx);
        for (window_id, workspace) in WorkspaceRegistry::as_ref(ctx).all_workspaces(ctx) {
            if window_id != current_window_id {
                paths.extend(workspace.as_ref(ctx).window_local_session_paths(ctx));
            }
        }
        paths
    }

    fn window_local_session_paths(&self, ctx: &AppContext) -> Vec<PathBuf> {
        self.tabs
            .iter()
            .flat_map(|tab| tab.pane_group.as_ref(ctx).terminal_views(ctx))
            .filter_map(|terminal_view| terminal_view.as_ref(ctx).active_session_path_if_local(ctx))
            .collect()
    }

    /// The interactive shell's `PATH`, so git hooks (e.g. git-lfs) resolve the same binaries as
    /// in a terminal even when Warp was launched from the macOS GUI.
    fn interactive_path_env(ctx: &mut ViewContext<Self>) -> BoxFuture<'static, Option<String>> {
        #[cfg(feature = "local_tty")]
        {
            LocalShellState::handle(ctx).update(ctx, |shell_state, ctx| {
                shell_state.get_interactive_path_env_var(ctx)
            })
        }
        #[cfg(not(feature = "local_tty"))]
        {
            let _ = ctx;
            futures::FutureExt::boxed(futures::future::ready(None))
        }
    }

    fn show_claude_worktree_toast(
        toast: DismissibleToast<WorkspaceAction>,
        ctx: &mut ViewContext<Self>,
    ) {
        let window_id = ctx.window_id();
        ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
            toast_stack.add_ephemeral_toast(toast, window_id, ctx);
        });
    }
}
