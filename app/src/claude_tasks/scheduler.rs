//! Runs due scheduled tasks in new tabs while Warp is open.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use ::settings::Setting as _;
use chrono::Local;
use warpui::r#async::Timer;
use warpui::{Entity, ModelContext, SingletonEntity, ViewHandle, WindowId};

use super::command::{claude_task_command, expand_dir};
use super::schedule::{Schedule, ScheduleTime};
use super::settings::{ClaudeTasksSettings, ScheduledTask};
use super::show_toast;
use crate::launch_configs::launch_config::{CommandTemplate, PaneMode, PaneTemplateType};
use crate::pane_group::PanesLayout;
use crate::view_components::DismissibleToast;
use crate::workspace::{Workspace, WorkspaceAction, WorkspaceRegistry};

/// Shorter than a minute so every calendar minute is observed at least once.
const TICK_INTERVAL: Duration = Duration::from_secs(20);

pub struct ClaudeTaskScheduler {
    /// Minute (Unix minutes) each task last fired in, so a minute observed twice fires once.
    last_fired: HashMap<String, i64>,
    reported_invalid: HashSet<String>,
}

impl ClaudeTaskScheduler {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        Self::schedule_tick(ctx);
        Self {
            last_fired: HashMap::new(),
            reported_invalid: HashSet::new(),
        }
    }

    fn schedule_tick(ctx: &mut ModelContext<Self>) {
        ctx.spawn(async { Timer::after(TICK_INTERVAL).await }, |me, _, ctx| {
            me.tick(ctx);
            Self::schedule_tick(ctx);
        });
    }

    fn tick(&mut self, ctx: &mut ModelContext<Self>) {
        let settings = ClaudeTasksSettings::as_ref(ctx);
        if !*settings.scheduled_tasks_enabled.value() {
            return;
        }
        let tasks = settings.scheduled_tasks.value().clone();
        let now = Local::now();
        let minute = now.timestamp().div_euclid(60);
        let time = ScheduleTime::from_datetime(&now);

        for task in tasks.iter().filter(|task| task.enabled && !task.launchd) {
            let schedule = match Schedule::parse(&task.schedule) {
                Ok(schedule) => schedule,
                Err(err) => {
                    if self.reported_invalid.insert(task.key()) {
                        log::warn!(
                            "Scheduled task {:?} has an invalid schedule: {err}",
                            task.name
                        );
                        notify(
                            DismissibleToast::error(format!(
                                "Tarea programada \"{}\": {err}",
                                task.name
                            )),
                            ctx,
                        );
                    }
                    continue;
                }
            };
            if !schedule.matches(&time) || self.last_fired.get(&task.key()) == Some(&minute) {
                continue;
            }
            self.last_fired.insert(task.key(), minute);
            run_task(task, ctx);
        }
    }

    /// Runs every enabled task now, regardless of schedule. Returns how many were started.
    pub fn run_all_now(&mut self, ctx: &mut ModelContext<Self>) -> usize {
        let tasks = ClaudeTasksSettings::as_ref(ctx)
            .scheduled_tasks
            .value()
            .clone();
        tasks
            .iter()
            .filter(|task| task.enabled)
            .filter(|task| run_task(task, ctx))
            .count()
    }
}

impl Entity for ClaudeTaskScheduler {
    type Event = ();
}

impl SingletonEntity for ClaudeTaskScheduler {}

fn target_window(
    ctx: &ModelContext<ClaudeTaskScheduler>,
) -> Option<(WindowId, ViewHandle<Workspace>)> {
    let registry = WorkspaceRegistry::as_ref(ctx);
    ctx.windows()
        .active_window()
        .and_then(|window_id| Some((window_id, registry.get(window_id, ctx)?)))
        .or_else(|| registry.all_workspaces(ctx).into_iter().next())
}

fn notify(toast: DismissibleToast<WorkspaceAction>, ctx: &mut ModelContext<ClaudeTaskScheduler>) {
    if let Some((window_id, _)) = target_window(ctx) {
        show_toast(toast, window_id, ctx);
    }
}

/// Opens a tab in the task's project and runs it there. Returns whether the tab was opened.
fn run_task(task: &ScheduledTask, ctx: &mut ModelContext<ClaudeTaskScheduler>) -> bool {
    let dir = expand_dir(&task.project_dir);
    if !dir.is_dir() {
        log::warn!(
            "Scheduled task {:?} skipped: missing project dir",
            task.name
        );
        notify(
            DismissibleToast::error(format!(
                "Tarea \"{}\" omitida: no existe {}",
                task.name,
                dir.display()
            )),
            ctx,
        );
        return false;
    }
    let Some((window_id, workspace)) = target_window(ctx) else {
        log::warn!(
            "Scheduled task {:?} skipped: no Warp window is open",
            task.name
        );
        return false;
    };

    let command = claude_task_command(&dir, &task.prompt);
    let focus = task.focus;
    let title = format!("Tarea: {}", task.name);
    workspace.update(ctx, |workspace, ctx| {
        let previous_tab = workspace.active_tab_pane_group().id();
        workspace.add_tab_with_pane_layout(
            PanesLayout::Template(PaneTemplateType::PaneTemplate {
                cwd: dir,
                commands: vec![CommandTemplate { exec: command }],
                is_focused: Some(true),
                pane_mode: PaneMode::Terminal,
                shell: None,
            }),
            Arc::new(HashMap::new()),
            Some(title),
            ctx,
        );
        if !focus
            && let Some(index) = (0..workspace.tab_count()).find(|index| {
                workspace
                    .get_pane_group_view(*index)
                    .is_some_and(|pane_group| pane_group.id() == previous_tab)
            })
        {
            workspace.activate_tab(index, ctx);
        }
    });
    show_toast(
        DismissibleToast::success(format!("Tarea programada iniciada: {}", task.name)),
        window_id,
        ctx,
    );
    true
}
