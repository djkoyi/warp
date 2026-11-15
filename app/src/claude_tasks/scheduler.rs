//! Runs due scheduled tasks in new tabs while Warp is open.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use ::settings::Setting as _;
use chrono::{DateTime, Local};
use warpui::r#async::Timer;
use warpui::{Entity, ModelContext, SingletonEntity, ViewContext, ViewHandle, WindowId};

use super::command::{claude_task_command, expand_dir};
use super::schedule::{Schedule, ScheduleTime};
use super::settings::{ClaudeTasksSettings, ScheduledTask};
use super::{show_toast, view_toast};
use crate::launch_configs::launch_config::{CommandTemplate, PaneMode, PaneTemplateType};
use crate::pane_group::PanesLayout;
use crate::view_components::DismissibleToast;
use crate::workspace::{Workspace, WorkspaceAction, WorkspaceRegistry};

/// Shorter than a minute so every calendar minute is observed at least once.
const TICK_INTERVAL: Duration = Duration::from_secs(20);

/// Most minutes a late tick (e.g. after the machine slept) evaluates, counting the current one.
const MAX_CATCH_UP_MINUTES: i64 = 10;

pub struct ClaudeTaskScheduler {
    /// Minute (Unix minutes) each task last fired in, so a minute observed twice fires once.
    last_fired: HashMap<String, i64>,
    /// Minute (Unix minutes) of the previous tick, so minutes skipped by a late timer still fire.
    last_tick_minute: Option<i64>,
    reported_invalid: HashSet<String>,
}

impl ClaudeTaskScheduler {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        Self::schedule_tick(ctx);
        Self {
            last_fired: HashMap::new(),
            last_tick_minute: None,
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
        let minute = Local::now().timestamp().div_euclid(60);
        let first_minute = pending_minutes_start(self.last_tick_minute, minute);
        self.last_tick_minute = Some(minute);

        let settings = ClaudeTasksSettings::as_ref(ctx);
        if !*settings.scheduled_tasks_enabled.value() {
            return;
        }
        let tasks = settings.scheduled_tasks.value().clone();
        let times: Vec<(i64, ScheduleTime)> = (first_minute..=minute)
            .filter_map(|minute| {
                let time = DateTime::from_timestamp(minute * 60, 0)?.with_timezone(&Local);
                Some((minute, ScheduleTime::from_datetime(&time)))
            })
            .collect();

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
            let last_fired = self.last_fired.get(&task.key()).copied();
            // Several skipped minutes may match; the task still runs only once per tick.
            let Some(due_minute) = times
                .iter()
                .rev()
                .find(|(minute, time)| {
                    schedule.matches(time) && last_fired.is_none_or(|last| last < *minute)
                })
                .map(|(minute, _)| *minute)
            else {
                continue;
            };
            self.last_fired.insert(task.key(), due_minute);
            run_task(task, ctx);
        }
    }
}

/// First minute a tick at `minute` must evaluate: the ones since the previous tick, capped at
/// [`MAX_CATCH_UP_MINUTES`]. A clock that moved backwards evaluates only the current minute.
pub(super) fn pending_minutes_start(last_tick_minute: Option<i64>, minute: i64) -> i64 {
    match last_tick_minute {
        Some(last) if last < minute => (last + 1).max(minute - MAX_CATCH_UP_MINUTES + 1),
        Some(_) | None => minute,
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

fn run_task(task: &ScheduledTask, ctx: &mut ModelContext<ClaudeTaskScheduler>) {
    let Some((_, workspace)) = target_window(ctx) else {
        log::warn!(
            "Scheduled task {:?} skipped: no Warp window is open",
            task.name
        );
        return;
    };
    workspace.update(ctx, |workspace, ctx| {
        open_task_tab(workspace, task, ctx);
    });
}

/// Opens a tab in the task's project and runs it there. Returns whether the tab was opened.
///
/// Takes the workspace directly so callers already inside a `Workspace` update (e.g. its own
/// action handler) never update it again through its handle, which panics.
pub(super) fn open_task_tab(
    workspace: &mut Workspace,
    task: &ScheduledTask,
    ctx: &mut ViewContext<Workspace>,
) -> bool {
    let dir = expand_dir(&task.project_dir);
    if !dir.is_dir() {
        log::warn!(
            "Scheduled task {:?} skipped: missing project dir",
            task.name
        );
        view_toast(
            DismissibleToast::error(format!(
                "Tarea \"{}\" omitida: no existe {}",
                task.name,
                dir.display()
            )),
            ctx,
        );
        return false;
    }

    let command = claude_task_command(&dir, &task.prompt);
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
        Some(format!("Tarea: {}", task.name)),
        ctx,
    );
    if !task.focus
        && let Some(index) = (0..workspace.tab_count()).find(|index| {
            workspace
                .get_pane_group_view(*index)
                .is_some_and(|pane_group| pane_group.id() == previous_tab)
        })
    {
        workspace.activate_tab(index, ctx);
    }
    view_toast(
        DismissibleToast::success(format!("Tarea programada iniciada: {}", task.name)),
        ctx,
    );
    true
}

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
