use serde::{Deserialize, Serialize};
use settings::macros::define_settings_group;
use settings::{SupportedPlatforms, SyncToCloud};

fn default_true() -> bool {
    true
}

/// A recurring headless Claude Code job.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(description = "A recurring Claude Code job run in its own tab.")]
pub struct ScheduledTask {
    #[schemars(description = "Unique, human-readable name of the task.")]
    pub name: String,
    #[schemars(description = "Project directory the task runs in. `~` is expanded.")]
    pub project_dir: String,
    #[schemars(description = "Prompt passed to `claude -p`. Newlines are sent as spaces.")]
    pub prompt: String,
    #[schemars(
        description = "When to run: `HH:MM` (daily), `weekdays HH:MM`, or a 5-field cron \
                       expression (`minute hour day-of-month month day-of-week`)."
    )]
    pub schedule: String,
    #[serde(default = "default_true")]
    #[schemars(description = "Whether the task runs on schedule.")]
    pub enabled: bool,
    #[serde(default)]
    #[schemars(description = "Whether the new tab takes focus when the task starts.")]
    pub focus: bool,
    #[serde(default)]
    #[schemars(
        description = "Run through a macOS launchd agent instead of inside Warp, so it also runs \
                       while Warp is closed. Install it with the Command Palette."
    )]
    pub launchd: bool,
}

impl ScheduledTask {
    /// Identifies the task across settings reloads.
    pub fn key(&self) -> String {
        format!("{}\u{0}{}", self.name, self.project_dir)
    }
}

impl settings_value::SettingsValue for ScheduledTask {}

/// A per-project command that starts the app's dev server.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(description = "Command that starts a project's dev server for preview.")]
pub struct PreviewCommand {
    #[schemars(description = "Project root. `~` is expanded. Applies to its subdirectories too.")]
    pub project_dir: String,
    #[schemars(description = "Shell command to run, e.g. `npm run dev`.")]
    pub command: String,
}

impl settings_value::SettingsValue for PreviewCommand {}

define_settings_group!(ClaudeTasksSettings, settings: [
    scheduled_tasks_enabled: ScheduledTasksEnabled {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Never,
        surface: settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "claude_tasks.enabled",
        description: "Whether scheduled Claude Code tasks run while Warp is open.",
    },
    scheduled_tasks: ScheduledTasks {
        type: Vec<ScheduledTask>,
        default: Vec::new(),
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Never,
        surface: settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "claude_tasks.scheduled",
        description: "Recurring Claude Code jobs, each opened in a new tab when due.",
    },
    preview_detection_enabled: PreviewDetectionEnabled {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Never,
        surface: settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "claude_tasks.preview_detection",
        description: "Whether to offer opening local dev-server URLs printed in the focused pane.",
    },
    preview_commands: PreviewCommands {
        type: Vec<PreviewCommand>,
        default: Vec::new(),
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Never,
        surface: settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "claude_tasks.preview_commands",
        description: "Per-project commands that start a dev server in a split pane.",
    },
]);
