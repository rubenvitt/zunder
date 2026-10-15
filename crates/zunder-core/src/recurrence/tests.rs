use super::*;
use time::macros::{date, datetime, offset};
use time::{Month, UtcOffset};

use crate::model::{MonthDay, RecurrenceFrom, RecurrenceUnit, WeekdayCode};

/// Europe/Berlin without a time zone database: CET (+01:00), CEST (+02:00)
/// from the last Sunday of March to the last Sunday of October, switching at
/// 01:00 UTC (EU rule since 1996).
struct Berlin;

fn last_sunday(year: i32, month: Month) -> Date {
    let last = Date::from_calendar_date(year, month, month.length(year)).unwrap();
    last - Duration::days(i64::from(last.weekday().number_days_from_sunday()))
}

impl LocalZone for Berlin {
    fn offset_at(&self, instant: OffsetDateTime) -> UtcOffset {
        let utc = instant.to_offset(UtcOffset::UTC);
        let year = utc.year();
        let start = last_sunday(year, Month::March)
            .with_hms(1, 0, 0)
            .unwrap()
            .assume_utc();
        let end = last_sunday(year, Month::October)
            .with_hms(1, 0, 0)
            .unwrap()
            .assume_utc();
        if start <= utc && utc < end {
            offset!(+2)
        } else {
            offset!(+1)
        }
    }
}

const UTC: FixedZone = FixedZone(UtcOffset::UTC);

fn rec(every: u16, unit: RecurrenceUnit, from: RecurrenceFrom) -> Recurrence {
    Recurrence {
        every,
        unit,
        weekdays: None,
        day: None,
        from,
    }
}

fn monthly(every: u16, day: Option<MonthDay>, from: RecurrenceFrom) -> Recurrence {
    Recurrence {
        day,
        ..rec(every, RecurrenceUnit::Month, from)
    }
}

fn yearly(every: u16, day: Option<MonthDay>, from: RecurrenceFrom) -> Recurrence {
    Recurrence {
        day,
        ..rec(every, RecurrenceUnit::Year, from)
    }
}

fn weekly(every: u16, weekdays: &[WeekdayCode], from: RecurrenceFrom) -> Recurrence {
    Recurrence {
        weekdays: Some(weekdays.to_vec()),
        ..rec(every, RecurrenceUnit::Week, from)
    }
}

use RecurrenceFrom::{Done as FromDone, Due as FromDue};
use WeekdayCode::{Mo, Th};

/// Next due date of a date-only series completed on time (on `d0`, today
/// = `d0`).
fn on_time(r: &Recurrence, d0: Date) -> Date {
    next_due_date(r, d0, d0, d0).unwrap()
}

/// Follows a date-only series `n` times, each instance completed on its
/// due date, carrying the written recurrence along.
fn chain(r: &Recurrence, d0: Date, n: usize) -> Vec<Date> {
    let mut r = r.clone();
    let mut due = d0;
    let mut out = Vec::new();
    for _ in 0..n {
        let next = next_occurrence(&r, Due::Date(due), None, at_noon(due), due, &UTC).unwrap();
        let Due::Date(d) = next.due else {
            panic!("date stays a date")
        };
        out.push(d);
        due = d;
        r = next.recurrence;
    }
    out
}

fn at_noon(d: Date) -> OffsetDateTime {
    d.with_hms(12, 0, 0).unwrap().assume_utc()
}

// --- month ends ---------------------------------------------------------

#[test]
fn day_31_clamps_to_short_months_without_drift() {
    let r = monthly(1, Some(MonthDay::Day(31)), FromDue);
    assert_eq!(
        chain(&r, date!(2027 - 01 - 31), 4),
        [
            date!(2027 - 02 - 28),
            date!(2027 - 03 - 31),
            date!(2027 - 04 - 30),
            date!(2027 - 05 - 31)
        ]
    );
}

#[test]
fn day_31_in_april_is_the_30th() {
    let r = monthly(1, Some(MonthDay::Day(31)), FromDue);
    assert_eq!(on_time(&r, date!(2027 - 03 - 31)), date!(2027 - 04 - 30));
}

#[test]
fn missing_day_is_taken_from_the_old_due_and_written() {
    let r = monthly(1, None, FromDue);
    let next = next_occurrence(
        &r,
        Due::Date(date!(2027 - 01 - 31)),
        None,
        at_noon(date!(2027 - 01 - 31)),
        date!(2027 - 01 - 31),
        &UTC,
    )
    .unwrap();
    assert_eq!(next.due, Due::Date(date!(2027 - 02 - 28)));
    assert_eq!(next.recurrence.day, Some(MonthDay::Day(31)));
    // The written day keeps the series on the 31st.
    assert_eq!(
        chain(&r, date!(2027 - 01 - 31), 3),
        [
            date!(2027 - 02 - 28),
            date!(2027 - 03 - 31),
            date!(2027 - 04 - 30)
        ]
    );
}

#[test]
fn present_day_and_other_units_keep_the_recurrence_unchanged() {
    let cases = [
        monthly(1, Some(MonthDay::Day(15)), FromDue),
        monthly(1, Some(MonthDay::Last), FromDue),
        monthly(1, None, FromDone),
        yearly(1, Some(MonthDay::Day(31)), FromDue),
        yearly(1, Some(MonthDay::Last), FromDue),
        yearly(1, None, FromDone),
        rec(2, RecurrenceUnit::Day, FromDue),
        weekly(1, &[Mo, Th], FromDue),
    ];
    for r in cases {
        let d0 = date!(2027 - 01 - 31);
        let next = next_occurrence(&r, Due::Date(d0), None, at_noon(d0), d0, &UTC).unwrap();
        assert_eq!(next.recurrence, r);
    }
}

#[test]
fn month_last_over_a_leap_year() {
    let r = monthly(1, Some(MonthDay::Last), FromDue);
    assert_eq!(
        chain(&r, date!(2027 - 12 - 31), 14),
        [
            date!(2028 - 01 - 31),
            date!(2028 - 02 - 29),
            date!(2028 - 03 - 31),
            date!(2028 - 04 - 30),
            date!(2028 - 05 - 31),
            date!(2028 - 06 - 30),
            date!(2028 - 07 - 31),
            date!(2028 - 08 - 31),
            date!(2028 - 09 - 30),
            date!(2028 - 10 - 31),
            date!(2028 - 11 - 30),
            date!(2028 - 12 - 31),
            date!(2029 - 01 - 31),
            date!(2029 - 02 - 28),
        ]
    );
}

#[test]
fn month_last_from_a_due_in_mid_month_snaps_to_that_months_end() {
    // k = 0 counts: the last day of D0's own month lies after D0.
    let r = monthly(1, Some(MonthDay::Last), FromDue);
    assert_eq!(on_time(&r, date!(2026 - 10 - 15)), date!(2026 - 10 - 31));
}

// --- 28 and 29 February -------------------------------------------------

#[test]
fn yearly_february_28_stays_february_28_over_eight_years() {
    // Contract 3.1: no drift onto the 29th in leap years, wherever the series
    // starts (common year or leap year).
    let r = rec(1, RecurrenceUnit::Year, FromDue);
    assert_eq!(
        chain(&r, date!(2027 - 02 - 28), 8),
        [
            date!(2028 - 02 - 28),
            date!(2029 - 02 - 28),
            date!(2030 - 02 - 28),
            date!(2031 - 02 - 28),
            date!(2032 - 02 - 28),
            date!(2033 - 02 - 28),
            date!(2034 - 02 - 28),
            date!(2035 - 02 - 28)
        ]
    );
    assert_eq!(
        chain(&r, date!(2024 - 02 - 28), 8),
        [
            date!(2025 - 02 - 28),
            date!(2026 - 02 - 28),
            date!(2027 - 02 - 28),
            date!(2028 - 02 - 28),
            date!(2029 - 02 - 28),
            date!(2030 - 02 - 28),
            date!(2031 - 02 - 28),
            date!(2032 - 02 - 28)
        ]
    );
}

#[test]
fn yearly_february_29_falls_on_the_28th_and_comes_back_in_leap_years() {
    let r = rec(1, RecurrenceUnit::Year, FromDue);
    assert_eq!(
        chain(&r, date!(2028 - 02 - 29), 8),
        [
            date!(2029 - 02 - 28),
            date!(2030 - 02 - 28),
            date!(2031 - 02 - 28),
            date!(2032 - 02 - 29),
            date!(2033 - 02 - 28),
            date!(2034 - 02 - 28),
            date!(2035 - 02 - 28),
            date!(2036 - 02 - 29)
        ]
    );
}

#[test]
fn yearly_without_day_writes_the_day_of_the_old_due() {
    // Stored series from before `day` was allowed for years ({unit: year}
    // without day) keep working: the server takes the day of D0 and writes
    // it into the next instance.
    let r = rec(1, RecurrenceUnit::Year, FromDue);
    let d0 = date!(2028 - 02 - 29);
    let next = next_occurrence(&r, Due::Date(d0), None, at_noon(d0), d0, &UTC).unwrap();
    assert_eq!(next.due, Due::Date(date!(2029 - 02 - 28)));
    assert_eq!(next.recurrence, yearly(1, Some(MonthDay::Day(29)), FromDue));
    let d0 = date!(2027 - 02 - 28);
    let next = next_occurrence(&r, Due::Date(d0), None, at_noon(d0), d0, &UTC).unwrap();
    assert_eq!(next.due, Due::Date(date!(2028 - 02 - 28)));
    assert_eq!(next.recurrence.day, Some(MonthDay::Day(28)));
}

#[test]
fn yearly_last_is_the_february_last() {
    let r = yearly(1, Some(MonthDay::Last), FromDue);
    assert_eq!(
        chain(&r, date!(2027 - 02 - 28), 3),
        [
            date!(2028 - 02 - 29),
            date!(2029 - 02 - 28),
            date!(2030 - 02 - 28)
        ]
    );
}

#[test]
fn yearly_day_applies_in_the_month_of_the_old_due() {
    let r = yearly(2, Some(MonthDay::Day(31)), FromDue);
    assert_eq!(on_time(&r, date!(2026 - 04 - 30)), date!(2028 - 04 - 30));
    // k = 0 counts as with months: the 20th of D0's own month lies after D0.
    let r = yearly(1, Some(MonthDay::Day(20)), FromDue);
    assert_eq!(on_time(&r, date!(2026 - 03 - 10)), date!(2026 - 03 - 20));
    assert_eq!(on_time(&r, date!(2026 - 03 - 20)), date!(2027 - 03 - 20));
}

#[test]
fn yearly_other_days_keep_month_and_day() {
    let r = rec(2, RecurrenceUnit::Year, FromDue);
    assert_eq!(on_time(&r, date!(2026 - 10 - 12)), date!(2028 - 10 - 12));
}

#[test]
fn monthly_february_29_without_day_writes_29() {
    let r = monthly(1, None, FromDue);
    let d0 = date!(2028 - 02 - 29);
    let next = next_occurrence(&r, Due::Date(d0), None, at_noon(d0), d0, &UTC).unwrap();
    assert_eq!(next.due, Due::Date(date!(2028 - 03 - 29)));
    assert_eq!(next.recurrence.day, Some(MonthDay::Day(29)));
}

#[test]
fn yearly_from_done_with_day_uses_the_month_of_the_completion() {
    let r = yearly(1, Some(MonthDay::Last), FromDone);
    let d0 = date!(2026 - 06 - 01);
    let done = date!(2027 - 02 - 10);
    assert_eq!(
        next_due_date(&r, d0, done, done),
        Some(date!(2028 - 02 - 29))
    );
    // Nothing is written for `from: "done"`.
    let r = rec(1, RecurrenceUnit::Year, FromDone);
    let next = next_occurrence(&r, Due::Date(d0), None, at_noon(done), done, &UTC).unwrap();
    assert_eq!(next.due, Due::Date(date!(2028 - 02 - 10)));
    assert_eq!(next.recurrence, r);
}

#[test]
fn yearly_from_done_on_february_29_lands_on_february_28() {
    let r = rec(1, RecurrenceUnit::Year, FromDone);
    assert_eq!(
        next_due_date(
            &r,
            date!(2028 - 02 - 29),
            date!(2028 - 02 - 29),
            date!(2028 - 02 - 29)
        ),
        Some(date!(2029 - 02 - 28))
    );
}

// --- weekdays -----------------------------------------------------------

#[test]
fn monday_and_thursday_weekly() {
    let r = weekly(1, &[Th, Mo], FromDue);
    // 2026-10-12 is a Monday.
    assert_eq!(
        chain(&r, date!(2026 - 10 - 12), 4),
        [
            date!(2026 - 10 - 15),
            date!(2026 - 10 - 19),
            date!(2026 - 10 - 22),
            date!(2026 - 10 - 26)
        ]
    );
}

#[test]
fn monday_and_thursday_every_second_week() {
    let r = weekly(2, &[Mo, Th], FromDue);
    assert_eq!(
        chain(&r, date!(2026 - 10 - 12), 4),
        [
            date!(2026 - 10 - 15),
            date!(2026 - 10 - 26),
            date!(2026 - 10 - 29),
            date!(2026 - 11 - 09)
        ]
    );
}

#[test]
fn weekdays_lock_onto_the_weeks_of_d0_even_off_pattern() {
    // D0 is a Wednesday; Thursday of the same week is the next date, then
    // the Monday two weeks after D0's week.
    let r = weekly(2, &[Mo, Th], FromDue);
    assert_eq!(
        chain(&r, date!(2026 - 10 - 14), 2),
        [date!(2026 - 10 - 15), date!(2026 - 10 - 26)]
    );
}

// --- intervals > 1 ------------------------------------------------------

#[test]
fn intervals_greater_than_one() {
    assert_eq!(
        on_time(&rec(3, RecurrenceUnit::Day, FromDue), date!(2026 - 10 - 30)),
        date!(2026 - 11 - 02)
    );
    assert_eq!(
        on_time(
            &rec(2, RecurrenceUnit::Week, FromDue),
            date!(2026 - 10 - 15)
        ),
        date!(2026 - 10 - 29)
    );
    assert_eq!(
        on_time(
            &monthly(2, Some(MonthDay::Day(15)), FromDue),
            date!(2026 - 11 - 15)
        ),
        date!(2027 - 01 - 15)
    );
    assert_eq!(
        on_time(
            &monthly(3, Some(MonthDay::Last), FromDue),
            date!(2026 - 11 - 30)
        ),
        date!(2027 - 02 - 28)
    );
}

// --- missed dates -------------------------------------------------------

#[test]
fn missed_month_ends_give_one_instance_not_before_today() {
    let r = monthly(1, Some(MonthDay::Last), FromDue);
    assert_eq!(
        next_due_date(
            &r,
            date!(2026 - 07 - 31),
            date!(2026 - 10 - 15),
            date!(2026 - 10 - 15)
        ),
        Some(date!(2026 - 10 - 31))
    );
}

#[test]
fn missed_daily_gives_today() {
    let r = rec(1, RecurrenceUnit::Day, FromDue);
    assert_eq!(
        next_due_date(
            &r,
            date!(2026 - 10 - 08),
            date!(2026 - 10 - 15),
            date!(2026 - 10 - 15)
        ),
        Some(date!(2026 - 10 - 15))
    );
}

#[test]
fn missed_with_interval_skips_to_the_grid() {
    // Every 3 days from 01.10.: 04., 07., 10., 13., 16. → first not before
    // the 15th is the 16th.
    let r = rec(3, RecurrenceUnit::Day, FromDue);
    assert_eq!(
        next_due_date(
            &r,
            date!(2026 - 10 - 01),
            date!(2026 - 10 - 15),
            date!(2026 - 10 - 15)
        ),
        Some(date!(2026 - 10 - 16))
    );
    // Weekly on Tuesdays from 01.09.2026 (a Tuesday).
    let r = rec(1, RecurrenceUnit::Week, FromDue);
    assert_eq!(
        next_due_date(
            &r,
            date!(2026 - 09 - 01),
            date!(2026 - 10 - 15),
            date!(2026 - 10 - 15)
        ),
        Some(date!(2026 - 10 - 20))
    );
}

#[test]
fn missed_weekdays_and_years() {
    let r = weekly(2, &[Mo, Th], FromDue);
    // Weeks of 12.10., 26.10., 09.11. …; today Tuesday 27.10. → Thu 29.10.
    assert_eq!(
        next_due_date(
            &r,
            date!(2026 - 10 - 12),
            date!(2026 - 10 - 27),
            date!(2026 - 10 - 27)
        ),
        Some(date!(2026 - 10 - 29))
    );
    let r = rec(1, RecurrenceUnit::Year, FromDue);
    assert_eq!(
        next_due_date(
            &r,
            date!(2020 - 02 - 29),
            date!(2026 - 10 - 15),
            date!(2026 - 10 - 15)
        ),
        Some(date!(2027 - 02 - 28))
    );
}

#[test]
fn early_completion_gives_the_next_date_after_d0() {
    let r = monthly(1, Some(MonthDay::Last), FromDue);
    assert_eq!(
        next_due_date(
            &r,
            date!(2026 - 10 - 31),
            date!(2026 - 10 - 20),
            date!(2026 - 10 - 20)
        ),
        Some(date!(2026 - 11 - 30))
    );
}

#[test]
fn today_itself_is_a_valid_next_date() {
    let r = monthly(1, Some(MonthDay::Last), FromDue);
    assert_eq!(
        next_due_date(
            &r,
            date!(2026 - 09 - 30),
            date!(2026 - 10 - 31),
            date!(2026 - 10 - 31)
        ),
        Some(date!(2026 - 10 - 31))
    );
}

// --- from done ----------------------------------------------------------

#[test]
fn from_done_counts_from_the_completion_date() {
    let d0 = date!(2026 - 09 - 01);
    let done = date!(2026 - 10 - 15);
    let today = date!(2026 - 10 - 20);
    let cases = [
        (rec(2, RecurrenceUnit::Day, FromDone), date!(2026 - 10 - 17)),
        (
            rec(1, RecurrenceUnit::Week, FromDone),
            date!(2026 - 10 - 22),
        ),
        (monthly(3, None, FromDone), date!(2027 - 01 - 15)),
        (
            monthly(1, Some(MonthDay::Last), FromDone),
            date!(2026 - 11 - 30),
        ),
        (
            monthly(1, Some(MonthDay::Day(5)), FromDone),
            date!(2026 - 11 - 05),
        ),
        (
            rec(1, RecurrenceUnit::Year, FromDone),
            date!(2027 - 10 - 15),
        ),
        // Wednesday → the earliest named weekday of the next week.
        (weekly(1, &[Th, Mo], FromDone), date!(2026 - 10 - 19)),
        (weekly(2, &[Th], FromDone), date!(2026 - 10 - 29)),
    ];
    for (r, want) in cases {
        let done = if r.weekdays.is_some() {
            date!(2026 - 10 - 14)
        } else {
            done
        };
        assert_eq!(next_due_date(&r, d0, done, today), Some(want), "{r:?}");
    }
}

#[test]
fn from_done_month_clamps_to_the_last_day() {
    let r = monthly(1, None, FromDone);
    assert_eq!(
        next_due_date(
            &r,
            date!(2026 - 01 - 01),
            date!(2027 - 01 - 31),
            date!(2027 - 01 - 31)
        ),
        Some(date!(2027 - 02 - 28))
    );
}

#[test]
fn from_done_does_not_adjust_to_today_and_writes_no_day() {
    // Completed offline long ago, synced today: anchored at the completion.
    let r = monthly(1, None, FromDone);
    let next = next_occurrence(
        &r,
        Due::Date(date!(2026 - 06 - 30)),
        None,
        at_noon(date!(2026 - 07 - 10)),
        date!(2026 - 10 - 15),
        &UTC,
    )
    .unwrap();
    assert_eq!(next.due, Due::Date(date!(2026 - 08 - 10)));
    assert_eq!(next.recurrence, r);
}

#[test]
fn from_done_uses_the_local_completion_date() {
    // 23:30 UTC on 14.10. is already 15.10. in Berlin.
    let r = rec(1, RecurrenceUnit::Day, FromDone);
    let next = next_occurrence(
        &r,
        Due::Date(date!(2026 - 10 - 10)),
        None,
        datetime!(2026-10-14 23:30 UTC),
        date!(2026 - 10 - 15),
        &Berlin,
    )
    .unwrap();
    assert_eq!(next.due, Due::Date(date!(2026 - 10 - 16)));
}

// --- time of day and daylight saving ------------------------------------

fn next_due(r: &Recurrence, due: OffsetDateTime, zone: &impl LocalZone) -> Due {
    let today = due.to_offset(zone.offset_at(due)).date();
    next_occurrence(r, Due::DateTime(due), None, due, today, zone)
        .unwrap()
        .due
}

#[test]
fn local_time_survives_the_end_of_daylight_saving() {
    let daily = rec(1, RecurrenceUnit::Day, FromDue);
    assert_eq!(
        next_due(&daily, datetime!(2026-10-24 09:00 +2), &Berlin),
        Due::DateTime(datetime!(2026-10-25 09:00 +1))
    );
    assert_eq!(
        next_due(&daily, datetime!(2026-10-25 09:00 +1), &Berlin),
        Due::DateTime(datetime!(2026-10-26 09:00 +1))
    );
}

#[test]
fn local_time_survives_the_start_of_daylight_saving() {
    // 28.03.2027 is the last Sunday of March.
    let daily = rec(1, RecurrenceUnit::Day, FromDue);
    assert_eq!(
        next_due(&daily, datetime!(2027-03-27 09:00 +1), &Berlin),
        Due::DateTime(datetime!(2027-03-28 09:00 +2))
    );
    let monthly_last = monthly(1, Some(MonthDay::Last), FromDue);
    assert_eq!(
        next_due(&monthly_last, datetime!(2027-02-28 17:00 +1), &Berlin),
        Due::DateTime(datetime!(2027-03-31 17:00 +2))
    );
}

#[test]
fn a_time_in_the_spring_gap_moves_later_by_the_gap() {
    let daily = rec(1, RecurrenceUnit::Day, FromDue);
    let first = next_due(&daily, datetime!(2027-03-27 02:30 +1), &Berlin);
    assert_eq!(first, Due::DateTime(datetime!(2027-03-28 03:30 +2)));
    let Due::DateTime(first) = first else {
        unreachable!()
    };
    // The series keeps the later local time afterwards (accepted, 3.1).
    assert_eq!(
        next_due(&daily, first, &Berlin),
        Due::DateTime(datetime!(2027-03-29 03:30 +2))
    );
}

#[test]
fn a_time_in_the_autumn_overlap_takes_the_earlier_instant() {
    let daily = rec(1, RecurrenceUnit::Day, FromDue);
    assert_eq!(
        next_due(&daily, datetime!(2026-10-24 02:30 +2), &Berlin),
        Due::DateTime(datetime!(2026-10-25 02:30 +2))
    );
    assert_eq!(
        next_due(&daily, datetime!(2026-10-25 02:30 +1), &Berlin),
        Due::DateTime(datetime!(2026-10-26 02:30 +1))
    );
}

#[test]
fn a_due_in_another_offset_is_read_in_the_server_zone() {
    // 07:00 UTC is 09:00 in Berlin; the next due is 09:00 local time.
    let daily = rec(1, RecurrenceUnit::Day, FromDue);
    assert_eq!(
        next_due(&daily, datetime!(2026-10-24 07:00 UTC), &Berlin),
        Due::DateTime(datetime!(2026-10-25 09:00 +1))
    );
    // 23:30 UTC on 30.09. is 01:30 on 01.10. in Berlin: D0 is the 1st.
    let monthly_none = monthly(1, None, FromDue);
    let next = next_occurrence(
        &monthly_none,
        Due::DateTime(datetime!(2026-09-30 23:30 UTC)),
        None,
        datetime!(2026-10-01 08:00 UTC),
        date!(2026 - 10 - 01),
        &Berlin,
    )
    .unwrap();
    assert_eq!(next.due, Due::DateTime(datetime!(2026-11-01 01:30 +1)));
    assert_eq!(next.recurrence.day, Some(MonthDay::Day(1)));
}

#[test]
fn a_plain_date_stays_a_plain_date() {
    let daily = rec(1, RecurrenceUnit::Day, FromDue);
    let next = next_occurrence(
        &daily,
        Due::Date(date!(2026 - 10 - 24)),
        None,
        datetime!(2026-10-24 22:00 UTC),
        date!(2026 - 10 - 24),
        &Berlin,
    )
    .unwrap();
    assert_eq!(next.due, Due::Date(date!(2026 - 10 - 25)));
}

// --- remind_at ----------------------------------------------------------

#[test]
fn reminder_keeps_its_distance_in_days_and_local_time() {
    let r = monthly(1, Some(MonthDay::Last), FromDue);
    let next = next_occurrence(
        &r,
        Due::Date(date!(2026 - 10 - 31)),
        Some(datetime!(2026-10-30 18:00 +1)),
        datetime!(2026-10-31 10:00 +1),
        date!(2026 - 10 - 31),
        &Berlin,
    )
    .unwrap();
    assert_eq!(next.due, Due::Date(date!(2026 - 11 - 30)));
    assert_eq!(next.remind_at, Some(datetime!(2026-11-29 18:00 +1)));
}

#[test]
fn reminder_keeps_local_time_across_daylight_saving() {
    let daily = rec(1, RecurrenceUnit::Day, FromDue);
    let next = next_occurrence(
        &daily,
        Due::Date(date!(2026 - 10 - 25)),
        Some(datetime!(2026-10-24 18:00 +2)),
        datetime!(2026-10-25 10:00 +1),
        date!(2026 - 10 - 25),
        &Berlin,
    )
    .unwrap();
    assert_eq!(next.remind_at, Some(datetime!(2026-10-25 18:00 +1)));
}

#[test]
fn reminder_moves_backwards_on_early_completion_from_done() {
    // Due 31.10., reminder the day before; done on 01.10. → due 01.11.,
    // shifted by +1 day, so the reminder is 31.10. 18:00.
    let r = monthly(1, None, FromDone);
    let next = next_occurrence(
        &r,
        Due::Date(date!(2026 - 10 - 31)),
        Some(datetime!(2026-10-30 18:00 +1)),
        datetime!(2026-10-01 10:00 +2),
        date!(2026 - 10 - 01),
        &Berlin,
    )
    .unwrap();
    assert_eq!(next.due, Due::Date(date!(2026 - 11 - 01)));
    assert_eq!(next.remind_at, Some(datetime!(2026-10-31 18:00 +1)));
    // Due 31.12., reminder on 30.12.; done on 05.06. → due 05.07. moves 179
    // days back, so does the reminder.
    let next = next_occurrence(
        &r,
        Due::Date(date!(2026 - 12 - 31)),
        Some(datetime!(2026-12-30 08:00 +1)),
        datetime!(2026-06-05 10:00 +2),
        date!(2026 - 06 - 05),
        &Berlin,
    )
    .unwrap();
    assert_eq!(next.due, Due::Date(date!(2026 - 07 - 05)));
    assert_eq!(next.remind_at, Some(datetime!(2026-07-04 08:00 +2)));
}

#[test]
fn no_reminder_without_one_on_the_predecessor() {
    let daily = rec(1, RecurrenceUnit::Day, FromDue);
    let next = next_occurrence(
        &daily,
        Due::Date(date!(2026 - 10 - 15)),
        None,
        at_noon(date!(2026 - 10 - 15)),
        date!(2026 - 10 - 15),
        &UTC,
    )
    .unwrap();
    assert_eq!(next.remind_at, None);
}

// --- limits -------------------------------------------------------------

#[test]
fn results_beyond_9999_give_none() {
    let daily = rec(1, RecurrenceUnit::Day, FromDue);
    let d0 = date!(9999 - 12 - 31);
    assert_eq!(next_due_date(&daily, d0, d0, d0), None);
    assert!(next_occurrence(&daily, Due::Date(d0), None, at_noon(d0), d0, &UTC).is_none());
    let yearly = rec(999, RecurrenceUnit::Year, FromDone);
    assert_eq!(next_due_date(&yearly, d0, date!(9500 - 01 - 01), d0), None);
    // A reminder pushed past 9999 drops the whole instance as well.
    let d = date!(9999 - 12 - 30);
    assert!(next_occurrence(
        &daily,
        Due::Date(d),
        Some(datetime!(9999-12-30 12:00 UTC)),
        at_noon(d),
        d,
        &UTC
    )
    .is_some());
    assert!(next_occurrence(
        &rec(1, RecurrenceUnit::Day, FromDone),
        Due::Date(d),
        Some(datetime!(9999-12-31 12:00 UTC)),
        at_noon(d),
        d,
        &UTC
    )
    .is_none());
}

#[test]
fn fixed_zone_and_closures_are_zones() {
    let plus_five = |_: OffsetDateTime| offset!(+5);
    let daily = rec(1, RecurrenceUnit::Day, FromDue);
    assert_eq!(
        next_due(&daily, datetime!(2026-10-15 09:00 +5), &plus_five),
        Due::DateTime(datetime!(2026-10-16 09:00 +5))
    );
    assert_eq!(
        next_due(
            &daily,
            datetime!(2026-10-15 09:00 +5),
            &FixedZone(offset!(+5))
        ),
        Due::DateTime(datetime!(2026-10-16 09:00 +5))
    );
}

#[test]
fn resolve_local_handles_gap_and_overlap() {
    let gap = datetime!(2027-03-28 02:30);
    assert_eq!(
        resolve_local(gap, &Berlin),
        Some(datetime!(2027-03-28 03:30 +2))
    );
    let fold = datetime!(2026-10-25 02:30);
    assert_eq!(
        resolve_local(fold, &Berlin),
        Some(datetime!(2026-10-25 02:30 +2))
    );
    let plain = datetime!(2026-07-01 12:00);
    assert_eq!(
        resolve_local(plain, &Berlin),
        Some(datetime!(2026-07-01 12:00 +2))
    );
    assert_eq!(
        to_local(datetime!(2026-07-01 10:00 UTC), &Berlin),
        Some(datetime!(2026-07-01 12:00))
    );
}

// --- properties ---------------------------------------------------------

mod props {
    use super::*;
    use proptest::prelude::*;

    fn date_in(from: Date, to: Date) -> impl Strategy<Value = Date> {
        (from.to_julian_day()..=to.to_julian_day()).prop_map(|j| Date::from_julian_day(j).unwrap())
    }

    fn any_date() -> impl Strategy<Value = Date> {
        date_in(date!(1990 - 01 - 01), date!(2100 - 12 - 31))
    }

    fn weekday_set() -> impl Strategy<Value = Vec<WeekdayCode>> {
        proptest::sample::subsequence(
            vec![
                WeekdayCode::Mo,
                WeekdayCode::Tu,
                WeekdayCode::We,
                WeekdayCode::Th,
                WeekdayCode::Fr,
                WeekdayCode::Sa,
                WeekdayCode::Su,
            ],
            1..=7,
        )
        .prop_shuffle()
    }

    fn month_day() -> impl Strategy<Value = Option<MonthDay>> {
        prop_oneof![
            Just(None),
            Just(Some(MonthDay::Last)),
            (1u8..=31).prop_map(|d| Some(MonthDay::Day(d))),
        ]
    }

    fn any_recurrence(from: RecurrenceFrom) -> impl Strategy<Value = Recurrence> {
        prop_oneof![
            (1u16..=60).prop_map(move |e| rec(e, RecurrenceUnit::Day, from)),
            (1u16..=10).prop_map(move |e| rec(e, RecurrenceUnit::Week, from)),
            (1u16..=10, weekday_set()).prop_map(move |(e, w)| weekly(e, &w, from)),
            (1u16..=24, month_day()).prop_map(move |(e, d)| monthly(e, d, from)),
            (1u16..=8, month_day()).prop_map(move |(e, d)| yearly(e, d, from)),
        ]
    }

    fn monday(d: Date) -> Date {
        d - Duration::days(i64::from(d.weekday().number_days_from_monday()))
    }

    fn month_index(d: Date) -> i64 {
        i64::from(d.year()) * 12 + i64::from(u8::from(d.month())) - 1
    }

    fn is_last_of_month(d: Date) -> bool {
        d.day() == d.month().length(d.year())
    }

    /// Does `x` fall on the configured day (`day`, without it the day of
    /// `anchor`), clamped to the length of its month?
    fn day_ok(day: Option<MonthDay>, anchor: Date, x: Date) -> bool {
        match day.unwrap_or(MonthDay::Day(anchor.day())) {
            MonthDay::Last => is_last_of_month(x),
            MonthDay::Day(d) => x.day() == d.min(x.month().length(x.year())),
        }
    }

    /// Independent oracle: is `x` a date of the fixed-calendar series of
    /// `r` anchored at `d0` (contract 3.1, `from: "due"`)?
    fn is_candidate(r: &Recurrence, d0: Date, x: Date) -> bool {
        if x <= d0 {
            return false;
        }
        let every = i64::from(r.every);
        let days = (x - d0).whole_days();
        match r.unit {
            RecurrenceUnit::Day => days % every == 0,
            RecurrenceUnit::Week => match &r.weekdays {
                None => days % (7 * every) == 0,
                Some(w) => {
                    let weeks = (monday(x) - monday(d0)).whole_days() / 7;
                    w.contains(&WeekdayCode::of(x.weekday())) && weeks % every == 0
                }
            },
            RecurrenceUnit::Month => {
                let months = month_index(x) - month_index(d0);
                months % every == 0 && day_ok(r.day, d0, x)
            }
            RecurrenceUnit::Year => {
                let years = i64::from(x.year() - d0.year());
                x.month() == d0.month() && years % every == 0 && day_ok(r.day, d0, x)
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        /// From due: the result lies after D0, not before today, is a date
        /// of the series, and no earlier such date exists (one instance,
        /// no avalanche).
        #[test]
        fn from_due_is_the_earliest_series_date_not_before_today(
            r in any_recurrence(FromDue),
            d0 in any_date(),
            today_offset in -400i64..4000,
            done_offset in -400i64..400,
        ) {
            let today = d0 + Duration::days(today_offset);
            let done = d0 + Duration::days(done_offset);
            let next = next_due_date(&r, d0, done, today).unwrap();
            prop_assert!(next > d0);
            prop_assert!(next >= today);
            prop_assert!(is_candidate(&r, d0, next), "{next} not in series");
            let mut x = (d0 + Duration::DAY).max(today);
            while x < next {
                prop_assert!(!is_candidate(&r, d0, x), "earlier date {x} skipped");
                x = x.next_day().unwrap();
            }
            // The completion date does not matter for a fixed calendar.
            prop_assert_eq!(next_due_date(&r, d0, d0, today), Some(next));
        }

        /// A monthly series completed on time hits the configured day in
        /// every `every`-th month, without drift, also when `day` starts
        /// out missing.
        #[test]
        fn monthly_series_do_not_drift(
            every in 1u16..=12,
            day in month_day(),
            start in any_date(),
        ) {
            let r = monthly(every, day, FromDue);
            let want_day = day.unwrap_or(MonthDay::Day(start.day()));
            let dates = chain(&r, start, 30);
            let mut prev = start;
            for (i, d) in dates.into_iter().enumerate() {
                let step = month_index(d) - month_index(prev);
                // Only the first step may stay in D0's own month (k = 0: the
                // configured day lies later in that month).
                if i == 0 && step == 0 {
                    prop_assert!(d > start);
                } else {
                    prop_assert_eq!(step, i64::from(every));
                }
                match want_day {
                    MonthDay::Last => prop_assert!(is_last_of_month(d)),
                    MonthDay::Day(n) => prop_assert_eq!(d.day(), n.min(d.month().length(d.year()))),
                }
                prev = d;
            }
        }

        /// A yearly series completed on time keeps its month and hits the
        /// configured day (without `day` the day it started on) in every
        /// `every`-th year: a 28.02. stays a 28.02., a 29.02. comes back in
        /// every leap year.
        #[test]
        fn yearly_series_do_not_drift(
            every in 1u16..=4,
            day in month_day(),
            start in any_date(),
        ) {
            let r = yearly(every, day, FromDue);
            let mut prev = start;
            for (i, d) in chain(&r, start, 12).into_iter().enumerate() {
                prop_assert_eq!(d.month(), start.month());
                let step = d.year() - prev.year();
                // Only the first step may stay in D0's own year (k = 0).
                if i == 0 && step == 0 {
                    prop_assert!(d > start);
                } else {
                    prop_assert_eq!(step, i32::from(every));
                }
                prop_assert!(day_ok(day, start, d), "{d} misses the day");
                prev = d;
            }
        }

        /// From done: the result depends only on the completion date,
        /// lies after it, and never adjusts to today.
        #[test]
        fn from_done_follows_the_completion(
            r in any_recurrence(FromDone),
            d0 in any_date(),
            other_d0 in any_date(),
            done in any_date(),
            today_offset in 0i64..1000,
        ) {
            let today = done + Duration::days(today_offset);
            let next = next_due_date(&r, d0, done, today).unwrap();
            prop_assert!(next > done);
            prop_assert_eq!(next_due_date(&r, other_d0, done, done), Some(next));
            let every = i64::from(r.every);
            match (r.unit, &r.weekdays, r.day) {
                (RecurrenceUnit::Day, _, _) => prop_assert_eq!((next - done).whole_days(), every),
                (RecurrenceUnit::Week, None, _) => {
                    prop_assert_eq!((next - done).whole_days(), 7 * every)
                }
                (RecurrenceUnit::Week, Some(w), _) => {
                    prop_assert_eq!((monday(next) - monday(done)).whole_days(), 7 * every);
                    let first = w.iter().map(|c| c.weekday().number_days_from_monday()).min();
                    prop_assert_eq!(Some(next.weekday().number_days_from_monday()), first);
                }
                (RecurrenceUnit::Month, _, day) => {
                    prop_assert_eq!(month_index(next) - month_index(done), every);
                    prop_assert!(day_ok(day, done, next));
                }
                (RecurrenceUnit::Year, _, day) => {
                    prop_assert_eq!(month_index(next) - month_index(done), 12 * every);
                    prop_assert!(day_ok(day, done, next));
                }
            }
        }

        /// The local time of a due timestamp survives (outside the spring
        /// gap), the offset is the zone's, and a reminder moves by as many
        /// days as the due date, keeping its local time.
        #[test]
        fn local_times_and_reminder_distance_are_kept(
            r in any_recurrence(FromDue),
            d0 in date_in(date!(2000 - 01 - 01), date!(2090 - 12 - 31)),
            hour in 0u8..24,
            minute in 0u8..60,
            remind_days_before in -3i64..10,
            remind_hour in 0u8..24,
            today_offset in -10i64..800,
        ) {
            prop_assume!(hour != 2 && remind_hour != 2);
            let local_due = d0.with_hms(hour, minute, 0).unwrap();
            let due = resolve_local(local_due, &Berlin).unwrap();
            let local_remind = (d0 - Duration::days(remind_days_before))
                .with_hms(remind_hour, 0, 0)
                .unwrap();
            let remind = resolve_local(local_remind, &Berlin).unwrap();
            let today = d0 + Duration::days(today_offset);
            let next = next_occurrence(&r, Due::DateTime(due), Some(remind), due, today, &Berlin)
                .unwrap();
            let Due::DateTime(next_due) = next.due else {
                panic!("timestamp stays a timestamp")
            };
            prop_assert_eq!(next_due.offset(), Berlin.offset_at(next_due));
            prop_assert_eq!(next_due.time(), local_due.time());
            prop_assert_eq!(Some(next_due.date()), next_due_date(&r, d0, d0, today));
            let next_remind = next.remind_at.unwrap();
            prop_assert_eq!(next_remind.offset(), Berlin.offset_at(next_remind));
            prop_assert_eq!(next_remind.time(), local_remind.time());
            prop_assert_eq!(
                (next_remind.date() - local_remind.date()).whole_days(),
                (next_due.date() - d0).whole_days()
            );
        }

        /// Resolving a local time and reading it back gives the same local
        /// time, except in the spring gap (one hour later there).
        #[test]
        fn resolve_local_round_trips(
            d in date_in(date!(2000 - 01 - 01), date!(2090 - 12 - 31)),
            seconds in 0u32..86_400,
        ) {
            let local = d.midnight() + Duration::seconds(i64::from(seconds));
            let instant = resolve_local(local, &Berlin).unwrap();
            let back = to_local(instant, &Berlin).unwrap();
            if back != local {
                prop_assert_eq!(back, local + Duration::HOUR);
                prop_assert_eq!(d, last_sunday(d.year(), Month::March));
            }
        }
    }
}
