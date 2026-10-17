//! Wochenbilanz ("Balance sichtbar"): what got done in an ISO week, counted
//! by weight and by whom it was for. Zunder mirrors, it does not judge.
//!
//! One rule for the web UI (Wochenspiegel in Heute, mirrored in
//! `web/src/sync/balance.ts`) and the MCP tool `week_balance`; the shared
//! test vectors (`kind: "week_balance"`) keep both sides in step:
//!
//! - A task counts when it is live (no tombstone), its status is `done` and
//!   its effective location is not a dev project. Subtasks count like other
//!   tasks; dropped tasks do not (dropping is a decision, not work done).
//! - It belongs to the ISO week of the local calendar day of
//!   `status_changed_at` (Monday to Sunday in the caller's zone; the server
//!   uses its `TZ`, the web UI the browser's zone).
//! - Every counted task adds one to `total`, one to its weight (`none` when
//!   unweighed) and one to its `for_whom` (`none` when unstated).

use serde::{Serialize, Serializer};
use time::{Date, Duration, OffsetDateTime};

use crate::model::{format_date, ForWhom, IsoWeek, Task, TaskStatus, Weight};
use crate::recurrence::{to_local, LocalZone};
use crate::state::State;

/// Done tasks of a week by weight; `none` = not weighed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct WeightCounts {
    pub brand: u32,
    pub glut: u32,
    pub rauch: u32,
    pub asche: u32,
    pub none: u32,
}

impl WeightCounts {
    fn add(&mut self, weight: Option<Weight>) {
        let slot = match weight {
            Some(Weight::Brand) => &mut self.brand,
            Some(Weight::Glut) => &mut self.glut,
            Some(Weight::Rauch) => &mut self.rauch,
            Some(Weight::Asche) => &mut self.asche,
            None => &mut self.none,
        };
        *slot += 1;
    }
}

/// Done tasks of a week by whom they were for; `none` = not stated.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ForWhomCounts {
    pub mich: u32,
    pub uns: u32,
    pub andere: u32,
    pub none: u32,
}

impl ForWhomCounts {
    fn add(&mut self, for_whom: Option<ForWhom>) {
        let slot = match for_whom {
            Some(ForWhom::Mich) => &mut self.mich,
            Some(ForWhom::Uns) => &mut self.uns,
            Some(ForWhom::Andere) => &mut self.andere,
            None => &mut self.none,
        };
        *slot += 1;
    }
}

/// The balance of one ISO week (the shape of the shared vectors and of
/// `week_balance`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct WeekBalance {
    pub week: IsoWeek,
    /// Monday of the week (`JJJJ-MM-TT`).
    #[serde(serialize_with = "date")]
    pub from: Date,
    /// Sunday of the week (`JJJJ-MM-TT`).
    #[serde(serialize_with = "date")]
    pub to: Date,
    /// Done tasks in the week.
    pub total: u32,
    pub by_weight: WeightCounts,
    pub by_for_whom: ForWhomCounts,
}

fn date<S: Serializer>(d: &Date, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&format_date(*d))
}

impl WeekBalance {
    /// An empty balance of `week`.
    pub fn empty(week: IsoWeek) -> Self {
        let from = week.monday();
        Self {
            week,
            from,
            to: from.saturating_add(Duration::days(6)),
            total: 0,
            by_weight: WeightCounts::default(),
            by_for_whom: ForWhomCounts::default(),
        }
    }

    fn add(&mut self, task: &Task) {
        self.total += 1;
        self.by_weight.add(task.weight);
        self.by_for_whom.add(task.for_whom);
    }
}

/// Whether a task counts in a balance at all (week aside): live, done, not
/// in a dev project.
pub fn counts(state: &State, task: &Task) -> bool {
    !task.deleted
        && task.status == TaskStatus::Done
        && state
            .effective_location(task.id)
            .is_some_and(|location| !state.is_dev_location(location))
}

/// The ISO week a done task belongs to: the week of the local calendar day
/// of its `status_changed_at` in `zone`.
pub fn week_of_completion(state: &State, task: &Task, zone: &impl LocalZone) -> Option<IsoWeek> {
    let changed = state.status_changed_at(task.id)?;
    let ms = i128::from(changed.wall_ms());
    let instant = OffsetDateTime::from_unix_timestamp_nanos(ms * 1_000_000).ok()?;
    Some(IsoWeek::of(to_local(instant, zone)?.date()))
}

/// The balance of `week` in `zone` (one pass over the tasks).
pub fn week_balance(state: &State, week: IsoWeek, zone: &impl LocalZone) -> WeekBalance {
    let mut balance = WeekBalance::empty(week);
    for task in state.live_tasks() {
        if counts(state, &task) && week_of_completion(state, &task, zone) == Some(week) {
            balance.add(&task);
        }
    }
    balance
}

#[cfg(test)]
mod tests {
    use time::macros::date;
    use time::UtcOffset;

    use super::*;
    use crate::builder::{NewArea, NewProject, NewTask, OpBatch};
    use crate::hlc::{Clock, NodeId};
    use crate::ids::Id;
    use crate::model::{Location, ProjectKind};
    use crate::op::Op;
    use crate::recurrence::FixedZone;

    fn zone(hours: i8) -> FixedZone {
        FixedZone(UtcOffset::from_hms(hours, 0, 0).unwrap())
    }

    fn week(s: &str) -> IsoWeek {
        IsoWeek::parse(s).unwrap()
    }

    #[test]
    fn empty_week_spans_monday_to_sunday() {
        let b = WeekBalance::empty(week("2026-W44"));
        assert_eq!(
            (b.from, b.to),
            (date!(2026 - 10 - 26), date!(2026 - 11 - 01))
        );
        // Week 53 of 2026 ends in 2027.
        let b = WeekBalance::empty(week("2026-W53"));
        assert_eq!(
            (b.from, b.to),
            (date!(2026 - 12 - 28), date!(2027 - 01 - 03))
        );
        let json = serde_json::to_value(b).unwrap();
        assert_eq!(json["week"], "2026-W53");
        assert_eq!(json["from"], "2026-12-28");
        assert_eq!(json["to"], "2027-01-03");
        assert_eq!(json["total"], 0);
        assert_eq!(json["by_weight"]["none"], 0);
        assert_eq!(json["by_for_whom"]["andere"], 0);
    }

    /// Ops creating `task` a minute before `closed_ms` and setting `status`
    /// at `closed_ms` (the wall clock of `status_changed_at`).
    fn closed(task: NewTask, status: TaskStatus, closed_ms: u64) -> (Id, Vec<Op>) {
        let mut clock = Clock::new(NodeId::new("t").unwrap());
        let mut create = OpBatch::new(&mut clock, closed_ms - 60_000);
        let id = create.create_task(task);
        let mut ops = create.into_ops();
        let mut close = OpBatch::new(&mut clock, closed_ms);
        close.set_status(id, status);
        ops.extend(close.into_ops());
        (id, ops)
    }

    #[test]
    fn counts_done_tasks_of_the_week_by_weight_and_for_whom() {
        let mut clock = Clock::new(NodeId::new("seed").unwrap());
        let mut seed = OpBatch::new(&mut clock, 1_600_000_000_000);
        let area = seed.create_area(NewArea::new("Arbeit"));
        let project = seed.create_project(NewProject::new("Haus", area));
        let dev = seed.create_project(NewProject {
            kind: ProjectKind::Dev,
            repo: Some("example/app".into()),
            ..NewProject::new("App", area)
        });
        let mut ops = seed.into_ops();
        // Wednesday 2026-10-28 12:00 CET.
        let wed = 1_793_185_200_000;
        let (parent, task_ops) = closed(
            NewTask {
                weight: Some(Weight::Glut),
                for_whom: Some(ForWhom::Mich),
                ..NewTask::new("Laufen").at(Location::Project { id: project })
            },
            TaskStatus::Done,
            wed,
        );
        ops.extend(task_ops);
        let subtask = NewTask {
            weight: Some(Weight::Rauch),
            ..NewTask::subtask("Schuhe", parent)
        };
        ops.extend(closed(subtask, TaskStatus::Done, wed).1);
        let dropped = NewTask {
            weight: Some(Weight::Brand),
            ..NewTask::new("Verworfen")
        };
        ops.extend(closed(dropped, TaskStatus::Dropped, wed).1);
        let in_dev = NewTask {
            weight: Some(Weight::Rauch),
            ..NewTask::new("Ticket").at(Location::Project { id: dev })
        };
        ops.extend(closed(in_dev, TaskStatus::Done, wed).1);
        let last_week = NewTask {
            for_whom: Some(ForWhom::Andere),
            ..NewTask::new("Vorher")
        };
        ops.extend(closed(last_week, TaskStatus::Done, wed - 7 * 86_400_000).1);
        let state = State::from_ops(&ops);

        let b = week_balance(&state, week("2026-W44"), &zone(1));
        assert_eq!(b.total, 2);
        assert_eq!(
            b.by_weight,
            WeightCounts {
                glut: 1,
                rauch: 1,
                ..WeightCounts::default()
            }
        );
        assert_eq!(
            b.by_for_whom,
            ForWhomCounts {
                mich: 1,
                none: 1,
                ..ForWhomCounts::default()
            }
        );
        let before = week_balance(&state, week("2026-W43"), &zone(1));
        assert_eq!((before.total, before.by_for_whom.andere), (1, 1));
        assert_eq!(week_balance(&state, week("2026-W45"), &zone(1)).total, 0);
    }

    #[test]
    fn the_week_follows_the_local_day() {
        // Sunday 2026-11-01 23:30 CET is 22:30 UTC: still week 44 in Berlin
        // winter time, already Monday of week 45 at UTC+2.
        let sunday_late = 1_793_572_200_000;
        let (_, ops) = closed(NewTask::new("Spät"), TaskStatus::Done, sunday_late);
        let state = State::from_ops(&ops);
        assert_eq!(week_balance(&state, week("2026-W44"), &zone(1)).total, 1);
        assert_eq!(week_balance(&state, week("2026-W44"), &zone(2)).total, 0);
        assert_eq!(week_balance(&state, week("2026-W45"), &zone(2)).total, 1);
    }
}
