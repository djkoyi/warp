//! Parsing and matching of scheduled-task schedules.

use chrono::{Datelike, Timelike};

/// A calendar minute, in local time, that a schedule is evaluated against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScheduleTime {
    pub minute: u32,
    pub hour: u32,
    pub day_of_month: u32,
    pub month: u32,
    /// 0 = Sunday.
    pub day_of_week: u32,
}

impl ScheduleTime {
    pub fn from_datetime<T: Datelike + Timelike>(time: &T) -> Self {
        Self {
            minute: time.minute(),
            hour: time.hour(),
            day_of_month: time.day(),
            month: time.month(),
            day_of_week: time.weekday().num_days_from_sunday(),
        }
    }
}

/// One cron field: the set of allowed values as a bitmask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CronField {
    bits: u64,
    /// Written starting with `*` (`*` or `*/n`). Vixie cron uses this, not the allowed values, to
    /// pick between OR and AND for the two day fields.
    is_wildcard: bool,
    /// Allows every value of the field's range, so launchd can omit the key.
    is_full: bool,
}

impl CronField {
    fn single(value: u32) -> Self {
        Self {
            bits: 1 << value,
            is_wildcard: false,
            is_full: false,
        }
    }

    fn wildcard(min: u32, max: u32) -> Self {
        Self {
            bits: range_bits(min, max, 1),
            is_wildcard: true,
            is_full: true,
        }
    }

    pub fn contains(&self, value: u32) -> bool {
        value < 64 && self.bits & (1 << value) != 0
    }

    pub fn is_wildcard(&self) -> bool {
        self.is_wildcard
    }

    pub fn is_full(&self) -> bool {
        self.is_full
    }

    /// Allowed values in ascending order.
    pub fn values(&self) -> Vec<u32> {
        (0..64).filter(|value| self.contains(*value)).collect()
    }
}

/// A parsed schedule, normalized to cron semantics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Schedule {
    pub minute: CronField,
    pub hour: CronField,
    pub day_of_month: CronField,
    pub month: CronField,
    pub day_of_week: CronField,
}

impl Schedule {
    /// Accepts `HH:MM` (daily), `daily HH:MM` / `diario HH:MM`, `weekdays HH:MM` /
    /// `laborables HH:MM`, or a 5-field cron expression.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let spec = spec.trim();
        let words: Vec<&str> = spec.split_whitespace().collect();
        match words.as_slice() {
            [time] if time.contains(':') => Self::at_time(time, CronField::wildcard(0, 6)),
            [prefix, time] => match prefix.to_ascii_lowercase().as_str() {
                "daily" | "diario" | "diaria" => Self::at_time(time, CronField::wildcard(0, 6)),
                "weekdays" | "laborables" => Self::at_time(
                    time,
                    CronField {
                        bits: range_bits(1, 5, 1),
                        is_wildcard: false,
                        is_full: false,
                    },
                ),
                _ => Err(format!("horario no reconocido: \"{spec}\"")),
            },
            [minute, hour, day_of_month, month, day_of_week] => {
                let mut day_of_week = parse_field(day_of_week, 0, 7)?;
                // Cron allows both 0 and 7 for Sunday.
                if day_of_week.contains(7) {
                    day_of_week.bits = (day_of_week.bits & !(1 << 7)) | 1;
                }
                day_of_week.is_full = day_of_week.bits == range_bits(0, 6, 1);
                Ok(Self {
                    minute: parse_field(minute, 0, 59)?,
                    hour: parse_field(hour, 0, 23)?,
                    day_of_month: parse_field(day_of_month, 1, 31)?,
                    month: parse_field(month, 1, 12)?,
                    day_of_week,
                })
            }
            _ => Err(format!(
                "horario no reconocido: \"{spec}\" (use HH:MM, \"weekdays HH:MM\" o cron de 5 campos)"
            )),
        }
    }

    fn at_time(time: &str, day_of_week: CronField) -> Result<Self, String> {
        let (hour, minute) = time
            .split_once(':')
            .ok_or_else(|| format!("hora inválida: \"{time}\""))?;
        let hour: u32 = hour
            .parse()
            .ok()
            .filter(|hour| *hour <= 23)
            .ok_or_else(|| format!("hora inválida: \"{time}\""))?;
        let minute: u32 = minute
            .parse()
            .ok()
            .filter(|minute| *minute <= 59)
            .ok_or_else(|| format!("hora inválida: \"{time}\""))?;
        Ok(Self {
            minute: CronField::single(minute),
            hour: CronField::single(hour),
            day_of_month: CronField::wildcard(1, 31),
            month: CronField::wildcard(1, 12),
            day_of_week,
        })
    }

    pub fn matches(&self, time: &ScheduleTime) -> bool {
        if !self.minute.contains(time.minute)
            || !self.hour.contains(time.hour)
            || !self.month.contains(time.month)
        {
            return false;
        }
        let dom = self.day_of_month.contains(time.day_of_month);
        let dow = self.day_of_week.contains(time.day_of_week);
        // Vixie cron: when neither day field starts with `*`, either one may match; otherwise
        // both must.
        if self.day_of_month.is_wildcard() || self.day_of_week.is_wildcard() {
            dom && dow
        } else {
            dom || dow
        }
    }
}

fn range_bits(min: u32, max: u32, step: u32) -> u64 {
    (min..=max)
        .step_by(step as usize)
        .fold(0, |bits, value| bits | (1 << value))
}

fn parse_number(text: &str, min: u32, max: u32) -> Result<u32, String> {
    text.parse::<u32>()
        .ok()
        .filter(|value| (min..=max).contains(value))
        .ok_or_else(|| format!("valor \"{text}\" fuera de rango {min}-{max}"))
}

fn parse_field(field: &str, min: u32, max: u32) -> Result<CronField, String> {
    if field == "*" {
        return Ok(CronField::wildcard(min, max));
    }
    let mut bits = 0;
    for part in field.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => {
                let step = step
                    .parse::<u32>()
                    .ok()
                    .filter(|step| *step > 0)
                    .ok_or_else(|| format!("paso inválido en \"{part}\""))?;
                (range, step)
            }
            None => (part, 1),
        };
        let (start, end) = if range == "*" {
            (min, max)
        } else if let Some((start, end)) = range.split_once('-') {
            let start = parse_number(start, min, max)?;
            let end = parse_number(end, min, max)?;
            if start > end {
                return Err(format!("rango invertido en \"{part}\""));
            }
            (start, end)
        } else {
            let start = parse_number(range, min, max)?;
            // `5/15` means "from 5 to the max, every 15", as in Vixie cron.
            let end = if part.contains('/') { max } else { start };
            (start, end)
        };
        bits |= range_bits(start, end, step);
    }
    Ok(CronField {
        bits,
        is_wildcard: field.starts_with('*'),
        is_full: bits == range_bits(min, max, 1),
    })
}

#[cfg(test)]
#[path = "schedule_tests.rs"]
mod tests;
