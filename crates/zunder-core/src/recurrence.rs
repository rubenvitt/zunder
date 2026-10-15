//! Calendar of recurring tasks: the due date (and reminder) of the next
//! instance of a series (contract 3.1, "Termin der Folgeinstanz").
//!
//! The rules work on local dates and local times of the server's time zone.
//! The crate has no time zone database: the caller supplies the zone as a
//! [`LocalZone`] (the UTC offset in effect at an instant; the server answers
//! it with `chrono::Local`), and this module turns local times into instants
//! with the contract's rules for the daylight saving gap (later by the length
//! of the gap) and overlap (the earlier instant).
//!
//! - `from: "due"` is a fixed calendar anchored at the old due date D0. The
//!   next date is the earliest date of the series after D0 that is not
//!   before today: missed dates are skipped, exactly one instance follows.
//! - `from: "done"` counts from the local completion date and never adjusts
//!   to today.
//! - A day of the month beyond the month's length gives its last day; the
//!   configured day (`day`, or for monthly and yearly series without it the
//!   day of D0, which the server then writes into the new instance's
//!   recurrence) keeps the series from drifting. Yearly series use the month
//!   of D0 and at least one year: a 28.02. stays a 28.02., a 29.02. falls
//!   on the 28.02. in common years and comes back in leap years, `"last"`
//!   is the February last. An instance moved into another month takes the
//!   series along.
//!
//! Only the server creates follow-up instances; clients never compute them.
//! Which tasks a push completed and what their next instance carries:
//! [`CompletionsBefore`].

use time::{Date, Duration, Month, OffsetDateTime, PrimitiveDateTime, UtcOffset};

use crate::model::{Due, MonthDay, Recurrence, RecurrenceFrom, RecurrenceUnit};

mod follow;

pub use follow::{CompletionsBefore, FollowUp, FollowUps};

/// A time zone as far as the calendar needs it: the UTC offset in effect at
/// an instant. Closures `Fn(OffsetDateTime) -> UtcOffset` are zones, too.
pub trait LocalZone {
    /// UTC offset in effect at `instant`.
    fn offset_at(&self, instant: OffsetDateTime) -> UtcOffset;
}

impl<F: Fn(OffsetDateTime) -> UtcOffset> LocalZone for F {
    fn offset_at(&self, instant: OffsetDateTime) -> UtcOffset {
        self(instant)
    }
}

/// A zone with one offset all year (no daylight saving).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixedZone(pub UtcOffset);

impl LocalZone for FixedZone {
    fn offset_at(&self, _instant: OffsetDateTime) -> UtcOffset {
        self.0
    }
}

/// What the server writes into the next instance of a series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextOccurrence {
    /// New due date: a plain date if the old one was, otherwise the same
    /// local time on the new date with the zone's offset then.
    pub due: Due,
    /// New reminder, if the predecessor had one: moved by as many calendar
    /// days as the due date, at the same local time.
    pub remind_at: Option<OffsetDateTime>,
    /// Recurrence of the new instance: unchanged, except that a monthly or
    /// yearly `from: "due"` series without `day` gets the day of the old due
    /// date.
    pub recurrence: Recurrence,
}

/// The next instance of a series whose current instance (due `due`,
/// reminder `remind_at`) was completed at `completed_at`, with `today` the
/// server's local date. `None` if a date would lie beyond the year 9999
/// (the server then creates no instance).
pub fn next_occurrence(
    recurrence: &Recurrence,
    due: Due,
    remind_at: Option<OffsetDateTime>,
    completed_at: OffsetDateTime,
    today: Date,
    zone: &impl LocalZone,
) -> Option<NextOccurrence> {
    let local_due = match due {
        Due::Date(d) => LocalDue::Date(d),
        Due::DateTime(dt) => LocalDue::DateTime(to_local(dt, zone)?),
    };
    let d0 = local_due.date();
    let done_on = to_local(completed_at, zone)?.date();
    let date = next_due_date(recurrence, d0, done_on, today)?;
    let new_due = match local_due {
        LocalDue::Date(_) => Due::Date(date),
        LocalDue::DateTime(local) => Due::DateTime(resolve_local(local.replace_date(date), zone)?),
    };
    let remind_at = match remind_at {
        None => None,
        Some(remind) => {
            let local = to_local(remind, zone)?;
            let shifted = local.date().checked_add(date - d0)?;
            Some(resolve_local(local.replace_date(shifted), zone)?)
        }
    };
    Some(NextOccurrence {
        due: new_due,
        remind_at,
        recurrence: series_recurrence(recurrence, d0),
    })
}

/// The recurrence a new instance carries: a monthly or yearly `from: "due"`
/// series without `day` gets the day of D0, so it does not drift (31.01. →
/// 28.02. → 31.03.; yearly 29.02. → 28.02. → 29.02. in the next leap year,
/// while a 28.02. stays a 28.02.). Everything else stays as it is.
pub fn series_recurrence(recurrence: &Recurrence, d0: Date) -> Recurrence {
    let mut out = recurrence.clone();
    if out.from == RecurrenceFrom::Due
        && matches!(out.unit, RecurrenceUnit::Month | RecurrenceUnit::Year)
        && out.day.is_none()
    {
        out.day = Some(MonthDay::Day(d0.day()));
    }
    out
}

/// The next due date of a series on local dates: `d0` is the old due date,
/// `done_on` the completion date, `today` the server's date. `None` beyond
/// the year 9999.
pub fn next_due_date(
    recurrence: &Recurrence,
    d0: Date,
    done_on: Date,
    today: Date,
) -> Option<Date> {
    match recurrence.from {
        RecurrenceFrom::Due => next_from_due(recurrence, d0, today),
        RecurrenceFrom::Done => next_from_done(recurrence, done_on),
    }
}

/// Fixed calendar: the earliest date of the series after `d0` that is not
/// before `today`.
fn next_from_due(r: &Recurrence, d0: Date, today: Date) -> Option<Date> {
    let every = i64::from(r.every);
    // Earliest admissible date.
    let lower = d0.next_day()?.max(today);
    match r.unit {
        RecurrenceUnit::Day => step_days(d0, every, lower),
        RecurrenceUnit::Week => match &r.weekdays {
            None => step_days(d0, 7 * every, lower),
            Some(weekdays) => {
                let mut offsets: Vec<i64> = weekdays
                    .iter()
                    .map(|w| i64::from(w.weekday().number_days_from_monday()))
                    .collect();
                offsets.sort_unstable();
                let week0 = monday(d0);
                let weeks = (monday(lower) - week0).whole_days() / 7;
                let mut k = div_ceil(weeks, every).max(0);
                // The week of `lower` may hold no named day on or after it;
                // then the next week of the series does.
                loop {
                    let week = week0.checked_add(Duration::weeks(k * every))?;
                    for &offset in &offsets {
                        let day = week.checked_add(Duration::days(offset))?;
                        if day >= lower {
                            return Some(day);
                        }
                    }
                    k += 1;
                }
            }
        },
        RecurrenceUnit::Month | RecurrenceUnit::Year => {
            let step = months_per_step(r);
            let day = r.day.unwrap_or(MonthDay::Day(d0.day()));
            let months = month_index(lower) - month_index(d0);
            // Months may stay in D0's own month (k = 0: the configured day
            // lies later in it); years always move on by at least one step,
            // also when D0 was moved off the configured day.
            let min_k = i64::from(r.unit == RecurrenceUnit::Year);
            let mut k = div_ceil(months, step).max(min_k);
            loop {
                let candidate = day_in_month(month_index(d0) + k * step, day)?;
                if candidate >= lower {
                    return Some(candidate);
                }
                k += 1;
            }
        }
    }
}

/// `d0 + k · step` days for the smallest k ≥ 1 that is not before `lower`.
fn step_days(d0: Date, step: i64, lower: Date) -> Option<Date> {
    let k = div_ceil((lower - d0).whole_days(), step).max(1);
    d0.checked_add(Duration::days(k.checked_mul(step)?))
}

/// n units after the completion date `done`.
fn next_from_done(r: &Recurrence, done: Date) -> Option<Date> {
    let every = i64::from(r.every);
    match r.unit {
        RecurrenceUnit::Day => done.checked_add(Duration::days(every)),
        RecurrenceUnit::Week => {
            let target = done.checked_add(Duration::weeks(every))?;
            match &r.weekdays {
                None => Some(target),
                Some(weekdays) => {
                    let first = weekdays
                        .iter()
                        .map(|w| w.weekday().number_days_from_monday())
                        .min()?;
                    monday(target).checked_add(Duration::days(i64::from(first)))
                }
            }
        }
        RecurrenceUnit::Month | RecurrenceUnit::Year => {
            let day = r.day.unwrap_or(MonthDay::Day(done.day()));
            day_in_month(month_index(done) + months_per_step(r), day)
        }
    }
}

/// Months between two dates of a monthly or yearly series.
fn months_per_step(r: &Recurrence) -> i64 {
    let every = i64::from(r.every);
    if r.unit == RecurrenceUnit::Year {
        12 * every
    } else {
        every
    }
}

/// The day `day` in the month with index `year · 12 + month − 1`, clamped
/// to the month's length. `None` outside the years 0–9999.
fn day_in_month(index: i64, day: MonthDay) -> Option<Date> {
    let year = i32::try_from(index.div_euclid(12)).ok()?;
    if !(0..=9999).contains(&year) {
        return None;
    }
    let month = Month::try_from(u8::try_from(index.rem_euclid(12) + 1).ok()?).ok()?;
    let length = month.length(year);
    let day = match day {
        MonthDay::Last => length,
        MonthDay::Day(d) => d.min(length),
    };
    Date::from_calendar_date(year, month, day).ok()
}

fn month_index(d: Date) -> i64 {
    i64::from(d.year()) * 12 + i64::from(u8::from(d.month())) - 1
}

/// The Monday of the week (Monday to Sunday) of `d`.
fn monday(d: Date) -> Date {
    d.checked_sub(Duration::days(i64::from(
        d.weekday().number_days_from_monday(),
    )))
    // Only the first days of the representable range have no Monday; the
    // year bounds of the calendar exclude them anyway.
    .unwrap_or(d)
}

/// Ceiling division for a positive divisor.
fn div_ceil(n: i64, d: i64) -> i64 {
    n.div_euclid(d) + i64::from(n.rem_euclid(d) != 0)
}

/// Local due: a plain date or a local date and time.
#[derive(Clone, Copy)]
enum LocalDue {
    Date(Date),
    DateTime(PrimitiveDateTime),
}

impl LocalDue {
    fn date(self) -> Date {
        match self {
            LocalDue::Date(d) => d,
            LocalDue::DateTime(dt) => dt.date(),
        }
    }
}

/// Local date and time of `instant` in `zone`. `None` at the edge of the
/// representable range.
pub fn to_local(instant: OffsetDateTime, zone: &impl LocalZone) -> Option<PrimitiveDateTime> {
    let local = instant.checked_to_offset(zone.offset_at(instant))?;
    Some(PrimitiveDateTime::new(local.date(), local.time()))
}

/// The instant of a local date and time in `zone`, written with the offset
/// in effect then. A local time that does not exist (spring gap) counts as
/// later by the length of the gap (02:30 → 03:30); one that exists twice
/// (autumn overlap) is the earlier instant. Assumes at most one offset
/// change within a day around `local`.
pub fn resolve_local(local: PrimitiveDateTime, zone: &impl LocalZone) -> Option<OffsetDateTime> {
    let probe = local.assume_utc();
    let before = zone.offset_at(probe.checked_sub(Duration::DAY).unwrap_or(probe));
    let after = zone.offset_at(probe.checked_add(Duration::DAY).unwrap_or(probe));
    let valid = [before, after]
        .into_iter()
        .map(|offset| local.assume_offset(offset))
        .filter(|candidate| zone.offset_at(*candidate) == candidate.offset())
        .min();
    // In the gap no offset fits: the offset before the change puts the
    // instant after it, at the local time shifted by the gap.
    let instant = valid.unwrap_or_else(|| local.assume_offset(before));
    let resolved = instant.checked_to_offset(zone.offset_at(instant))?;
    (0..=9999).contains(&resolved.year()).then_some(resolved)
}

#[cfg(test)]
mod tests;
