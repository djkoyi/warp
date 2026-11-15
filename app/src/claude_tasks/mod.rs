//! Claude Code helpers for a terminal dedicated to Claude Code sessions: recurring headless jobs
//! (scheduled tasks) and dev-server previews.

pub mod command;
pub mod launchd;
pub mod preview;
pub mod schedule;
pub mod scheduler;
pub mod settings;

use ::settings::{Setting as _, ToggleableSetting as _};
use warpui::keymap::{BindingDescription, EditableBinding};
use warpui::{AppContext, ModelContext, SingletonEntity, ViewContext, WindowId};

use self::preview::{PreviewDetector, latest_dev_server_url, resolve_preview_command};
use self::scheduler::ClaudeTaskScheduler;
pub use self::settings::ClaudeTasksSettings;
use crate::pane_group::Direction;
use crate::util::bindings::BindingGroup;
use crate::view_components::DismissibleToast;
use crate::workspace::{ToastStack, Workspace, WorkspaceAction};

/// Command Palette actions, dispatched through [`WorkspaceAction::ClaudeTasks`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeTasksAction {
    ToggleScheduledTasks,
    ShowScheduledTasks,
    RunScheduledTasksNow,
    InstallLaunchdAgents,
    OpenPreview,
    RunPreviewCommand,
    TogglePreviewDetection,
}

pub fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    app.add_singleton_model(ClaudeTaskScheduler::new);
    app.add_singleton_model(PreviewDetector::new);

    let binding = |name: &'static str, description: BindingDescription, action| {
        EditableBinding::new(name, description, WorkspaceAction::ClaudeTasks(action))
            .with_context_predicate(id!("Workspace"))
            .with_group(BindingGroup::Terminal.as_str())
    };
    app.register_editable_bindings([
        binding(
            "claude_tasks:toggle_scheduled_tasks",
            BindingDescription::new_preserve_case("Tareas programadas: desactivar")
                .with_dynamic_override(|ctx| {
                    (!*ClaudeTasksSettings::as_ref(ctx)
                        .scheduled_tasks_enabled
                        .value())
                    .then(|| "Tareas programadas: activar".to_owned())
                }),
            ClaudeTasksAction::ToggleScheduledTasks,
        ),
        binding(
            "claude_tasks:show_scheduled_tasks",
            BindingDescription::new_preserve_case("Tareas programadas: ver estado"),
            ClaudeTasksAction::ShowScheduledTasks,
        ),
        binding(
            "claude_tasks:run_scheduled_tasks_now",
            BindingDescription::new_preserve_case("Tareas programadas: ejecutar todas ahora"),
            ClaudeTasksAction::RunScheduledTasksNow,
        ),
        binding(
            "claude_tasks:install_launchd_agents",
            BindingDescription::new_preserve_case(
                "Tareas programadas: instalar agentes launchd (con Warp cerrado)",
            ),
            ClaudeTasksAction::InstallLaunchdAgents,
        ),
        binding(
            "claude_tasks:open_preview",
            BindingDescription::new_preserve_case(
                "Vista previa: abrir servidor local del panel en el navegador",
            ),
            ClaudeTasksAction::OpenPreview,
        ),
        binding(
            "claude_tasks:run_preview_command",
            BindingDescription::new_preserve_case(
                "Vista previa: ejecutar servidor del proyecto en panel dividido",
            ),
            ClaudeTasksAction::RunPreviewCommand,
        ),
        binding(
            "claude_tasks:toggle_preview_detection",
            BindingDescription::new_preserve_case("Vista previa: desactivar detección automática")
                .with_dynamic_override(|ctx| {
                    (!*ClaudeTasksSettings::as_ref(ctx)
                        .preview_detection_enabled
                        .value())
                    .then(|| "Vista previa: activar detección automática".to_owned())
                }),
            ClaudeTasksAction::TogglePreviewDetection,
        ),
    ]);
}

pub(crate) fn show_toast<M>(
    toast: DismissibleToast<WorkspaceAction>,
    window_id: WindowId,
    ctx: &mut ModelContext<M>,
) {
    ToastStack::handle(ctx).update(ctx, |stack, ctx| {
        stack.add_ephemeral_toast(toast, window_id, ctx);
    });
}

fn view_toast(toast: DismissibleToast<WorkspaceAction>, ctx: &mut ViewContext<Workspace>) {
    let window_id = ctx.window_id();
    ToastStack::handle(ctx).update(ctx, |stack, ctx| {
        stack.add_ephemeral_toast(toast, window_id, ctx);
    });
}

pub fn handle_workspace_action(
    workspace: &mut Workspace,
    action: ClaudeTasksAction,
    ctx: &mut ViewContext<Workspace>,
) {
    match action {
        ClaudeTasksAction::ToggleScheduledTasks => {
            let result = ClaudeTasksSettings::handle(ctx).update(ctx, |settings, ctx| {
                settings.scheduled_tasks_enabled.toggle_and_save_value(ctx)
            });
            match result {
                Ok(true) => view_toast(
                    DismissibleToast::success("Tareas programadas activadas".to_owned()),
                    ctx,
                ),
                Ok(false) => view_toast(
                    DismissibleToast::default("Tareas programadas desactivadas".to_owned()),
                    ctx,
                ),
                Err(err) => log::error!("Failed to toggle scheduled tasks: {err:#}"),
            }
        }
        ClaudeTasksAction::TogglePreviewDetection => {
            let result = ClaudeTasksSettings::handle(ctx).update(ctx, |settings, ctx| {
                settings
                    .preview_detection_enabled
                    .toggle_and_save_value(ctx)
            });
            if let Err(err) = result {
                log::error!("Failed to toggle preview detection: {err:#}");
            }
        }
        ClaudeTasksAction::ShowScheduledTasks => {
            view_toast(DismissibleToast::default(scheduled_tasks_summary(ctx)), ctx);
        }
        ClaudeTasksAction::RunScheduledTasksNow => {
            let tasks = ClaudeTasksSettings::as_ref(ctx)
                .scheduled_tasks
                .value()
                .clone();
            let started = tasks
                .iter()
                .filter(|task| task.enabled)
                .filter(|task| scheduler::open_task_tab(workspace, task, ctx))
                .count();
            if started == 0 {
                view_toast(
                    DismissibleToast::default(
                        "No hay tareas programadas activas. Agréguelas en [claude_tasks] de \
                         settings.toml."
                            .to_owned(),
                    ),
                    ctx,
                );
            }
        }
        ClaudeTasksAction::InstallLaunchdAgents => install_launchd_agents(ctx),
        ClaudeTasksAction::OpenPreview => {
            let url = workspace
                .active_tab_pane_group()
                .as_ref(ctx)
                .focused_session_view(ctx)
                .and_then(|view| latest_dev_server_url(&view, ctx));
            match url {
                Some(url) => ctx.open_url(&url),
                None => view_toast(
                    DismissibleToast::error(
                        "No se detectó un servidor local (localhost:PUERTO) en este panel."
                            .to_owned(),
                    ),
                    ctx,
                ),
            }
        }
        ClaudeTasksAction::RunPreviewCommand => run_preview_command(workspace, ctx),
    }
}

fn scheduled_tasks_summary(ctx: &AppContext) -> String {
    let settings = ClaudeTasksSettings::as_ref(ctx);
    let tasks = settings.scheduled_tasks.value();
    if tasks.is_empty() {
        return "No hay tareas programadas. Agréguelas como [[claude_tasks.scheduled]] en \
                settings.toml."
            .to_owned();
    }
    let paused = if *settings.scheduled_tasks_enabled.value() {
        ""
    } else {
        " (todas en pausa)"
    };
    let lines: Vec<String> = tasks
        .iter()
        .map(|task| {
            let state = match (task.enabled, task.launchd) {
                (false, _) => "inactiva",
                (true, true) => "launchd",
                (true, false) => "activa",
            };
            let validity = match schedule::Schedule::parse(&task.schedule) {
                Ok(_) => String::new(),
                Err(err) => format!(" - ERROR: {err}"),
            };
            format!("{} [{}] {}{validity}", task.name, task.schedule, state)
        })
        .collect();
    format!("{} tarea(s){paused}: {}", tasks.len(), lines.join(" | "))
}

fn run_preview_command(workspace: &mut Workspace, ctx: &mut ViewContext<Workspace>) {
    let pane_group = workspace.active_tab_pane_group().clone();
    let cwd = pane_group
        .as_ref(ctx)
        .active_session_view(ctx)
        .and_then(|view| view.as_ref(ctx).pwd_if_local(ctx));
    let Some(cwd) = cwd.map(std::path::PathBuf::from) else {
        view_toast(
            DismissibleToast::error("El panel activo no tiene un directorio local.".to_owned()),
            ctx,
        );
        return;
    };
    let configured = ClaudeTasksSettings::as_ref(ctx)
        .preview_commands
        .value()
        .clone();
    let Some((dir, command)) = resolve_preview_command(&configured, &cwd) else {
        view_toast(
            DismissibleToast::error(
                "Sin comando de vista previa para este proyecto: agregue \
                 [[claude_tasks.preview_commands]] en settings.toml o un script \"dev\" en \
                 package.json."
                    .to_owned(),
            ),
            ctx,
        );
        return;
    };
    let shell_command = command::preview_command(&dir, &command);
    pane_group.update(ctx, |pane_group, ctx| {
        let pane_id =
            pane_group.add_terminal_pane_ignoring_default_session_mode(Direction::Right, None, ctx);
        if let Some(view) = pane_group.terminal_view_from_pane_id(pane_id, ctx) {
            view.update(ctx, |view, ctx| {
                view.set_pending_command_queue(vec![shell_command], ctx);
            });
        }
    });
}

fn install_launchd_agents(ctx: &mut ViewContext<Workspace>) {
    let tasks = ClaudeTasksSettings::as_ref(ctx)
        .scheduled_tasks
        .value()
        .clone();
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let agents = match launchd::build_launch_agents(&tasks, &home) {
        Ok(agents) => agents,
        Err(err) => {
            view_toast(DismissibleToast::error(format!("launchd: {err}")), ctx);
            return;
        }
    };

    #[cfg(target_os = "macos")]
    {
        let window_id = ctx.window_id();
        ctx.spawn(
            async move { launchd::install(&agents, &home) },
            move |_, result, ctx| {
                let toast = match result {
                    Ok(0) => DismissibleToast::default(
                        "Agentes launchd eliminados: ninguna tarea tiene launchd = true."
                            .to_owned(),
                    ),
                    Ok(count) => DismissibleToast::success(format!(
                        "{count} agente(s) launchd instalados en ~/Library/LaunchAgents"
                    )),
                    Err(err) => DismissibleToast::error(format!("launchd: {err}")),
                };
                ToastStack::handle(ctx).update(ctx, |stack, ctx| {
                    stack.add_ephemeral_toast(toast, window_id, ctx);
                });
            },
        );
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = agents;
        view_toast(
            DismissibleToast::error("Los agentes launchd solo existen en macOS.".to_owned()),
            ctx,
        );
    }
}
