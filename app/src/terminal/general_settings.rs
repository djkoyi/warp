use std::collections::HashSet;

use warp_core::settings::macros::define_settings_group;
use warp_core::settings::{RespectUserSyncSetting, SupportedPlatforms, SyncToCloud};

use crate::banner::BannerState;
use crate::resource_center::Tip;

define_settings_group!(GeneralSettings, settings: [
    show_warning_before_quitting: ShowWarningBeforeQuitting {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        surface: settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "general.show_warning_before_quitting",
        description: "Whether to show a warning dialog before quitting Warp.",
    },
    quit_on_last_window_closed: QuitOnLastWindowClosed {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::MAC,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        surface: settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "general.quit_on_last_window_closed",
        description: "Whether to quit Warp when the last window is closed.",
    },
    restore_session: RestoreSession {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        surface: settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "general.restore_session",
        description: "Whether to restore the previous session when Warp starts up.",
    },
    claude_code_auto_resume: ClaudeCodeAutoResume {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Never,
        surface: settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "general.claude_code.auto_resume",
        description: "Whether restored panes resume the Claude Code conversation that was running in them.",
    },
    claude_code_resume_ask_first: ClaudeCodeResumeAskFirst {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Never,
        surface: settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "general.claude_code.resume_ask_first",
        description: "Whether to show a one-click banner instead of resuming Claude Code automatically.",
    },
    // Never synced: these flags are placed on a command line that runs on restore.
    claude_code_resume_flags: ClaudeCodeResumeFlags {
        type: String,
        default: "--dangerously-skip-permissions".to_owned(),
        supported_platforms: SupportedPlatforms::DESKTOP,
        sync_to_cloud: SyncToCloud::Never,
        surface: settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "general.claude_code.resume_flags",
        description: "Extra arguments appended to `claude --resume <session>` when restoring a pane.",
    },
    add_app_as_login_item: LoginItem {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::OR(
            Box::new(SupportedPlatforms::MAC),
            Box::new(SupportedPlatforms::WINDOWS),
        ),
        sync_to_cloud: SyncToCloud::Never,
        surface: settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "general.login_item",
        description: "Whether to launch Warp automatically when you log in.",
    },
    // Records whether the app has been added as a login item.
    // If it has, we don't try to add it again unless the user explicitly
    // retoggles the setting. This is to allow a user to remove the login item
    // directly from their OS's startup UI and not have it re-added when they
    // next start Warp.
    app_added_as_login_item: AppAddedAsLoginItem {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::OR(
            Box::new(SupportedPlatforms::MAC),
            Box::new(SupportedPlatforms::WINDOWS),
        ),
        sync_to_cloud: SyncToCloud::Never,
        surface: settings::SettingSurfaces::GUI,
        private: true,
    },
    link_tooltip: LinkTooltip {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        surface: settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "general.link_tooltip",
        description: "Whether to show a tooltip when hovering over links.",
    },
    welcome_tips_features_used: WelcomeTipsFeaturesUsed {
        type: HashSet<Tip>,
        default: HashSet::new(),
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Never,
        surface: settings::SettingSurfaces::GUI,
        private: true,
    },
    welcome_tips_skipped_or_completed: WelcomeTipsCompleted {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        surface: settings::SettingSurfaces::GUI,
        private: true,
    },
    agent_mode_onboarding_block_shown: AgentModeOnboardingBlockShown {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        surface: settings::SettingSurfaces::GUI,
        private: true,
    },
    telemetry_banner_dismissed: TelemetryBannerDismissed {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        surface: settings::SettingSurfaces::GUI,
        private: true,
    },
    user_default_shell_unsupported_banner_state: UserDefaultShellUnsupportedBannerState {
        type: BannerState,
        default: BannerState::default(),
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        surface: settings::SettingSurfaces::GUI,
        private: true,
    },
    open_in_warp_banner_dismissed_for_markdown: OpenInWarpBannerDismissedMarkdown {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Never,
        surface: settings::SettingSurfaces::GUI,
        private: true,
    },
    open_in_warp_banner_dismissed_for_code_and_text: OpenInWarpBannerDismissedCode {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Never,
        surface: settings::SettingSurfaces::GUI,
        private: true,
    },
    did_non_anonymous_user_log_in: DidNonAnonymousUserLogIn {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Never,
        surface: settings::SettingSurfaces::GUI,
        private: true,
    },
    build_plan_migration_modal_dismissed: BuildPlanMigrationModalDismissed {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        surface: settings::SettingSurfaces::GUI,
        private: true,
    },
    // One-time flag tracking whether the OpenWarp launch modal has already been
    // shown to the user. Not user-visible; modeled as a setting so it's only
    // shown once per user regardless of the number of devices they use.
    did_check_to_trigger_openwarp_launch_modal: DidShowOpenWarpLaunchModal {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::No),
        surface: settings::SettingSurfaces::GUI,
        private: true,
    },
    auto_open_code_review_pane_on_first_agent_change: AutoOpenCodeReviewPaneOnFirstAgentChange {
        type: bool,
        default: false,
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        surface: settings::SettingSurfaces::GUI,
        private: false,
        toml_path: "code.editor.auto_open_code_review_pane_on_first_agent_change",
        description: "Whether to automatically open the code review pane when the agent makes its first change.",
    },
    bonus_grants_shown: BonusGrantsShown {
        type: HashSet<String>,
        default: HashSet::new(),
        supported_platforms: SupportedPlatforms::ALL,
        sync_to_cloud: SyncToCloud::Globally(RespectUserSyncSetting::Yes),
        surface: settings::SettingSurfaces::GUI,
        private: true,
    },
]);
