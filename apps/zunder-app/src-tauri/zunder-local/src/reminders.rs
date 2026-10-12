//! Local reminders: which notifications should be pending, and how to get
//! from the currently scheduled set to that set.
//!
//! Everything here is pure. On mobile the Tauri shell executes a
//! [`SchedulePlan`] through the OS scheduler and persists
//! [`SchedulePlan::scheduled`] in the local store; on desktop (no OS
//! scheduler) it fires reminders itself using [`due_in_window`].
//!
//! A reminder exists for every open (not done/dropped), live task
//! - at its `remind_at`, and
//! - at its `due` if `due` carries a time of day (a plain date has no
//!   moment to notify at); skipped when it coincides with `remind_at`.
//!
//! Only future reminders count, and only the [`MAX_PENDING`] earliest ones,
//! because iOS keeps at most 64 pending local notifications per app.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use zunder_core::ids::Id;
use zunder_core::{Due, Location, State};

/// iOS limit for pending local notifications per app; used on all platforms.
pub const MAX_PENDING: usize = 64;

const TITLE_MAX_CHARS: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReminderKind {
    /// The task's `remind_at`.
    Remind,
    /// The task's `due` (with time of day).
    Due,
}

impl ReminderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ReminderKind::Remind => "remind",
            ReminderKind::Due => "due",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "remind" => Some(ReminderKind::Remind),
            "due" => Some(ReminderKind::Due),
            _ => None,
        }
    }

    fn label_de(self) -> &'static str {
        match self {
            ReminderKind::Remind => "Erinnerung",
            ReminderKind::Due => "Jetzt fällig",
        }
    }
}

/// A notification that should fire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reminder {
    pub task_id: Id,
    pub kind: ReminderKind,
    /// Unix milliseconds.
    pub at_ms: i64,
    pub title: String,
    pub body: String,
}

impl Reminder {
    pub fn key(&self) -> (Id, ReminderKind) {
        (self.task_id, self.kind)
    }
}

/// A reminder handed to the notification system under a numeric ID
/// (the notification plugin uses `i32` IDs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduledReminder {
    pub id: i32,
    #[serde(flatten)]
    pub reminder: Reminder,
}

/// Steps from the scheduled set to the desired set. Executors must cancel
/// before they schedule: a changed reminder keeps its ID.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SchedulePlan {
    /// IDs of pending notifications to cancel.
    pub cancel: Vec<i32>,
    /// Notifications to schedule (new or changed).
    pub schedule: Vec<ScheduledReminder>,
    /// The complete set that is scheduled after the plan ran (kept and new),
    /// ordered by time. Persist it and pass it to the next plan.
    pub scheduled: Vec<ScheduledReminder>,
}

impl SchedulePlan {
    pub fn is_noop(&self) -> bool {
        self.cancel.is_empty() && self.schedule.is_empty()
    }
}

/// The reminders that should be pending at `now_ms`: future only, earliest
/// first, at most `limit`.
pub fn upcoming_reminders(state: &State, now_ms: i64, limit: usize) -> Vec<Reminder> {
    let mut out = Vec::new();
    for task in state.live_tasks() {
        if task.status.is_closed() {
            continue;
        }
        let remind_ms = task.remind_at.map(unix_ms);
        let due_ms = match task.due {
            Some(Due::DateTime(dt)) => Some(unix_ms(dt)),
            Some(Due::Date(_)) | None => None,
        };
        let container = container_name(state, task.id);
        let title = truncate_chars(&task.title, TITLE_MAX_CHARS);
        let mut push = |kind: ReminderKind, at_ms: i64| {
            if at_ms > now_ms {
                let body = match &container {
                    Some(name) => format!("{} · {name}", kind.label_de()),
                    None => kind.label_de().to_owned(),
                };
                out.push(Reminder {
                    task_id: task.id,
                    kind,
                    at_ms,
                    title: title.clone(),
                    body,
                });
            }
        };
        if let Some(at) = remind_ms {
            push(ReminderKind::Remind, at);
        }
        if let Some(at) = due_ms.filter(|d| Some(*d) != remind_ms) {
            push(ReminderKind::Due, at);
        }
    }
    out.sort_by_key(|r| (r.at_ms, r.task_id, r.kind));
    out.truncate(limit);
    out
}

/// Plans the transition from `current` (what was scheduled before) to
/// `desired` (from [`upcoming_reminders`]).
///
/// - unchanged reminders are kept (no OS call),
/// - changed ones are cancelled and scheduled again under the same ID,
/// - reminders no longer desired are cancelled,
/// - entries of `current` at or before `now_ms` have fired already: they are
///   dropped without cancelling, so a delivered notification stays visible.
pub fn plan_schedule(
    current: &[ScheduledReminder],
    desired: &[Reminder],
    now_ms: i64,
) -> SchedulePlan {
    let desired_keys: HashSet<_> = desired.iter().map(Reminder::key).collect();
    let mut live: HashMap<(Id, ReminderKind), &ScheduledReminder> = HashMap::new();
    let mut used_ids: HashSet<i32> = HashSet::new();
    let mut plan = SchedulePlan::default();

    for entry in current {
        // Keep elapsed IDs reserved for this round: on Android scheduling an
        // ID replaces a delivered notification with the same ID.
        used_ids.insert(entry.id);
        if entry.reminder.at_ms <= now_ms {
            continue;
        }
        if desired_keys.contains(&entry.reminder.key()) {
            live.insert(entry.reminder.key(), entry);
        } else {
            plan.cancel.push(entry.id);
        }
    }

    for reminder in desired {
        match live.get(&reminder.key()) {
            Some(existing) if existing.reminder == *reminder => {
                plan.scheduled.push((*existing).clone());
            }
            Some(existing) => {
                plan.cancel.push(existing.id);
                let entry = ScheduledReminder {
                    id: existing.id,
                    reminder: reminder.clone(),
                };
                plan.schedule.push(entry.clone());
                plan.scheduled.push(entry);
            }
            None => {
                let id = free_id(reminder, &used_ids);
                used_ids.insert(id);
                let entry = ScheduledReminder {
                    id,
                    reminder: reminder.clone(),
                };
                plan.schedule.push(entry.clone());
                plan.scheduled.push(entry);
            }
        }
    }
    plan.cancel.sort_unstable();
    plan
}

/// Reminders whose time lies in `(after_ms, until_ms]`, for the desktop
/// timer: desktop has no OS scheduler, so the app fires reminders itself
/// while it runs, computed straight from the state (no stored schedule that
/// could lag behind a change).
pub fn due_in_window(state: &State, after_ms: i64, until_ms: i64) -> Vec<Reminder> {
    let mut due = upcoming_reminders(state, after_ms, usize::MAX);
    due.retain(|r| r.at_ms <= until_ms);
    due
}

/// Time of the next reminder after `now_ms`.
pub fn next_reminder_at(state: &State, now_ms: i64) -> Option<i64> {
    upcoming_reminders(state, now_ms, 1)
        .first()
        .map(|r| r.at_ms)
}

/// A stable notification ID for `(task, kind)`; on collision the next
/// candidate in a deterministic sequence is used.
fn free_id(reminder: &Reminder, used: &HashSet<i32>) -> i32 {
    (0u32..)
        .map(|attempt| notification_id(reminder.task_id, reminder.kind, attempt))
        .find(|id| !used.contains(id))
        .expect("the ID space is far larger than the number of reminders")
}

/// FNV-1a over task ID, kind and attempt, folded into `1..=i32::MAX`.
pub fn notification_id(task_id: Id, kind: ReminderKind, attempt: u32) -> i32 {
    let mut hash: u32 = 0x811c_9dc5;
    let kind_byte = match kind {
        ReminderKind::Remind => 1u8,
        ReminderKind::Due => 2u8,
    };
    for byte in task_id
        .as_str()
        .as_bytes()
        .iter()
        .copied()
        .chain([kind_byte])
        .chain(attempt.to_le_bytes())
    {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    let id = i32::try_from(hash & 0x7fff_ffff).expect("masked to 31 bits");
    id.max(1)
}

fn unix_ms(dt: OffsetDateTime) -> i64 {
    i64::try_from(dt.unix_timestamp_nanos() / 1_000_000).unwrap_or(i64::MAX)
}

/// Name of the area or project the task effectively lives in.
fn container_name(state: &State, task_id: Id) -> Option<String> {
    match state.effective_location(task_id)? {
        Location::Project { id } => state.project(id).map(|p| p.name),
        Location::Area { id } => state.area(id).map(|a| a.name),
        Location::Inbox | Location::Loose => None,
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((idx, _)) => format!("{}…", &s[..idx]),
        None => s.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use zunder_core::model::fields::task as f;
    use zunder_core::{Clock, NewArea, NewProject, NewTask, NodeId, Op, OpBatch, TaskStatus};

    const NOW: i64 = 1_790_000_000_000; // 2026-09-21
    const MIN: i64 = 60_000;

    fn rfc3339(ms: i64) -> String {
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
            .unwrap()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap()
    }

    struct World {
        clock: Clock,
        state: State,
    }

    impl World {
        fn new() -> Self {
            Self {
                clock: Clock::new(NodeId::new("test").unwrap()),
                state: State::new(),
            }
        }

        fn batch(&mut self, build: impl FnOnce(&mut OpBatch<'_>) -> Id) -> Id {
            let mut batch = OpBatch::new(&mut self.clock, NOW as u64);
            let id = build(&mut batch);
            let ops: Vec<Op> = batch.into_ops();
            for op in &ops {
                self.state.apply(op);
            }
            id
        }

        fn task(&mut self, title: &str) -> Id {
            self.batch(|b| b.create_task(NewTask::new(title)))
        }

        fn set(&mut self, id: Id, field: &str, value: serde_json::Value) {
            self.batch(|b| {
                b.set_task(id, field, value);
                id
            });
        }
    }

    #[test]
    fn remind_at_and_timed_due_produce_reminders() {
        let mut w = World::new();
        let a = w.task("Zahnarzt anrufen");
        w.set(a, f::REMIND_AT, json!(rfc3339(NOW + 10 * MIN)));
        let b = w.task("Steuer abgeben");
        w.set(b, f::DUE, json!(rfc3339(NOW + 5 * MIN)));

        let got = upcoming_reminders(&w.state, NOW, MAX_PENDING);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].task_id, b);
        assert_eq!(got[0].kind, ReminderKind::Due);
        assert_eq!(got[0].at_ms, NOW + 5 * MIN);
        assert_eq!(got[0].body, "Jetzt fällig");
        assert_eq!(got[1].task_id, a);
        assert_eq!(got[1].kind, ReminderKind::Remind);
        assert_eq!(got[1].title, "Zahnarzt anrufen");
        assert_eq!(got[1].body, "Erinnerung");
    }

    #[test]
    fn plain_dates_past_times_and_closed_or_deleted_tasks_are_ignored() {
        let mut w = World::new();
        let date_only = w.task("Nur Datum");
        w.set(date_only, f::DUE, json!("2099-01-01"));
        let past = w.task("Vorbei");
        w.set(past, f::REMIND_AT, json!(rfc3339(NOW - MIN)));
        let exactly_now = w.task("Genau jetzt");
        w.set(exactly_now, f::REMIND_AT, json!(rfc3339(NOW)));
        for status in [TaskStatus::Done, TaskStatus::Dropped] {
            let t = w.task("Geschlossen");
            w.set(t, f::REMIND_AT, json!(rfc3339(NOW + MIN)));
            w.set(t, f::STATUS, json!(status));
        }
        let deleted = w.task("Gelöscht");
        w.set(deleted, f::REMIND_AT, json!(rfc3339(NOW + MIN)));
        w.set(deleted, zunder_core::model::fields::DELETED, json!(true));

        assert!(upcoming_reminders(&w.state, NOW, MAX_PENDING).is_empty());
    }

    #[test]
    fn due_equal_to_remind_at_yields_one_reminder() {
        let mut w = World::new();
        let t = w.task("Termin");
        // Same instant, different offsets.
        w.set(t, f::REMIND_AT, json!("2026-10-12T09:00:00+02:00"));
        w.set(t, f::DUE, json!("2026-10-12T07:00:00Z"));
        let got = upcoming_reminders(&w.state, NOW, MAX_PENDING);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].kind, ReminderKind::Remind);
    }

    #[test]
    fn body_names_the_project_or_area_of_the_root_task() {
        let mut w = World::new();
        let area = w.batch(|b| b.create_area(NewArea::new("Gesundheit")));
        let project = w.batch(|b| b.create_project(NewProject::new("Umzug", area)));
        let in_area =
            w.batch(|b| b.create_task(NewTask::new("Joggen").at(Location::Area { id: area })));
        let parent = w
            .batch(|b| b.create_task(NewTask::new("Kisten").at(Location::Project { id: project })));
        let sub = w.batch(|b| b.create_task(NewTask::subtask("Klebeband", parent)));
        w.set(in_area, f::REMIND_AT, json!(rfc3339(NOW + MIN)));
        w.set(sub, f::REMIND_AT, json!(rfc3339(NOW + 2 * MIN)));

        let got = upcoming_reminders(&w.state, NOW, MAX_PENDING);
        assert_eq!(got[0].body, "Erinnerung · Gesundheit");
        assert_eq!(got[1].body, "Erinnerung · Umzug");
    }

    #[test]
    fn keeps_only_the_earliest_up_to_the_limit() {
        let mut w = World::new();
        for i in 0..5 {
            let t = w.task(&format!("Aufgabe {i}"));
            w.set(t, f::REMIND_AT, json!(rfc3339(NOW + (5 - i) * MIN)));
        }
        let got = upcoming_reminders(&w.state, NOW, 3);
        let times: Vec<_> = got.iter().map(|r| r.at_ms).collect();
        assert_eq!(times, vec![NOW + MIN, NOW + 2 * MIN, NOW + 3 * MIN]);
    }

    #[test]
    fn long_titles_are_truncated() {
        let mut w = World::new();
        let t = w.task(&"ä".repeat(300));
        w.set(t, f::REMIND_AT, json!(rfc3339(NOW + MIN)));
        let got = upcoming_reminders(&w.state, NOW, 1);
        assert_eq!(got[0].title.chars().count(), TITLE_MAX_CHARS + 1);
        assert!(got[0].title.ends_with('…'));
    }

    fn reminder(task: u128, kind: ReminderKind, at_ms: i64, title: &str) -> Reminder {
        Reminder {
            task_id: Id::parse(&format!("t{:023}", task)).unwrap(),
            kind,
            at_ms,
            title: title.into(),
            body: "Erinnerung".into(),
        }
    }

    #[test]
    fn plan_from_nothing_schedules_everything() {
        let desired = vec![
            reminder(1, ReminderKind::Remind, NOW + MIN, "A"),
            reminder(1, ReminderKind::Due, NOW + 2 * MIN, "A"),
        ];
        let plan = plan_schedule(&[], &desired, NOW);
        assert!(plan.cancel.is_empty());
        assert_eq!(plan.schedule.len(), 2);
        assert_eq!(plan.scheduled, plan.schedule);
        assert_ne!(plan.schedule[0].id, plan.schedule[1].id);
        assert!(plan.schedule.iter().all(|s| s.id > 0));
    }

    #[test]
    fn plan_is_a_noop_when_nothing_changed() {
        let desired = vec![reminder(1, ReminderKind::Remind, NOW + MIN, "A")];
        let first = plan_schedule(&[], &desired, NOW);
        let second = plan_schedule(&first.scheduled, &desired, NOW);
        assert!(second.is_noop());
        assert_eq!(second.scheduled, first.scheduled);
    }

    #[test]
    fn changed_reminders_are_rescheduled_under_the_same_id() {
        let before = plan_schedule(
            &[],
            &[reminder(1, ReminderKind::Remind, NOW + MIN, "A")],
            NOW,
        );
        let id = before.scheduled[0].id;
        for changed in [
            reminder(1, ReminderKind::Remind, NOW + 5 * MIN, "A"),
            reminder(1, ReminderKind::Remind, NOW + MIN, "A umbenannt"),
        ] {
            let plan = plan_schedule(&before.scheduled, std::slice::from_ref(&changed), NOW);
            assert_eq!(plan.cancel, vec![id]);
            assert_eq!(plan.schedule.len(), 1);
            assert_eq!(plan.schedule[0].id, id);
            assert_eq!(plan.schedule[0].reminder, changed);
        }
    }

    #[test]
    fn reminders_no_longer_desired_are_cancelled() {
        let before = plan_schedule(
            &[],
            &[
                reminder(1, ReminderKind::Remind, NOW + MIN, "A"),
                reminder(2, ReminderKind::Remind, NOW + MIN, "B"),
            ],
            NOW,
        );
        let keep = before.scheduled[0].reminder.clone();
        let dropped_id = before.scheduled[1].id;
        let plan = plan_schedule(&before.scheduled, &[keep], NOW);
        assert_eq!(plan.cancel, vec![dropped_id]);
        assert!(plan.schedule.is_empty());
        assert_eq!(plan.scheduled.len(), 1);
    }

    #[test]
    fn fired_reminders_are_forgotten_without_cancelling() {
        let before = plan_schedule(
            &[],
            &[reminder(1, ReminderKind::Remind, NOW + MIN, "A")],
            NOW,
        );
        let later = NOW + 2 * MIN;
        let plan = plan_schedule(&before.scheduled, &[], later);
        assert!(plan.is_noop());
        assert!(plan.scheduled.is_empty());
    }

    #[test]
    fn colliding_ids_are_resolved_deterministically() {
        let r = reminder(7, ReminderKind::Remind, NOW + MIN, "A");
        let natural = notification_id(r.task_id, r.kind, 0);
        // Another entry already occupies the natural ID (a fired one).
        let occupant = ScheduledReminder {
            id: natural,
            reminder: reminder(8, ReminderKind::Due, NOW - MIN, "B"),
        };
        let plan = plan_schedule(&[occupant], std::slice::from_ref(&r), NOW);
        assert_eq!(plan.schedule[0].id, notification_id(r.task_id, r.kind, 1));
        assert!(plan.cancel.is_empty());
    }

    #[test]
    fn notification_ids_are_stable_and_positive() {
        let id = Id::parse("tz4a98xxat96iws9zmbrgj3a").unwrap();
        let a = notification_id(id, ReminderKind::Remind, 0);
        assert_eq!(a, notification_id(id, ReminderKind::Remind, 0));
        assert_ne!(a, notification_id(id, ReminderKind::Due, 0));
        for task in 0..10_000u128 {
            assert!(
                notification_id(
                    Id::parse(&format!("t{:023}", task)).unwrap(),
                    ReminderKind::Due,
                    0
                ) > 0
            );
        }
    }

    #[test]
    fn desktop_timer_helpers() {
        let mut w = World::new();
        let a = w.task("A");
        w.set(a, f::REMIND_AT, json!(rfc3339(NOW + MIN)));
        let b = w.task("B");
        w.set(b, f::DUE, json!(rfc3339(NOW + 3 * MIN)));

        assert_eq!(next_reminder_at(&w.state, NOW), Some(NOW + MIN));
        assert_eq!(next_reminder_at(&w.state, NOW + MIN), Some(NOW + 3 * MIN));
        assert_eq!(next_reminder_at(&w.state, NOW + 3 * MIN), None);

        assert!(due_in_window(&w.state, NOW, NOW + MIN - 1).is_empty());
        let fired = due_in_window(&w.state, NOW, NOW + MIN);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].title, "A");
        let fired = due_in_window(&w.state, NOW + MIN, NOW + 10 * MIN);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].kind, ReminderKind::Due);
    }
}
