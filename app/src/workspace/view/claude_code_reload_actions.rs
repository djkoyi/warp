use warpui::{AppContext, SingletonEntity as _, ViewContext, ViewHandle};

use super::Workspace;
use crate::terminal::TerminalView;
use crate::terminal::claude_code_reload::ClaudeCodeReloadSummary;
use crate::view_components::{DismissibleToast, ToastLink};
use crate::workspace::{ToastStack, WorkspaceAction, WorkspaceRegistry};

impl Workspace {
    pub(super) fn reload_claude_in_all_tabs(&mut self, force: bool, ctx: &mut ViewContext<Self>) {
        let current_window_id = ctx.window_id();
        // This workspace is mid-update, so it is read through `self`: reading it through its
        // handle from the registry would panic.
        let mut terminal_views = self.window_terminal_views(ctx);
        for (window_id, workspace) in WorkspaceRegistry::as_ref(ctx).all_workspaces(ctx) {
            if window_id != current_window_id {
                terminal_views.extend(workspace.as_ref(ctx).window_terminal_views(ctx));
            }
        }
        Self::reload_claude_in_terminal_views(
            terminal_views,
            WorkspaceAction::ForceReloadClaudeInAllTabs,
            force,
            ctx,
        );
    }

    pub(super) fn reload_claude_in_active_tab(&mut self, force: bool, ctx: &mut ViewContext<Self>) {
        let terminal_views = self.active_tab_pane_group().as_ref(ctx).terminal_views(ctx);
        Self::reload_claude_in_terminal_views(
            terminal_views,
            WorkspaceAction::ForceReloadClaudeInActiveTab,
            force,
            ctx,
        );
    }

    fn window_terminal_views(&self, ctx: &AppContext) -> Vec<ViewHandle<TerminalView>> {
        self.tabs
            .iter()
            .flat_map(|tab| tab.pane_group.as_ref(ctx).terminal_views(ctx))
            .collect()
    }

    fn reload_claude_in_terminal_views(
        mut terminal_views: Vec<ViewHandle<TerminalView>>,
        force_action: WorkspaceAction,
        force: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        // Forcing right after a reload must not restart the sessions that already reloaded, so
        // it only finishes the pending ones while there are any.
        if force
            && terminal_views
                .iter()
                .any(|terminal_view| terminal_view.as_ref(ctx).is_claude_code_reload_pending())
        {
            terminal_views
                .retain(|terminal_view| terminal_view.as_ref(ctx).is_claude_code_reload_pending());
        }
        let mut summary = ClaudeCodeReloadSummary::default();
        for terminal_view in terminal_views {
            let decision = terminal_view.update(ctx, |terminal_view, ctx| {
                terminal_view.reload_claude_code_session(force, ctx)
            });
            summary.record(&decision);
        }

        let message = summary.message();
        let mut toast =
            if summary.skipped_count() == 0 && summary.pending == 0 && summary.reloaded > 0 {
                DismissibleToast::success(message)
            } else {
                DismissibleToast::default(message)
            };
        if summary.pending > 0 {
            toast = toast.with_link(
                ToastLink::new("Forzar recarga".to_owned()).with_onclick_action(force_action),
            );
        }
        let window_id = ctx.window_id();
        ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
            toast_stack.add_ephemeral_toast(toast, window_id, ctx);
        });
    }
}
