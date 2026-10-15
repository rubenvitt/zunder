use serde_json::json;
use time::macros::date;
use time::UtcOffset;

use super::*;
use crate::hlc::{Clock, NodeId};
use crate::model::{Due, Link, Location, MonthDay, RecurrenceFrom, RecurrenceUnit, TaskType};
use crate::recurrence::FixedZone;
use crate::validate::{validate_push_as, Author};

/// 2025-10-09T08:53:20Z.
const NOW: u64 = 1_760_000_000_000;
const TODAY: Date = date!(2025 - 10 - 09);
const UTC: FixedZone = FixedZone(UtcOffset::UTC);

struct World {
    clock: Clock,
    state: State,
}

impl World {
    fn new() -> Self {
        Self {
            clock: Clock::new(NodeId::new("t").unwrap()),
            state: State::default(),
        }
    }

    /// Applies the operations `build` adds (with `clock` at `ms`) and
    /// returns the follow-ups they call for (without creating them).
    fn push_with(
        &mut self,
        clock: &mut Clock,
        ms: u64,
        build: impl FnOnce(&mut OpBatch<'_>),
    ) -> FollowUps {
        let mut batch = OpBatch::new(clock, ms);
        build(&mut batch);
        let ops = batch.into_ops();
        let before = CompletionsBefore::capture(&self.state, &ops);
        for op in &ops {
            self.state.apply(op);
        }
        before.follow_ups(&self.state, TODAY, &UTC)
    }

    fn push(&mut self, build: impl FnOnce(&mut OpBatch<'_>)) -> FollowUps {
        let mut clock = self.clock.clone();
        let out = self.push_with(&mut clock, NOW, build);
        self.clock = clock;
        out
    }

    /// Like `push`, and creates the instances as the server would: the
    /// server's push is valid as `Author::Server` and calls for nothing.
    /// Returns the IDs of the new instances.
    fn push_and_follow(&mut self, build: impl FnOnce(&mut OpBatch<'_>)) -> Vec<Id> {
        let follow_ups = self.push(build);
        assert!(follow_ups.out_of_range.is_empty());
        if follow_ups.instances.is_empty() {
            return Vec::new();
        }
        let mut ids = Vec::new();
        let mut batch = OpBatch::new(&mut self.clock, NOW);
        for f in follow_ups.instances {
            ids.push(f.write(&mut batch));
        }
        let ops = batch.into_ops();
        let outcome = validate_push_as(&self.state, &ops, NOW, Author::Server);
        assert!(outcome.rejected.is_empty(), "{:?}", outcome.rejected);
        let before = CompletionsBefore::capture(&self.state, &ops);
        for op in &ops {
            self.state.apply(op);
        }
        let again = before.follow_ups(&self.state, TODAY, &UTC);
        assert_eq!(again, FollowUps::default(), "the server's own push settles");
        ids
    }

    /// A recurring task due `due` (monthly on the day of `due`).
    fn recurring(&mut self, title: &str, due: &str) -> Id {
        let mut id = None;
        self.push_and_follow(|b| {
            let t = b.create_task(NewTask {
                due: Due::parse(due),
                ..NewTask::new(title)
            });
            b.set_task(t, fields::task::RECURRENCE, monthly());
            id = Some(t);
        });
        id.unwrap()
    }

    fn set_status(&mut self, id: Id, status: TaskStatus) -> Vec<Id> {
        self.push_and_follow(|b| {
            b.set_status(id, status);
        })
    }

    fn task(&self, id: Id) -> crate::model::Task {
        self.state.task(id).unwrap()
    }
}

fn monthly() -> serde_json::Value {
    json!({"every": 1, "unit": "month", "from": "due"})
}

fn predecessors(f: &FollowUps) -> Vec<Id> {
    f.instances.iter().map(|i| i.predecessor).collect()
}

#[test]
fn completing_creates_the_next_instance() {
    let mut w = World::new();
    let mut area_id = None;
    w.push_and_follow(|b| {
        area_id = Some(b.create_area(crate::builder::NewArea::new("Arbeit")));
    });
    let area_id = area_id.unwrap();
    let mut id = None;
    w.push_and_follow(|b| {
        let t = b.create_task(NewTask {
            description: Some("Stunden prüfen".into()),
            due: Due::parse("2025-10-31T17:00:00Z"),
            remind_at: Some(time::macros::datetime!(2025-10-30 08:00 UTC)),
            links: vec![Link {
                url: "https://odoo.example".into(),
                title: Some("Odoo".into()),
            }],
            vault_links: vec!["Arbeit/Odoo.md".into()],
            plan: Some("Plan".into()),
            ..NewTask::new("Odoo absenden").at(Location::Area { id: area_id })
        });
        b.set_task(
            t,
            fields::task::RECURRENCE,
            json!({"every": 1, "unit": "month", "day": "last", "from": "due"}),
        );
        b.set_task(
            t,
            fields::task::PHOENIX,
            json!({"role": "flamme", "fire": "2025-W44"}),
        );
        id = Some(t);
    });
    let id = id.unwrap();

    let follow = w.push(|b| {
        b.set_status(id, TaskStatus::Done);
    });
    assert_eq!(follow.out_of_range, Vec::<Id>::new());
    assert_eq!(follow.instances.len(), 1);
    let next = &follow.instances[0];
    assert_eq!(next.predecessor, id);
    assert_eq!(
        next.task,
        NewTask {
            title: "Odoo absenden".into(),
            description: Some("Stunden prüfen".into()),
            task_type: TaskType::Aufgabe,
            status: TaskStatus::Open,
            location: Location::Area { id: area_id },
            due: Due::parse("2025-11-30T17:00:00Z"),
            remind_at: Some(time::macros::datetime!(2025-11-29 08:00 UTC)),
            links: vec![Link {
                url: "https://odoo.example".into(),
                title: Some("Odoo".into()),
            }],
            vault_links: vec!["Arbeit/Odoo.md".into()],
            ..NewTask::default()
        }
    );
    assert_eq!(next.recurrence.day, Some(MonthDay::Last));
}

#[test]
fn the_written_instance_carries_the_series_and_nothing_else() {
    let mut w = World::new();
    let id = w.recurring("Miete", "2025-10-31");
    let next = w.set_status(id, TaskStatus::Done);
    assert_eq!(next.len(), 1);
    let task = w.task(next[0]);
    assert_eq!(task.title, "Miete");
    assert_eq!(task.status, TaskStatus::Open);
    assert_eq!(task.due, Due::parse("2025-11-30"));
    assert_eq!(task.recurrence_prev, Some(id));
    // The day of the old due date is written, so the series does not drift.
    let r = task.recurrence.unwrap();
    assert_eq!(
        (r.unit, r.day),
        (RecurrenceUnit::Month, Some(MonthDay::Day(31)))
    );
    assert_eq!(task.phoenix, None);
    assert_eq!(task.reference, None);
    assert!(task.blocked_by.is_empty());
    // The predecessor keeps its recurrence.
    assert_eq!(w.task(id).recurrence.unwrap().day, None);
}

#[test]
fn dropping_creates_the_next_instance_too() {
    let mut w = World::new();
    let id = w.recurring("Miete", "2025-10-31");
    let follow = w.push(|b| {
        b.set_status(id, TaskStatus::Dropped);
    });
    assert_eq!(predecessors(&follow), [id]);
}

#[test]
fn other_status_changes_and_plain_tasks_create_nothing() {
    let mut w = World::new();
    let id = w.recurring("Miete", "2025-10-31");
    assert!(w.set_status(id, TaskStatus::InProgress).is_empty());
    let mut plain = None;
    w.push_and_follow(|b| {
        plain = Some(b.create_task(NewTask {
            due: Due::parse("2025-10-31"),
            ..NewTask::new("Einmalig")
        }));
    });
    assert!(w.set_status(plain.unwrap(), TaskStatus::Done).is_empty());
}

#[test]
fn reopening_and_completing_again_creates_no_second_instance() {
    let mut w = World::new();
    let id = w.recurring("Miete", "2025-10-31");
    let first = w.set_status(id, TaskStatus::Done);
    assert_eq!(first.len(), 1);
    assert!(w.set_status(id, TaskStatus::Open).is_empty());
    // Reopening leaves the instance alone.
    assert!(!w.task(first[0]).deleted);
    assert!(w.set_status(id, TaskStatus::Done).is_empty());
    // Within one push, too.
    let follow = w.push(|b| {
        b.set_status(id, TaskStatus::Open);
        b.set_status(id, TaskStatus::Done);
    });
    assert!(follow.instances.is_empty());
}

#[test]
fn a_deleted_instance_is_created_again_on_the_next_completion() {
    let mut w = World::new();
    let id = w.recurring("Miete", "2025-10-31");
    let first = w.set_status(id, TaskStatus::Done)[0];
    w.push_and_follow(|b| {
        b.delete(EntityKind::Task, first);
        b.set_status(id, TaskStatus::Open);
    });
    let again = w.set_status(id, TaskStatus::Done);
    assert_eq!(again.len(), 1);
    assert_ne!(again[0], first);
    // Restoring the deleted one triggers nothing.
    let restored = w.push_and_follow(|b| {
        b.set(EntityKind::Task, first, fields::DELETED, false);
    });
    assert!(restored.is_empty());
}

#[test]
fn concurrent_completions_in_arrival_order() {
    let mut w = World::new();
    let id = w.recurring("Miete", "2025-10-31");
    let mut phone = Clock::new(NodeId::new("phone").unwrap());
    let mut laptop = Clock::new(NodeId::new("laptop").unwrap());
    let first = w.push_with(&mut phone, NOW + 2_000, |b| {
        b.set_status(id, TaskStatus::Done);
    });
    assert_eq!(predecessors(&first), [id]);
    // The second client's completion arrives while the task is closed: no
    // second instance, whether its operation is older or newer.
    let second = w.push_with(&mut laptop, NOW + 1_000, |b| {
        b.set_status(id, TaskStatus::Done);
    });
    assert!(second.instances.is_empty());
    let third = w.push_with(&mut laptop, NOW + 3_000, |b| {
        b.set_status(id, TaskStatus::Dropped);
    });
    assert!(
        third.instances.is_empty(),
        "done → dropped is no completion"
    );
}

#[test]
fn creating_and_completing_in_one_push_counts() {
    let mut w = World::new();
    let follow = w.push(|b| {
        let t = b.create_task(NewTask {
            due: Due::parse("2025-10-31"),
            ..NewTask::new("Offline erfasst")
        });
        b.set_task(t, fields::task::RECURRENCE, monthly());
        b.set_status(t, TaskStatus::Done);
    });
    assert_eq!(follow.instances.len(), 1);

    // Created as done: nothing.
    let follow = w.push(|b| {
        let t = b.create_task(NewTask {
            due: Due::parse("2025-10-31"),
            ..NewTask::new("Schon erledigt").with_status(TaskStatus::Done)
        });
        b.set_task(t, fields::task::RECURRENCE, monthly());
    });
    assert!(follow.instances.is_empty());
}

#[test]
fn reopening_and_completing_in_one_push_counts_without_an_instance() {
    let mut w = World::new();
    // Created as done: no instance yet.
    let mut id = None;
    assert!(w
        .push_and_follow(|b| {
            let t = b.create_task(NewTask {
                due: Due::parse("2025-10-31"),
                ..NewTask::new("Miete").with_status(TaskStatus::Done)
            });
            b.set_task(t, fields::task::RECURRENCE, monthly());
            id = Some(t);
        })
        .is_empty());
    let id = id.unwrap();
    let follow = w.push(|b| {
        b.set_status(id, TaskStatus::Open);
        b.set_status(id, TaskStatus::Done);
    });
    assert_eq!(predecessors(&follow), [id]);
}

#[test]
fn a_stale_reopening_does_not_count() {
    let mut w = World::new();
    let id = w.recurring("Miete", "2025-10-31");
    // Closed without a follow-up (as if the instance were deleted later).
    let mut late = Clock::new(NodeId::new("late").unwrap());
    let done = w.push_with(&mut late, NOW + 10_000, |b| {
        b.set_status(id, TaskStatus::Done);
    });
    assert_eq!(done.instances.len(), 1);
    // An offline client reopened and completed before that: its reopening
    // loses against the status before the push.
    let mut old = Clock::new(NodeId::new("old").unwrap());
    let follow = w.push_with(&mut old, NOW + 5_000, |b| {
        b.set_status(id, TaskStatus::Open);
        b.set_status(id, TaskStatus::Done);
    });
    assert!(follow.instances.is_empty());
}

#[test]
fn ending_the_series_in_the_same_push_creates_nothing() {
    let mut w = World::new();
    let id = w.recurring("Miete", "2025-10-31");
    let follow = w.push(|b| {
        b.set_status(id, TaskStatus::Dropped);
        b.clear(EntityKind::Task, id, fields::task::RECURRENCE);
    });
    assert!(follow.instances.is_empty());
}

#[test]
fn deleted_closed_restored_or_subtasks_create_nothing() {
    let mut w = World::new();
    let id = w.recurring("Miete", "2025-10-31");
    let follow = w.push(|b| {
        b.set_status(id, TaskStatus::Done);
        b.delete(EntityKind::Task, id);
    });
    assert!(follow.instances.is_empty(), "deleted in the same push");
    let follow = w.push(|b| {
        b.set(EntityKind::Task, id, fields::DELETED, false);
    });
    assert!(follow.instances.is_empty(), "restored as done");

    // The state does not validate: a subtask carrying a recurrence anyway.
    let other = w.recurring("Wurzel", "2025-10-31");
    let sub = w.recurring("Unter", "2025-10-31");
    w.push(|b| {
        b.set_parent(sub, Some(other));
    });
    assert!(w.set_status(sub, TaskStatus::Done).is_empty());
}

#[test]
fn several_instances_come_in_predecessor_order() {
    let mut w = World::new();
    let a = w.recurring("A", "2025-10-31");
    let b_id = w.recurring("B", "2025-10-31");
    let follow = w.push(|b| {
        b.set_status(a, TaskStatus::Done);
        b.set_status(b_id, TaskStatus::Done);
    });
    let mut expected = vec![a, b_id];
    expected.sort();
    assert_eq!(predecessors(&follow), expected);
}

#[test]
fn from_done_counts_from_the_completion() {
    let mut w = World::new();
    let mut id = None;
    w.push_and_follow(|b| {
        let t = b.create_task(NewTask {
            due: Due::parse("2025-01-15"),
            ..NewTask::new("Filter wechseln")
        });
        b.set_task(
            t,
            fields::task::RECURRENCE,
            json!({"every": 3, "unit": "month", "from": "done"}),
        );
        id = Some(t);
    });
    let follow = w.push(|b| {
        b.set_status(id.unwrap(), TaskStatus::Done);
    });
    // Completed on 2025-10-09 (NOW): three months later, no day written.
    let next = &follow.instances[0];
    assert_eq!(next.task.due, Due::parse("2026-01-09"));
    assert_eq!(next.recurrence.day, None);
    assert_eq!(next.recurrence.from, RecurrenceFrom::Done);
}

#[test]
fn missed_dates_are_skipped() {
    let mut w = World::new();
    let id = w.recurring("Miete", "2025-07-31");
    let follow = w.push(|b| {
        b.set_status(id, TaskStatus::Done);
    });
    assert_eq!(follow.instances[0].task.due, Due::parse("2025-10-31"));
}

#[test]
fn dates_beyond_9999_create_nothing() {
    let mut w = World::new();
    let mut id = None;
    w.push_and_follow(|b| {
        let t = b.create_task(NewTask {
            due: Due::parse("9999-12-31"),
            ..NewTask::new("Weltende")
        });
        b.set_task(
            t,
            fields::task::RECURRENCE,
            json!({"every": 1, "unit": "day", "from": "due"}),
        );
        id = Some(t);
    });
    let id = id.unwrap();
    let follow = w.push(|b| {
        b.set_status(id, TaskStatus::Done);
    });
    assert!(follow.instances.is_empty());
    assert_eq!(follow.out_of_range, [id]);
}
