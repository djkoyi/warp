use super::*;

fn at(minute: u32, hour: u32, day_of_month: u32, month: u32, day_of_week: u32) -> ScheduleTime {
    ScheduleTime {
        minute,
        hour,
        day_of_month,
        month,
        day_of_week,
    }
}

#[test]
fn daily_time_matches_only_that_minute() {
    for spec in ["08:30", "daily 08:30", "diario 8:30"] {
        let schedule = Schedule::parse(spec).unwrap();
        assert!(schedule.matches(&at(30, 8, 15, 6, 3)), "{spec}");
        assert!(!schedule.matches(&at(31, 8, 15, 6, 3)), "{spec}");
        assert!(!schedule.matches(&at(30, 20, 15, 6, 3)), "{spec}");
    }
}

#[test]
fn weekdays_skip_weekend() {
    let schedule = Schedule::parse("weekdays 07:05").unwrap();
    assert!(schedule.matches(&at(5, 7, 1, 1, 1)));
    assert!(schedule.matches(&at(5, 7, 1, 1, 5)));
    assert!(!schedule.matches(&at(5, 7, 1, 1, 6)));
    assert!(!schedule.matches(&at(5, 7, 1, 1, 0)));
}

#[test]
fn cron_steps_ranges_and_lists() {
    let schedule = Schedule::parse("*/15 9-17 * * 1,3,5").unwrap();
    assert_eq!(schedule.minute.values(), vec![0, 15, 30, 45]);
    assert_eq!(schedule.hour.values(), (9..=17).collect::<Vec<_>>());
    assert!(schedule.matches(&at(45, 17, 10, 4, 3)));
    assert!(!schedule.matches(&at(45, 18, 10, 4, 3)));
    assert!(!schedule.matches(&at(45, 17, 10, 4, 2)));
    assert!(!schedule.matches(&at(44, 17, 10, 4, 3)));
}

#[test]
fn cron_sunday_as_seven() {
    let schedule = Schedule::parse("0 0 * * 7").unwrap();
    assert_eq!(schedule.day_of_week.values(), vec![0]);
    assert!(schedule.matches(&at(0, 0, 2, 2, 0)));
}

#[test]
fn cron_restricted_day_fields_are_ored() {
    let schedule = Schedule::parse("0 12 1 * 1").unwrap();
    assert!(
        schedule.matches(&at(0, 12, 1, 3, 4)),
        "day of month matches"
    );
    assert!(schedule.matches(&at(0, 12, 9, 3, 1)), "day of week matches");
    assert!(!schedule.matches(&at(0, 12, 9, 3, 4)));
}

#[test]
fn cron_stepped_wildcard_day_field_is_anded() {
    // `*/2` counts as `*` for the OR rule, as in Vixie cron: odd days that are also Mondays.
    let schedule = Schedule::parse("0 12 */2 * 1").unwrap();
    assert!(schedule.matches(&at(0, 12, 3, 3, 1)));
    assert!(!schedule.matches(&at(0, 12, 4, 3, 1)), "even day");
    assert!(!schedule.matches(&at(0, 12, 3, 3, 2)), "not Monday");
}

#[test]
fn cron_start_with_step() {
    let schedule = Schedule::parse("5/20 * * * *").unwrap();
    assert_eq!(schedule.minute.values(), vec![5, 25, 45]);
}

#[test]
fn invalid_specs_are_rejected() {
    for spec in [
        "",
        "25:00",
        "8:60",
        "mañana 08:00",
        "* * * *",
        "60 * * * *",
        "*/0 * * * *",
        "5-1 * * * *",
        "0 0 0 * *",
        "a b c d e",
    ] {
        assert!(
            Schedule::parse(spec).is_err(),
            "{spec:?} should be rejected"
        );
    }
}
