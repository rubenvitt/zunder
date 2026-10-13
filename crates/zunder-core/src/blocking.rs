//! Task status follows dependencies (`blocked_by`).
//!
//! When a push gives an `open` task a blocker that is neither done nor
//! dropped, the task becomes `blocked`. When the last such blocker of a
//! `blocked` task is resolved (done, dropped, deleted or removed from
//! `blocked_by`), the task becomes `open` again.
//!
//! The rules react to changes, not to the state as a whole, so a status a
//! person chose stays: a task set to `open` despite open blockers is not
//! blocked again until a new blocker appears, a task blocked by hand without
//! any `blocked_by` (waiting for someone) is never opened, and a push that
//! changes a task's status itself decides that status. Tasks in progress,
//! in review or closed are left alone.
//!
//! Only the server applies these rules (after each stored push, as its own
//! push), so offline clients and every write path agree.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::ids::Id;
use crate::model::{fields, TaskStatus};
use crate::op::{EntityKind, Op};
use crate::state::State;

/// The tasks a push may affect, with their open blockers before the push.
/// Capture it on the state before applying the push, then ask
/// [`BlockingBefore::status_changes`] on the state after.
#[derive(Debug, Default)]
pub struct BlockingBefore {
    tasks: BTreeMap<Id, Before>,
}

#[derive(Debug)]
struct Before {
    /// Status if the task was live before the push.
    status: Option<TaskStatus>,
    /// Live blockers that were neither done nor dropped.
    open: BTreeSet<Id>,
}

impl BlockingBefore {
    /// Captures the tasks `ops` may affect: tasks whose `blocked_by`,
    /// `status` or `deleted` changes, and every task naming one of those in
    /// its `blocked_by`.
    pub fn capture(state: &State, ops: &[Op]) -> Self {
        let mut direct: BTreeSet<Id> = BTreeSet::new();
        // Tasks whose open-ness as a blocker may change.
        let mut blockers: HashSet<Id> = HashSet::new();
        for op in ops {
            if op.entity != EntityKind::Task {
                continue;
            }
            match op.field.as_str() {
                fields::task::BLOCKED_BY => {
                    direct.insert(op.entity_id);
                }
                fields::task::STATUS | fields::DELETED => {
                    direct.insert(op.entity_id);
                    blockers.insert(op.entity_id);
                }
                _ => {}
            }
        }
        let mut candidates = direct;
        if !blockers.is_empty() {
            // Stored lists, tombstoned tasks included: a task restored by the
            // push keeps the list it had.
            for task in state.entities_of(EntityKind::Task) {
                if task.blocked_by().iter().any(|b| blockers.contains(b)) {
                    candidates.insert(task.id());
                }
            }
        }
        let tasks = candidates
            .into_iter()
            .map(|id| {
                let status = state
                    .live_entity(EntityKind::Task, id)
                    .map(|e| e.task_status());
                (
                    id,
                    Before {
                        status,
                        open: open_blockers(state, id),
                    },
                )
            })
            .collect();
        Self { tasks }
    }

    /// Status changes the push calls for, as `(task, new status)` ordered by
    /// task ID; `state` is the state after the push.
    pub fn status_changes(&self, state: &State) -> Vec<(Id, TaskStatus)> {
        let mut out = Vec::new();
        for (&id, before) in &self.tasks {
            let Some(task) = state.live_entity(EntityKind::Task, id) else {
                continue;
            };
            let status = task.task_status();
            if before.status.is_some_and(|s| s != status) {
                // The push set the status itself.
                continue;
            }
            let open = open_blockers(state, id);
            match status {
                TaskStatus::Open if !open.is_subset(&before.open) => {
                    out.push((id, TaskStatus::Blocked));
                }
                TaskStatus::Blocked if !before.open.is_empty() && open.is_empty() => {
                    out.push((id, TaskStatus::Open));
                }
                _ => {}
            }
        }
        out
    }
}

/// Live blockers of a live task that are neither done nor dropped.
fn open_blockers(state: &State, task_id: Id) -> BTreeSet<Id> {
    state
        .blockers(task_id)
        .into_iter()
        .filter(|b| {
            state
                .live_entity(EntityKind::Task, *b)
                .is_some_and(|e| !e.task_status().is_closed())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{NewTask, OpBatch};
    use crate::hlc::{Clock, NodeId};
    use serde_json::{json, Value};

    const NOW: u64 = 1_760_000_000_000;

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

        /// Applies the operations `build` adds and returns the status changes
        /// they call for (without applying those).
        fn push(&mut self, build: impl FnOnce(&mut OpBatch<'_>)) -> Vec<(Id, TaskStatus)> {
            let mut batch = OpBatch::new(&mut self.clock, NOW);
            build(&mut batch);
            let ops = batch.into_ops();
            let before = BlockingBefore::capture(&self.state, &ops);
            for op in &ops {
                self.state.apply(op);
            }
            before.status_changes(&self.state)
        }

        /// Like `push`, and applies the changes as the server would.
        fn push_and_follow(&mut self, build: impl FnOnce(&mut OpBatch<'_>)) {
            let changes = self.push(build);
            if !changes.is_empty() {
                let again = self.push(|b| {
                    for (id, status) in &changes {
                        b.set_status(*id, *status);
                    }
                });
                assert!(again.is_empty(), "the server's own push settles");
            }
        }

        fn task(&mut self, title: &str, status: TaskStatus) -> Id {
            let mut id = None;
            self.push_and_follow(|b| {
                id = Some(b.create_task(NewTask {
                    status,
                    ..NewTask::new(title)
                }));
            });
            id.unwrap()
        }

        fn status(&self, id: Id) -> TaskStatus {
            self.state.task(id).unwrap().status
        }
    }

    fn list(ids: &[Id]) -> Value {
        json!(ids.iter().map(Id::to_string).collect::<Vec<_>>())
    }

    #[test]
    fn a_new_open_blocker_blocks_an_open_task() {
        let mut w = World::new();
        let a = w.task("A", TaskStatus::Open);
        let t = w.task("T", TaskStatus::Open);
        let changes = w.push(|b| {
            b.set_task(t, fields::task::BLOCKED_BY, list(&[a]));
        });
        assert_eq!(changes, vec![(t, TaskStatus::Blocked)]);
    }

    #[test]
    fn a_new_task_with_open_blockers_starts_blocked() {
        let mut w = World::new();
        let a = w.task("A", TaskStatus::InProgress);
        let mut t = None;
        let changes = w.push(|b| {
            let id = b.create_task(NewTask::new("T"));
            b.set_task(id, fields::task::BLOCKED_BY, list(&[a]));
            t = Some(id);
        });
        assert_eq!(changes, vec![(t.unwrap(), TaskStatus::Blocked)]);
    }

    #[test]
    fn closed_or_deleted_blockers_do_not_block() {
        let mut w = World::new();
        let done = w.task("erledigt", TaskStatus::Done);
        let dropped = w.task("verworfen", TaskStatus::Dropped);
        let gone = w.task("gelöscht", TaskStatus::Open);
        w.push_and_follow(|b| {
            b.delete(EntityKind::Task, gone);
        });
        let t = w.task("T", TaskStatus::Open);
        let changes = w.push(|b| {
            b.set_task(t, fields::task::BLOCKED_BY, list(&[done, dropped, gone]));
        });
        assert!(changes.is_empty());
    }

    #[test]
    fn only_open_tasks_get_blocked() {
        let mut w = World::new();
        let a = w.task("A", TaskStatus::Open);
        for status in [
            TaskStatus::InProgress,
            TaskStatus::Blocked,
            TaskStatus::Done,
            TaskStatus::Dropped,
        ] {
            let t = w.task("T", status);
            let changes = w.push(|b| {
                b.set_task(t, fields::task::BLOCKED_BY, list(&[a]));
            });
            assert!(changes.is_empty(), "{status:?}");
        }
    }

    #[test]
    fn resolving_the_last_blocker_opens_the_task() {
        let mut w = World::new();
        let a = w.task("A", TaskStatus::Open);
        let c = w.task("C", TaskStatus::Open);
        let t = w.task("T", TaskStatus::Open);
        w.push_and_follow(|b| {
            b.set_task(t, fields::task::BLOCKED_BY, list(&[a, c]));
        });
        assert_eq!(w.status(t), TaskStatus::Blocked);

        // One blocker left: still blocked.
        w.push_and_follow(|b| {
            b.set_status(a, TaskStatus::Done);
        });
        assert_eq!(w.status(t), TaskStatus::Blocked);
        // Dropping the last one opens it.
        w.push_and_follow(|b| {
            b.set_status(c, TaskStatus::Dropped);
        });
        assert_eq!(w.status(t), TaskStatus::Open);
        // Reopening a blocker blocks it again.
        w.push_and_follow(|b| {
            b.set_status(a, TaskStatus::Open);
        });
        assert_eq!(w.status(t), TaskStatus::Blocked);
    }

    #[test]
    fn deleting_or_removing_the_blocker_opens_the_task() {
        let mut w = World::new();
        let a = w.task("A", TaskStatus::Open);
        let t = w.task("T", TaskStatus::Open);
        let u = w.task("U", TaskStatus::Open);
        w.push_and_follow(|b| {
            b.set_task(t, fields::task::BLOCKED_BY, list(&[a]));
            b.set_task(u, fields::task::BLOCKED_BY, list(&[a]));
        });
        assert_eq!(w.status(t), TaskStatus::Blocked);
        assert_eq!(w.status(u), TaskStatus::Blocked);

        w.push_and_follow(|b| {
            b.clear(EntityKind::Task, t, fields::task::BLOCKED_BY);
        });
        assert_eq!(w.status(t), TaskStatus::Open);
        assert_eq!(w.status(u), TaskStatus::Blocked);

        w.push_and_follow(|b| {
            b.delete(EntityKind::Task, a);
        });
        assert_eq!(w.status(u), TaskStatus::Open);
        // Restoring the blocker blocks again.
        w.push_and_follow(|b| {
            b.set_task(a, fields::DELETED, false);
        });
        assert_eq!(w.status(u), TaskStatus::Blocked);
    }

    #[test]
    fn a_status_chosen_in_the_push_wins() {
        let mut w = World::new();
        let a = w.task("A", TaskStatus::Open);
        let t = w.task("T", TaskStatus::Blocked);
        // Adding a blocker and opening by hand in one push: stays open.
        let changes = w.push(|b| {
            b.set_task(t, fields::task::BLOCKED_BY, list(&[a]));
            b.set_status(t, TaskStatus::Open);
        });
        assert!(changes.is_empty());
        // Opened by hand despite the open blocker: not blocked again …
        let changes = w.push(|b| {
            b.set_task(t, fields::task::TITLE, "T neu");
        });
        assert!(changes.is_empty());
        // … until a new blocker appears.
        let c = w.task("C", TaskStatus::Open);
        let changes = w.push(|b| {
            b.set_task(t, fields::task::BLOCKED_BY, list(&[a, c]));
        });
        assert_eq!(changes, vec![(t, TaskStatus::Blocked)]);
    }

    #[test]
    fn a_task_blocked_by_hand_stays_blocked() {
        let mut w = World::new();
        let a = w.task("A", TaskStatus::Done);
        // Blocked without (open) blockers, e.g. waiting for a person.
        let t = w.task("T", TaskStatus::Blocked);
        let changes = w.push(|b| {
            b.set_task(t, fields::task::BLOCKED_BY, list(&[a]));
        });
        assert!(changes.is_empty());
        let changes = w.push(|b| {
            b.clear(EntityKind::Task, t, fields::task::BLOCKED_BY);
        });
        assert!(changes.is_empty());
    }

    #[test]
    fn unrelated_pushes_capture_nothing() {
        let mut w = World::new();
        let a = w.task("A", TaskStatus::Open);
        let mut batch = OpBatch::new(&mut w.clock, NOW);
        batch.set_task(a, fields::task::TITLE, "A neu");
        let ops = batch.into_ops();
        assert!(BlockingBefore::capture(&w.state, &ops).tasks.is_empty());
    }
}
