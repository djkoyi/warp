//! Generates macOS launchd agents so scheduled tasks also run while Warp is closed.

use std::collections::HashSet;
use std::path::Path;

use super::command::{CLAUDE_BINARY, SKIP_PERMISSIONS_FLAG, expand_dir, single_line_prompt};
use super::schedule::{CronField, Schedule};
use super::settings::ScheduledTask;

pub const LABEL_PREFIX: &str = "pa.gob.css.warp.claude-task.";

/// launchd has no cron syntax, so each schedule is expanded into calendar entries.
const MAX_CALENDAR_ENTRIES: usize = 1000;

/// `$1` is the project directory and `$2` the prompt: user text reaches the shell only as
/// positional parameters, never as script text. `--` keeps a prompt starting with `-` from being
/// read as an option.
const RUNNER_SCRIPT: &str = "cd -- \"$1\" && exec \"$3\" \"$4\" -p -- \"$2\"";

/// A plist ready to be written to `~/Library/LaunchAgents`.
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct LaunchAgent {
    pub label: String,
    pub plist: String,
}

#[cfg(target_os = "macos")]
fn launch_agents_dir(home: &Path) -> std::path::PathBuf {
    home.join("Library").join("LaunchAgents")
}

#[cfg(target_os = "macos")]
fn plist_path(home: &Path, label: &str) -> std::path::PathBuf {
    launch_agents_dir(home).join(format!("{label}.plist"))
}

/// Builds one agent per enabled task marked `launchd = true`.
pub fn build_launch_agents(
    tasks: &[ScheduledTask],
    home: &Path,
) -> Result<Vec<LaunchAgent>, String> {
    let mut used_labels = HashSet::new();
    let mut agents = Vec::new();
    for task in tasks.iter().filter(|task| task.enabled && task.launchd) {
        let schedule =
            Schedule::parse(&task.schedule).map_err(|err| format!("{}: {err}", task.name))?;
        let calendar =
            calendar_entries(&schedule).map_err(|err| format!("{}: {err}", task.name))?;
        let base = format!("{LABEL_PREFIX}{}", slug(&task.name));
        let mut label = base.clone();
        let mut suffix = 2;
        while !used_labels.insert(label.clone()) {
            label = format!("{base}-{suffix}");
            suffix += 1;
        }
        let plist = render_plist(&label, task, &calendar, home);
        agents.push(LaunchAgent { label, plist });
    }
    Ok(agents)
}

fn slug(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        "task".to_owned()
    } else {
        slug
    }
}

type CalendarEntry = Vec<(&'static str, u32)>;

/// Expands a schedule into `StartCalendarInterval` dictionaries. Fields allowing every value are
/// omitted, as launchd treats a missing key as "every". launchd ANDs the keys of one dictionary,
/// so when [`Schedule::matches`] ORs the day fields they get separate entries whose union
/// reproduces that rule.
pub fn calendar_entries(schedule: &Schedule) -> Result<Vec<CalendarEntry>, String> {
    let time_fields = [
        ("Minute", &schedule.minute),
        ("Hour", &schedule.hour),
        ("Month", &schedule.month),
    ];
    let day_of_month = ("Day", &schedule.day_of_month);
    let day_of_week = ("Weekday", &schedule.day_of_week);
    let day_variants: Vec<Vec<(&'static str, &CronField)>> =
        if schedule.day_of_month.is_wildcard() || schedule.day_of_week.is_wildcard() {
            vec![vec![day_of_month, day_of_week]]
        } else {
            vec![vec![day_of_month], vec![day_of_week]]
        };

    let mut entries = Vec::new();
    for day_fields in day_variants {
        let mut partial: Vec<CalendarEntry> = vec![vec![]];
        for (key, field) in time_fields.iter().copied().chain(day_fields) {
            if field.is_full() {
                continue;
            }
            let values = field.values();
            if partial.len() * values.len() > MAX_CALENDAR_ENTRIES {
                return Err(format!(
                    "el horario genera más de {MAX_CALENDAR_ENTRIES} entradas para launchd"
                ));
            }
            partial = partial
                .into_iter()
                .flat_map(|entry| {
                    values.iter().map(move |value| {
                        let mut entry = entry.clone();
                        entry.push((key, *value));
                        entry
                    })
                })
                .collect();
        }
        entries.extend(partial);
    }
    Ok(entries)
}

fn xml_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            // XML 1.0 cannot represent other control characters at all.
            c if c.is_control() && !matches!(c, '\t' | '\n' | '\r') => escaped.push(' '),
            c => escaped.push(c),
        }
    }
    escaped
}

fn string_element(text: &str) -> String {
    format!("<string>{}</string>", xml_escape(text))
}

fn render_plist(
    label: &str,
    task: &ScheduledTask,
    calendar: &[CalendarEntry],
    home: &Path,
) -> String {
    let project_dir = expand_dir(&task.project_dir);
    let project_dir = project_dir.to_string_lossy();
    let home_str = home.to_string_lossy();
    let log_path = home
        .join("Library/Logs/warp-claude-tasks")
        .join(format!("{label}.log"));
    let path_env = format!(
        "{home_str}/.local/bin:{home_str}/.claude/local:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
    );
    let arguments = [
        "/bin/zsh",
        "-lc",
        RUNNER_SCRIPT,
        "warp-claude-task",
        &project_dir,
        &single_line_prompt(&task.prompt),
        CLAUDE_BINARY,
        SKIP_PERMISSIONS_FLAG,
    ];

    let mut out = String::new();
    out.push_str(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n",
    );
    out.push_str(&format!(
        "  <key>Label</key>\n  {}\n",
        string_element(label)
    ));
    out.push_str("  <key>ProgramArguments</key>\n  <array>\n");
    for argument in arguments {
        out.push_str(&format!("    {}\n", string_element(argument)));
    }
    out.push_str("  </array>\n");
    out.push_str(&format!(
        "  <key>WorkingDirectory</key>\n  {}\n",
        string_element(&project_dir)
    ));
    out.push_str(&format!(
        "  <key>EnvironmentVariables</key>\n  <dict>\n    <key>PATH</key>\n    {}\n  </dict>\n",
        string_element(&path_env)
    ));
    out.push_str("  <key>StartCalendarInterval</key>\n  <array>\n");
    for entry in calendar {
        out.push_str("    <dict>");
        for (key, value) in entry {
            out.push_str(&format!("<key>{key}</key><integer>{value}</integer>"));
        }
        out.push_str("</dict>\n");
    }
    out.push_str("  </array>\n");
    let log_path = log_path.to_string_lossy();
    out.push_str(&format!(
        "  <key>StandardOutPath</key>\n  {}\n  <key>StandardErrorPath</key>\n  {}\n",
        string_element(&log_path),
        string_element(&log_path)
    ));
    out.push_str("</dict>\n</plist>\n");
    out
}

/// Writes the agents, replaces previously installed ones and bootstraps them into the user's GUI
/// domain with `launchctl`. Returns how many agents were bootstrapped.
#[cfg(target_os = "macos")]
pub fn install(agents: &[LaunchAgent], home: &Path) -> Result<usize, String> {
    use std::process::Command;

    let domain = format!("gui/{}", nix::unistd::getuid());
    let dir = launch_agents_dir(home);
    std::fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let logs = home.join("Library/Logs/warp-claude-tasks");
    std::fs::create_dir_all(&logs).map_err(|err| format!("{}: {err}", logs.display()))?;

    let entries = std::fs::read_dir(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(label) = name
            .strip_suffix(".plist")
            .filter(|label| label.starts_with(LABEL_PREFIX))
        {
            let path = entry.path();
            // A failed bootout just means the agent wasn't loaded.
            let _ = Command::new("launchctl")
                .arg("bootout")
                .arg(format!("{domain}/{label}"))
                .output();
            std::fs::remove_file(&path).map_err(|err| format!("{}: {err}", path.display()))?;
        }
    }

    for agent in agents {
        let path = plist_path(home, &agent.label);
        std::fs::write(&path, &agent.plist).map_err(|err| format!("{}: {err}", path.display()))?;
        // Replaces `load -w`: clears a disabled override left by an earlier `unload -w`.
        let _ = Command::new("launchctl")
            .arg("enable")
            .arg(format!("{domain}/{}", agent.label))
            .output();
        let output = Command::new("launchctl")
            .arg("bootstrap")
            .arg(&domain)
            .arg(&path)
            .output()
            .map_err(|err| format!("launchctl: {err}"))?;
        if !output.status.success() {
            return Err(format!(
                "launchctl bootstrap {domain} {}: {}",
                path.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    Ok(agents.len())
}

#[cfg(test)]
#[path = "launchd_tests.rs"]
mod tests;
