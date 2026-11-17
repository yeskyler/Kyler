//! Five-field schedules. UTC persistence, with explicit DST gap/overlap rules.
use crate::invalid;

use anyhow::{Context, Result, ensure};
use chrono::{DateTime, LocalResult, TimeZone, Utc};
use chrono_tz::Tz;
use croner::Cron;

pub fn next(cron: &str, timezone: &str, after_ms: i64) -> Result<i64> {
    ensure!(
        cron.len() <= 128 && cron.split_whitespace().count() == 5,
        invalid("Use a five-field Cron expression (minute hour day month weekday).")
    );
    let cron: Cron = cron
        .parse()
        .map_err(|_| invalid("Invalid Cron expression."))?;
    let zone: Tz = timezone
        .parse()
        .map_err(|_| invalid("Use an IANA time zone, such as Asia/Shanghai."))?;
    let mut after = DateTime::<Utc>::from_timestamp_millis(after_ms)
        .context("Invalid schedule reference time")?
        .with_timezone(&zone);
    for _ in 0..180 {
        let candidate = cron
            .find_next_occurrence(&after, false)
            .map_err(|_| invalid("This schedule has no reachable next occurrence."))?;
        let matches = cron
            .is_time_matching(&candidate)
            .map_err(|_| invalid("Cannot evaluate this schedule."))?;
        let first_occurrence = match zone.from_local_datetime(&candidate.naive_local()) {
            LocalResult::Ambiguous(first, second) => candidate == first.min(second),
            LocalResult::Single(_) => true,
            LocalResult::None => false,
        };
        if matches && first_occurrence && candidate.timestamp_millis() > after_ms {
            return Ok(candidate.timestamp_millis());
        }
        after = candidate;
    }
    Err(invalid("Cannot find an unambiguous schedule occurrence."))
}
pub fn preview(cron: &str, timezone: &str, after_ms: i64) -> Result<Vec<i64>> {
    let mut after = after_ms;
    let mut values = Vec::new();
    for _ in 0..5 {
        after = next(cron, timezone, after)?;
        values.push(after);
    }
    Ok(values)
}
