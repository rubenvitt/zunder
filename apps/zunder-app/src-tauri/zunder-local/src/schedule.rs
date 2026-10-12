//! Executes reminder plans against an OS notification scheduler (mobile).
//!
//! The scheduler is a trait so the whole flow (plan, cancel before
//! schedule, persist only what was scheduled) is testable without a device;
//! the app implements it with `tauri-plugin-notification`, handing it times
//! from [`plugin_time`].

use std::collections::HashSet;
use std::sync::Mutex;

use time::OffsetDateTime;

use crate::error::{Error, Result};
use crate::reminders::{
    plan_schedule, upcoming_reminders, SchedulePlan, ScheduledReminder, MAX_PENDING,
};
use crate::store::LocalStore;
use crate::sync::lock;

/// An OS facility for pending local notifications.
pub trait NotificationScheduler {
    /// Cancels pending notifications (unknown IDs are ignored).
    fn cancel(&self, ids: &[i32]) -> Result<()>;
    /// Schedules a notification at `reminder.reminder.at_ms` under its ID
    /// (replacing a pending one with the same ID).
    fn schedule(&self, reminder: &ScheduledReminder) -> Result<()>;
}

/// Brings the pending notifications in line with the store's visible state.
///
/// The store lock is held only to read and to save, not while talking to
/// the scheduler. Callers serialize concurrent reschedules.
///
/// Each reminder is scheduled on its own: one that fails (iOS refuses a
/// moment that has just passed) is left out of the saved set and the others
/// still go out; the next run plans it again. The first such error is
/// returned after saving. If cancelling fails, nothing is scheduled or
/// saved: the next run plans from the old set again, which is safe because
/// cancelling unknown IDs is a no-op and scheduling an ID replaces the
/// pending notification.
pub fn reschedule<S: NotificationScheduler>(
    store: &Mutex<LocalStore>,
    scheduler: &S,
    now_ms: i64,
) -> Result<SchedulePlan> {
    let (current, desired) = {
        let store = lock(store);
        (
            store.scheduled_reminders()?,
            upcoming_reminders(store.state(), now_ms, MAX_PENDING),
        )
    };
    let plan = plan_schedule(&current, &desired, now_ms);
    if plan.is_noop() && plan.scheduled == current {
        return Ok(plan);
    }
    if !plan.cancel.is_empty() {
        scheduler.cancel(&plan.cancel)?;
    }
    let mut failed = HashSet::new();
    let mut first_error = None;
    for reminder in &plan.schedule {
        if let Err(e) = scheduler.schedule(reminder) {
            failed.insert(reminder.id);
            first_error.get_or_insert(e);
        }
    }
    let scheduled: Vec<ScheduledReminder> = plan
        .scheduled
        .iter()
        .filter(|s| !failed.contains(&s.id))
        .cloned()
        .collect();
    lock(store).save_scheduled_reminders(&scheduled)?;
    match first_error {
        None => Ok(plan),
        Some(e) => Err(e),
    }
}

/// The moment to hand to `tauri-plugin-notification` (2.x) for `at_ms`,
/// working around how its native parts read the date it sends
/// (`yyyy-MM-ddTHH:mm:ss.<9 digits>Z`):
///
/// - Rounded up to a whole second. Android parses with `.SSS'Z'` and takes
///   all nine digits as milliseconds, so `.5` would fire 5.8 days late.
///   Rounding up means a reminder never fires early.
/// - `local_offset_secs` (iOS only): iOS ignores the `Z` and reads the
///   fields as local wall-clock time. Passing the local wall-clock time of
///   the moment, labelled as UTC, makes iOS schedule the right instant.
///   Give the device's UTC offset *at that moment* (it differs across a DST
///   change). `None` on Android, which parses the time as UTC.
///
/// Re-check both whenever the plugin is updated.
pub fn plugin_time(at_ms: i64, local_offset_secs: Option<i32>) -> Result<OffsetDateTime> {
    let secs = at_ms.div_euclid(1000) + i64::from(at_ms.rem_euclid(1000) != 0);
    OffsetDateTime::from_unix_timestamp(secs + i64::from(local_offset_secs.unwrap_or(0)))
        .map_err(scheduler_error)
}

/// Wraps a scheduler error message.
pub fn scheduler_error(e: impl std::fmt::Display) -> Error {
    Error::Notification(e.to_string())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use serde_json::json;
    use zunder_core::hlc::system_time_ms;
    use zunder_core::model::fields::task as f;
    use zunder_core::NewTask;

    use super::*;
    use crate::testutil::Author;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Cancel(Vec<i32>),
        Schedule(i32, i64),
    }

    #[derive(Default)]
    struct Recorder {
        calls: RefCell<Vec<Call>>,
        fail: bool,
        /// Fails only the reminder at this moment.
        fail_at_ms: Option<i64>,
    }

    impl NotificationScheduler for Recorder {
        fn cancel(&self, ids: &[i32]) -> Result<()> {
            self.calls.borrow_mut().push(Call::Cancel(ids.to_vec()));
            Ok(())
        }

        fn schedule(&self, r: &ScheduledReminder) -> Result<()> {
            if self.fail || self.fail_at_ms == Some(r.reminder.at_ms) {
                return Err(scheduler_error("keine Berechtigung"));
            }
            self.calls
                .borrow_mut()
                .push(Call::Schedule(r.id, r.reminder.at_ms));
            Ok(())
        }
    }

    fn rfc3339(ms: i64) -> String {
        time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
            .unwrap()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap()
    }

    fn apply(store: &Mutex<LocalStore>, ops: &[zunder_core::Op]) {
        let r = lock(store).apply_local(ops, system_time_ms()).unwrap();
        assert!(r.rejected.is_empty(), "{:?}", r.rejected);
    }

    #[test]
    fn schedules_cancels_and_persists() {
        let store = Mutex::new(LocalStore::open_in_memory().unwrap());
        let mut ui = Author::new("web-a");
        let now = system_time_ms() as i64;
        let at = now + 3_600_000;
        let (task, create) = ui.create(|b| b.create_task(NewTask::new("Zahnarzt")));
        apply(&store, &create);
        apply(
            &store,
            &ui.ops(|b| {
                b.set_task(task, f::REMIND_AT, json!(rfc3339(at)));
            }),
        );

        let recorder = Recorder::default();
        let plan = reschedule(&store, &recorder, now).unwrap();
        let id = plan.scheduled[0].id;
        assert_eq!(*recorder.calls.borrow(), vec![Call::Schedule(id, at)]);
        assert_eq!(lock(&store).scheduled_reminders().unwrap(), plan.scheduled);

        // Nothing changed: no scheduler calls.
        let recorder = Recorder::default();
        reschedule(&store, &recorder, now).unwrap();
        assert!(recorder.calls.borrow().is_empty());

        // Moved: cancel first, then schedule under the same ID.
        let later = at + 60_000;
        apply(
            &store,
            &ui.ops(|b| {
                b.set_task(task, f::REMIND_AT, json!(rfc3339(later)));
            }),
        );
        let recorder = Recorder::default();
        reschedule(&store, &recorder, now).unwrap();
        assert_eq!(
            *recorder.calls.borrow(),
            vec![Call::Cancel(vec![id]), Call::Schedule(id, later)]
        );

        // Done: cancelled and forgotten.
        apply(
            &store,
            &ui.ops(|b| {
                b.set_task(task, f::STATUS, "done");
            }),
        );
        let recorder = Recorder::default();
        reschedule(&store, &recorder, now).unwrap();
        assert_eq!(*recorder.calls.borrow(), vec![Call::Cancel(vec![id])]);
        assert!(lock(&store).scheduled_reminders().unwrap().is_empty());
    }

    #[test]
    fn nothing_is_saved_when_the_scheduler_fails() {
        let store = Mutex::new(LocalStore::open_in_memory().unwrap());
        let mut ui = Author::new("web-a");
        let now = system_time_ms() as i64;
        let (task, create) = ui.create(|b| b.create_task(NewTask::new("A")));
        apply(&store, &create);
        apply(
            &store,
            &ui.ops(|b| {
                b.set_task(task, f::REMIND_AT, json!(rfc3339(now + 60_000)));
            }),
        );
        let failing = Recorder {
            fail: true,
            ..Recorder::default()
        };
        let err = reschedule(&store, &failing, now).unwrap_err();
        assert_eq!(err.code(), "notification");
        assert!(lock(&store).scheduled_reminders().unwrap().is_empty());

        // The next run schedules again.
        let recorder = Recorder::default();
        let plan = reschedule(&store, &recorder, now).unwrap();
        assert_eq!(plan.schedule.len(), 1);
    }

    #[test]
    fn one_failing_reminder_does_not_block_the_others() {
        let store = Mutex::new(LocalStore::open_in_memory().unwrap());
        let mut ui = Author::new("web-a");
        let now = system_time_ms() as i64;
        let times = [now + 1_000, now + 60_000, now + 120_000];
        for (i, at) in times.iter().enumerate() {
            let (task, create) = ui.create(|b| b.create_task(NewTask::new(format!("T{i}"))));
            apply(&store, &create);
            apply(
                &store,
                &ui.ops(|b| {
                    b.set_task(task, f::REMIND_AT, json!(rfc3339(*at)));
                }),
            );
        }

        // The earliest one has passed by the time the OS sees it.
        let failing = Recorder {
            fail_at_ms: Some(times[0]),
            ..Recorder::default()
        };
        let err = reschedule(&store, &failing, now).unwrap_err();
        assert_eq!(err.code(), "notification");
        let saved: Vec<i64> = lock(&store)
            .scheduled_reminders()
            .unwrap()
            .iter()
            .map(|s| s.reminder.at_ms)
            .collect();
        assert_eq!(saved, times[1..].to_vec());
        assert_eq!(failing.calls.borrow().len(), 2);

        // The next run retries only the failed one.
        let recorder = Recorder::default();
        let plan = reschedule(&store, &recorder, now).unwrap();
        assert_eq!(plan.schedule.len(), 1);
        assert_eq!(plan.schedule[0].reminder.at_ms, times[0]);
        assert_eq!(plan.scheduled.len(), 3);
    }

    fn iso(t: OffsetDateTime) -> String {
        use time::format_description::well_known::iso8601::{Config, Iso8601};
        const DEFAULT: u128 = Config::DEFAULT.encode();
        t.format(&Iso8601::<DEFAULT>).unwrap()
    }

    #[test]
    fn plugin_times_are_whole_seconds_rounded_up() {
        // 2026-10-12T08:30:00Z
        let at = 1_791_793_800_000;
        assert_eq!(
            iso(plugin_time(at, None).unwrap()),
            "2026-10-12T08:30:00.000000000Z"
        );
        assert_eq!(
            iso(plugin_time(at + 500, None).unwrap()),
            "2026-10-12T08:30:01.000000000Z"
        );
        assert_eq!(
            iso(plugin_time(at - 999, None).unwrap()),
            "2026-10-12T08:30:00.000000000Z"
        );
    }

    #[test]
    fn plugin_times_for_ios_carry_the_local_wall_clock() {
        let at = 1_791_793_800_000; // 08:30Z
                                    // Berlin in summer time (UTC+2): iOS must read 10:30 local.
        assert_eq!(
            iso(plugin_time(at, Some(7_200)).unwrap()),
            "2026-10-12T10:30:00.000000000Z"
        );
        // New York (UTC-4 in October): 04:30 local.
        assert_eq!(
            iso(plugin_time(at, Some(-4 * 3_600)).unwrap()),
            "2026-10-12T04:30:00.000000000Z"
        );
        // After the change to winter time the offset is the one at that
        // moment: 2026-10-26T08:30Z is 09:30 in Berlin (UTC+1).
        let winter = at + 14 * 86_400_000;
        assert_eq!(
            iso(plugin_time(winter, Some(3_600)).unwrap()),
            "2026-10-26T09:30:00.000000000Z"
        );
    }

    #[test]
    fn elapsed_entries_are_dropped_from_the_stored_set() {
        let store = Mutex::new(LocalStore::open_in_memory().unwrap());
        let mut ui = Author::new("web-a");
        let now = system_time_ms() as i64;
        let (task, create) = ui.create(|b| b.create_task(NewTask::new("A")));
        apply(&store, &create);
        apply(
            &store,
            &ui.ops(|b| {
                b.set_task(task, f::REMIND_AT, json!(rfc3339(now + 1_000)));
            }),
        );
        reschedule(&store, &Recorder::default(), now).unwrap();
        // After the reminder fired: no OS call, but the stored set is cleaned.
        let recorder = Recorder::default();
        reschedule(&store, &recorder, now + 2_000).unwrap();
        assert!(recorder.calls.borrow().is_empty());
        assert!(lock(&store).scheduled_reminders().unwrap().is_empty());
    }
}
