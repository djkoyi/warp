use std::path::Path;

use super::*;

fn task(name: &str, schedule: &str) -> ScheduledTask {
    ScheduledTask {
        name: name.to_owned(),
        project_dir: "/Users/fr/proyectos/sipe".to_owned(),
        prompt: "Revisa <logs> & \"resume\"\nhoy".to_owned(),
        schedule: schedule.to_owned(),
        enabled: true,
        focus: false,
        launchd: true,
    }
}

#[test]
fn daily_schedule_is_one_entry() {
    let entries = calendar_entries(&Schedule::parse("08:30").unwrap()).unwrap();
    assert_eq!(entries, vec![vec![("Minute", 30), ("Hour", 8)]]);
}

#[test]
fn weekdays_expand_per_day() {
    let entries = calendar_entries(&Schedule::parse("weekdays 07:00").unwrap()).unwrap();
    assert_eq!(entries.len(), 5);
    assert_eq!(entries[0], vec![("Minute", 0), ("Hour", 7), ("Weekday", 1)]);
}

#[test]
fn both_day_fields_produce_union() {
    let entries = calendar_entries(&Schedule::parse("0 12 1 * 1").unwrap()).unwrap();
    assert_eq!(
        entries,
        vec![
            vec![("Minute", 0), ("Hour", 12), ("Day", 1)],
            vec![("Minute", 0), ("Hour", 12), ("Weekday", 1)],
        ]
    );
}

#[test]
fn too_many_entries_is_an_error() {
    assert!(calendar_entries(&Schedule::parse("*/1 */1 * * *").unwrap()).is_err());
}

#[test]
fn plist_escapes_and_passes_prompt_as_argument() {
    let agents = build_launch_agents(
        &[
            task("Resumen diario!", "08:30"),
            task("resumen diario", "09:00"),
        ],
        Path::new("/Users/fr"),
    )
    .unwrap();
    assert_eq!(agents.len(), 2);
    assert_eq!(agents[0].label, format!("{LABEL_PREFIX}resumen-diario"));
    assert_eq!(agents[1].label, format!("{LABEL_PREFIX}resumen-diario-2"));
    let plist = &agents[0].plist;
    assert!(plist.contains("<string>Revisa &lt;logs&gt; &amp; &quot;resume&quot; hoy</string>"));
    assert!(plist.contains("<string>/Users/fr/proyectos/sipe</string>"));
    assert!(plist.contains("<string>--dangerously-skip-permissions</string>"));
    assert!(plist.contains(
        "<dict><key>Minute</key><integer>30</integer><key>Hour</key><integer>8</integer></dict>"
    ));
    assert!(!plist.contains("Revisa <logs>"));
}

#[test]
fn only_enabled_launchd_tasks_get_agents() {
    let mut in_warp = task("a", "08:00");
    in_warp.launchd = false;
    let mut disabled = task("b", "08:00");
    disabled.enabled = false;
    let agents = build_launch_agents(&[in_warp, disabled], Path::new("/Users/fr")).unwrap();
    assert!(agents.is_empty());
}

#[test]
fn invalid_schedule_names_the_task() {
    let err = build_launch_agents(&[task("roto", "nunca")], Path::new("/h")).unwrap_err();
    assert!(err.starts_with("roto:"));
}
