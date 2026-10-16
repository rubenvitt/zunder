//! "Heute nicht": putting a task off to a later day, counted by the
//! deferral counter of the Phönix-Progression (Verschiebungs-Counter). The
//! web UI mirrors these rules in `web/src/sync/defer.ts`.

use time::{Date, OffsetDateTime, Weekday};

use crate::model::{Due, Task};

/// Deferrals that need no decision. The next one (the third) is a
/// decision instead: drop the task, cut it anew or let it rest.
pub const FREE_DEFERRALS: u32 = 2;

/// Where a task is put off to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferTarget {
    /// The day after today.
    Tomorrow,
    /// Monday of next week.
    NextWeek,
    /// A given day after today.
    Day(Date),
}

impl DeferTarget {
    /// The day the target names, seen from `today`; `None` for a given day
    /// that is not after today.
    pub fn day(self, today: Date) -> Option<Date> {
        match self {
            DeferTarget::Tomorrow => today.next_day(),
            DeferTarget::NextWeek => Some(today.next_occurrence(Weekday::Monday)),
            DeferTarget::Day(day) => (day > today).then_some(day),
        }
    }
}

/// The changes of one deferral (fields that stay are `None`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deferral {
    /// New due date: an earlier one moves to `day` (a timestamp keeps its
    /// time of day and offset); a task without date and reminder gets `day`.
    pub due: Option<Due>,
    /// New reminder: one before `day` moves to `day` at the same time.
    pub remind_at: Option<OffsetDateTime>,
    /// The counter after this deferral.
    pub deferrals: u32,
}

/// Whether putting `task` off once more is a decision rather than a click.
pub fn needs_decision(task: &Task) -> bool {
    task.deferrals >= FREE_DEFERRALS
}

/// What putting `task` off to `day` changes; `None` when its due date and
/// reminder already lie on or after `day`.
pub fn plan(task: &Task, day: Date) -> Option<Deferral> {
    let due = match task.due {
        Some(Due::Date(d)) if d < day => Some(Due::Date(day)),
        Some(Due::DateTime(t)) if t.date() < day => Some(Due::DateTime(t.replace_date(day))),
        _ => None,
    };
    let remind_at = task
        .remind_at
        .filter(|r| r.date() < day)
        .map(|r| r.replace_date(day));
    let due = match (due, task.due, task.remind_at) {
        (None, None, None) => Some(Due::Date(day)),
        (due, ..) => due,
    };
    if due.is_none() && remind_at.is_none() {
        return None;
    }
    Some(Deferral {
        due,
        remind_at,
        deferrals: task.deferrals.saturating_add(1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::parse_rfc3339;
    use crate::state::State;
    use crate::testutil::uid;
    use crate::NewTask;
    use time::macros::date;

    fn task(due: Option<&str>, remind_at: Option<&str>, deferrals: u32) -> Task {
        let mut state = State::new();
        let mut clock = crate::Clock::new(crate::NodeId::new("t").unwrap());
        let mut batch = crate::OpBatch::new(&mut clock, 1_760_000_000_000);
        let id = batch.create_task(NewTask {
            id: Some(uid(1)),
            due: due.and_then(Due::parse),
            remind_at: remind_at.and_then(parse_rfc3339),
            ..NewTask::new("Steuer")
        });
        if deferrals > 0 {
            batch.set_task(id, "deferrals", deferrals);
        }
        for op in batch.into_ops() {
            state.apply(&op);
        }
        state.task(id).unwrap()
    }

    #[test]
    fn targets() {
        let friday = date!(2026 - 10 - 16);
        assert_eq!(
            DeferTarget::Tomorrow.day(friday),
            Some(date!(2026 - 10 - 17))
        );
        assert_eq!(
            DeferTarget::NextWeek.day(friday),
            Some(date!(2026 - 10 - 19))
        );
        // On a Monday, next week is seven days away.
        assert_eq!(
            DeferTarget::NextWeek.day(date!(2026 - 10 - 19)),
            Some(date!(2026 - 10 - 26))
        );
        assert_eq!(DeferTarget::Day(friday).day(friday), None);
        assert_eq!(
            DeferTarget::Day(date!(2026 - 11 - 02)).day(friday),
            Some(date!(2026 - 11 - 02))
        );
    }

    #[test]
    fn moves_an_earlier_due_date_and_counts() {
        let t = task(Some("2026-10-14"), None, 0);
        let d = plan(&t, date!(2026 - 10 - 17)).unwrap();
        assert_eq!(d.due, Due::parse("2026-10-17"));
        assert_eq!(d.remind_at, None);
        assert_eq!(d.deferrals, 1);
    }

    #[test]
    fn keeps_time_of_day_and_offset() {
        let t = task(
            Some("2026-10-16T18:30:00+02:00"),
            Some("2026-10-16T09:00:00+02:00"),
            1,
        );
        let d = plan(&t, date!(2026 - 10 - 19)).unwrap();
        assert_eq!(d.due, Due::parse("2026-10-19T18:30:00+02:00"));
        assert_eq!(d.remind_at, parse_rfc3339("2026-10-19T09:00:00+02:00"));
        assert_eq!(d.deferrals, 2);
    }

    #[test]
    fn a_reminder_alone_moves_without_adding_a_due_date() {
        let t = task(None, Some("2026-10-16T09:00:00Z"), 0);
        let d = plan(&t, date!(2026 - 10 - 17)).unwrap();
        assert_eq!(d.due, None);
        assert_eq!(d.remind_at, parse_rfc3339("2026-10-17T09:00:00Z"));
    }

    #[test]
    fn an_undated_task_gets_the_day() {
        let d = plan(&task(None, None, 0), date!(2026 - 10 - 17)).unwrap();
        assert_eq!(d.due, Due::parse("2026-10-17"));
    }

    #[test]
    fn nothing_to_move_when_already_later() {
        let t = task(Some("2026-10-20"), Some("2026-10-19T09:00:00Z"), 0);
        assert_eq!(plan(&t, date!(2026 - 10 - 17)), None);
        // A later reminder stays while the due date moves.
        let t = task(Some("2026-10-15"), Some("2026-10-30T09:00:00Z"), 0);
        let d = plan(&t, date!(2026 - 10 - 17)).unwrap();
        assert_eq!(d.due, Due::parse("2026-10-17"));
        assert_eq!(d.remind_at, None);
    }

    #[test]
    fn the_third_deferral_is_a_decision() {
        assert!(!needs_decision(&task(None, None, 0)));
        assert!(!needs_decision(&task(None, None, 1)));
        assert!(needs_decision(&task(None, None, 2)));
        assert!(needs_decision(&task(None, None, 5)));
    }
}
