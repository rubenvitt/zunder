//! Follow-up instances: which tasks a push completed so that their series
//! needs its next instance, and what that instance looks like (contract 3.1,
//! "Folgeinstanzen").
//!
//! Like [`crate::blocking`], the trigger compares the state before a push
//! with the state after it: capture [`CompletionsBefore`] on the state before
//! applying the push, then ask [`CompletionsBefore::follow_ups`] on the state
//! after. Only the server does this (as its own push with
//! [`crate::validate::Author::Server`], because of `recurrence_prev`).

use std::collections::{BTreeSet, HashSet};

use time::{Date, OffsetDateTime};

use super::{next_occurrence, LocalZone};
use crate::builder::{NewTask, OpBatch};
use crate::ids::Id;
use crate::model::{fields, Recurrence, TaskStatus};
use crate::op::{EntityKind, Op};
use crate::state::State;

/// The tasks a push may complete. Capture it on the state before applying
/// the push.
#[derive(Debug, Default)]
pub struct CompletionsBefore {
    /// Tasks the push completes if they are closed afterwards.
    candidates: BTreeSet<Id>,
}

impl CompletionsBefore {
    /// Captures the tasks `ops` complete if they end up `done` or `dropped`:
    /// a task with a `status` operation in `ops` that (a) was live and
    /// neither done nor dropped before, or (b) gets a `status` operation
    /// with another value than done/dropped that beats the status winning
    /// before (any such operation for a task without a status before). (b)
    /// covers pushes that bundle several steps of one task (create and
    /// complete, reopen and complete again).
    pub fn capture(state: &State, ops: &[Op]) -> Self {
        let mut candidates = BTreeSet::new();
        for op in ops {
            if op.entity != EntityKind::Task
                || op.field != fields::task::STATUS
                || candidates.contains(&op.entity_id)
            {
                continue;
            }
            let entity = state.entity(EntityKind::Task, op.entity_id);
            let open_before =
                entity.is_some_and(|e| !e.is_deleted() && !e.task_status().is_closed());
            let reopens = !is_closed_value(op)
                && entity
                    .and_then(|e| e.field(fields::task::STATUS))
                    .is_none_or(|winner| (&op.hlc, op.op_id) > (&winner.hlc, winner.op_id));
            if open_before || reopens {
                candidates.insert(op.entity_id);
            }
        }
        Self { candidates }
    }

    /// The next instances the push calls for, ordered by the ID of their
    /// predecessor; `state` is the state after the push, `today` the
    /// server's local date, `zone` its time zone. A task qualifies if it is
    /// live, `done` or `dropped`, a root task, has `recurrence` and `due`,
    /// and no live task names it in `recurrence_prev` yet.
    pub fn follow_ups(&self, state: &State, today: Date, zone: &impl LocalZone) -> FollowUps {
        let mut out = FollowUps::default();
        if self.candidates.is_empty() {
            return out;
        }
        let mut has_next: Option<HashSet<Id>> = None;
        for &id in &self.candidates {
            let Some(task) = state.task(id) else {
                continue;
            };
            if task.deleted || !task.status.is_closed() || task.parent_id.is_some() {
                continue;
            }
            let (Some(recurrence), Some(due)) = (&task.recurrence, task.due) else {
                continue;
            };
            let has_next = has_next.get_or_insert_with(|| successors(state));
            if has_next.contains(&id) {
                continue;
            }
            let completed_at = state
                .status_changed_at(id)
                .and_then(|hlc| hlc_instant(hlc.wall_ms()));
            let next = completed_at.and_then(|completed_at| {
                next_occurrence(recurrence, due, task.remind_at, completed_at, today, zone)
            });
            let Some(next) = next else {
                out.out_of_range.push(id);
                continue;
            };
            out.instances.push(FollowUp {
                predecessor: id,
                task: NewTask {
                    title: task.title,
                    description: task.description,
                    task_type: task.task_type,
                    status: TaskStatus::Open,
                    // A root task's own location is its effective one.
                    location: task.location,
                    due: Some(next.due),
                    remind_at: next.remind_at,
                    vault_links: task.vault_links,
                    links: task.links,
                    // The weighing carries over; the deferrals and whom the
                    // closed instance waited on belong to it (the new one
                    // waits on nobody and starts open).
                    weight: task.weight,
                    for_whom: task.for_whom,
                    ..NewTask::default()
                },
                recurrence: next.recurrence,
            });
        }
        out
    }
}

/// Result of [`CompletionsBefore::follow_ups`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FollowUps {
    /// Instances to create, ordered by predecessor ID.
    pub instances: Vec<FollowUp>,
    /// Completed tasks whose next date would lie beyond the year 9999: no
    /// instance (the server logs them).
    pub out_of_range: Vec<Id>,
}

/// The next instance of a series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowUp {
    /// The completed instance (written as `recurrence_prev`).
    pub predecessor: Id,
    /// Fields of the new task (status open, recomputed due and reminder).
    pub task: NewTask,
    /// Recurrence of the new task.
    pub recurrence: Recurrence,
}

impl FollowUp {
    /// Adds the operations creating this instance to `batch` (only fields
    /// with a value); returns the new task's ID.
    pub fn write(self, batch: &mut OpBatch<'_>) -> Id {
        let id = batch.create_task(self.task);
        batch.set_task(id, fields::task::RECURRENCE, self.recurrence);
        batch.set_task(
            id,
            fields::task::RECURRENCE_PREV,
            self.predecessor.to_string(),
        );
        id
    }
}

/// IDs named in `recurrence_prev` of a live task.
fn successors(state: &State) -> HashSet<Id> {
    state
        .entities_of(EntityKind::Task)
        .filter(|e| !e.is_deleted())
        .filter_map(|e| {
            e.str_value(fields::task::RECURRENCE_PREV)
                .and_then(crate::ids::parse_id)
        })
        .collect()
}

fn is_closed_value(op: &Op) -> bool {
    op.value
        .as_str()
        .and_then(|s| s.parse::<TaskStatus>().ok())
        .is_some_and(TaskStatus::is_closed)
}

/// The instant of an HLC's wall clock (Unix milliseconds).
fn hlc_instant(wall_ms: u64) -> Option<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(wall_ms) * 1_000_000).ok()
}

#[cfg(test)]
mod tests;
