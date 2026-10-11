//! Executes reminder plans against an OS notification scheduler (mobile).
//!
//! The scheduler is a trait so the whole flow (plan, cancel before
//! schedule, persist only on success) is testable without a device; the app
//! implements it with `tauri-plugin-notification`.

use std::sync::Mutex;

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
/// the scheduler. Callers serialize concurrent reschedules. If the scheduler
/// fails, nothing is saved: the next run plans from the old set again, which
/// is safe because cancelling unknown IDs is a no-op and scheduling an ID
/// replaces the pending notification.
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
    for reminder in &plan.schedule {
        scheduler.schedule(reminder)?;
    }
    lock(store).save_scheduled_reminders(&plan.scheduled)?;
    Ok(plan)
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
    }

    impl NotificationScheduler for Recorder {
        fn cancel(&self, ids: &[i32]) -> Result<()> {
            self.calls.borrow_mut().push(Call::Cancel(ids.to_vec()));
            Ok(())
        }

        fn schedule(&self, r: &ScheduledReminder) -> Result<()> {
            if self.fail {
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
