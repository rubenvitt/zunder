//! Planung: Heute als eine Reihenfolge, Input pro Ziel und Woche und das
//! Logbuch „Erledigt“ (design 5.1/5.2, 6.1 and 7).
//!
//! One rule each for the web UI (mirrored in `web/src/sync/planning.ts`),
//! the MCP tools and the server; the shared test vectors (`kind:
//! "today_list"`, `"goal_input"`, `"done_log"`) keep both sides in step.
//!
//! Common ground:
//! - "Live" means without tombstone. Open statuses are `open`,
//!   `in_progress`, `blocked` and `review`.
//! - A flame runs while `flame_started_at` is set and `flame_ended_at` is
//!   empty or older ([`flame_running`]). It counts until `now`.
//! - A flame counts whole minutes (rounded down per flame) for the local
//!   calendar day of its **start** in the caller's zone, even when it ends
//!   after midnight.
//! - The effective goal of a task is its own `goal_id`, otherwise the one of
//!   its root task; a deleted (or unknown) goal counts as none.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};

use serde::{Serialize, Serializer};
use time::{Date, Duration, OffsetDateTime};

use crate::balance::week_of_completion;
use crate::hlc::Hlc;
use crate::ids::Id;
use crate::model::{format_date, Due, GoalState, IsoWeek, Task, TaskStatus, Weight};
use crate::op::EntityKind;
use crate::recurrence::{resolve_local, to_local, LocalZone};
use crate::state::State;

fn date<S: Serializer>(d: &Date, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&format_date(*d))
}

/// Whether a flame runs on the task: started and not ended since.
pub fn flame_running(task: &Task) -> bool {
    match (task.flame_started_at, task.flame_ended_at) {
        (Some(_), None) => true,
        (Some(started), Some(ended)) => ended < started,
        _ => false,
    }
}

/// Whole minutes from `start` to `end` (rounded down, never negative).
fn whole_minutes(start: OffsetDateTime, end: OffsetDateTime) -> u32 {
    u32::try_from((end - start).whole_minutes().max(0)).unwrap_or(u32::MAX)
}

/// The flames of a task as (start, whole minutes): the finished ones from
/// `flame_log`, then the running one until `now`.
fn flames(task: &Task, now: OffsetDateTime) -> impl Iterator<Item = (OffsetDateTime, u32)> + '_ {
    let running = flame_running(task)
        .then_some(task.flame_started_at)
        .flatten()
        .map(|start| (start, whole_minutes(start, now)));
    task.flame_log
        .iter()
        .map(|f| (f.start, whole_minutes(f.start, f.end)))
        .chain(running)
}

fn local_day(instant: OffsetDateTime, zone: &impl LocalZone) -> Option<Date> {
    to_local(instant, zone).map(|local| local.date())
}

/// The local calendar day of a task's `status_changed_at`.
fn day_of_status_change(state: &State, task: &Task, zone: &impl LocalZone) -> Option<Date> {
    let ms = i128::from(state.status_changed_at(task.id)?.wall_ms());
    let instant = OffsetDateTime::from_unix_timestamp_nanos(ms * 1_000_000).ok()?;
    local_day(instant, zone)
}

/// Whether a task counts in the personal views (Logbuch): its effective
/// location is not a dev project.
fn personal(state: &State, task: &Task) -> bool {
    state
        .effective_location(task.id)
        .is_some_and(|location| !state.is_dev_location(location))
}

/// `goal` if it names a live goal.
fn live_goal(state: &State, goal: Option<Id>) -> Option<Id> {
    goal.filter(|g| state.live_entity(EntityKind::Goal, *g).is_some())
}

/// The effective goal of a task: its own `goal_id`, otherwise the one of its
/// root task (chain over `parent_id`). A deleted (or unknown) goal counts as
/// none, so a subtask whose own goal is deleted counts for its root's goal.
pub fn effective_goal(state: &State, task_id: Id) -> Option<Id> {
    effective_goal_by(state, task_id, |id| state.task(id).and_then(|t| t.goal_id))
}

/// [`effective_goal`] over a precomputed map of all tasks' own `goal_id`.
fn effective_goal_in(state: &State, own: &HashMap<Id, Option<Id>>, task: &Task) -> Option<Id> {
    effective_goal_by(state, task.id, |id| own.get(&id).copied().flatten())
}

/// The one rule behind [`effective_goal`]; `own_goal` gives a task's stored
/// `goal_id`.
fn effective_goal_by(
    state: &State,
    task_id: Id,
    own_goal: impl Fn(Id) -> Option<Id>,
) -> Option<Id> {
    live_goal(state, own_goal(task_id)).or_else(|| {
        let root = state.root_task(task_id)?;
        live_goal(state, own_goal(root))
    })
}

// ----- Heute ----------------------------------------------------------------

/// The rank a task without an explicit `rank` sorts by: its creation time in
/// ms as base 36, zero-padded to nine digits, plus the suffix `i` (exactly
/// `defaultRank` in `web/src/sync/rank.ts`), so explicit and implicit ranks
/// share one key space.
pub fn default_rank(created_ms: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut digits = Vec::new();
    let mut rest = created_ms;
    loop {
        digits.push(DIGITS[(rest % 36) as usize]);
        rest /= 36;
        if rest == 0 {
            break;
        }
    }
    while digits.len() < 9 {
        digits.push(b'0');
    }
    digits.reverse();
    digits.push(b'i');
    String::from_utf8(digits).expect("ASCII digits")
}

/// Heute as one list (design 5.1/5.2), as task IDs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TodayList {
    /// Planned for today, then left over from earlier days, then due by
    /// today without a plan.
    pub items: Vec<Id>,
    /// The running flame, otherwise (unless the plan is done) the first
    /// item that is open or in progress and waits on nobody.
    pub focus: Option<Id>,
    /// A live task of any status is planned for today.
    pub has_plan: bool,
    /// There is a plan and none of its tasks is open any more.
    pub plan_done: bool,
}

/// Order of the weights in lists: what matters first, unweighed in the
/// middle (as `weightRank` in the web client).
fn weight_rank(weight: Option<Weight>) -> u8 {
    match weight {
        Some(Weight::Brand) => 0,
        Some(Weight::Glut) => 1,
        None => 2,
        Some(Weight::Rauch) => 3,
        Some(Weight::Asche) => 4,
    }
}

/// Present values first (in string order), missing ones last.
fn cmp_present_first(a: Option<&str>, b: Option<&str>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => a.cmp(b),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// A candidate with what its group sorts by.
struct Entry {
    task: Task,
    created: Hlc,
    /// Group 3 only: the due instant (a plain date is local midnight).
    due_at: Option<OffsetDateTime>,
}

/// The list order of a task as `compareTasks` in the web client: `rank`,
/// else [`default_rank`] of the creation time.
fn sort_key(task: &Task, created: &Hlc) -> String {
    task.rank
        .clone()
        .unwrap_or_else(|| default_rank(created.wall_ms()))
}

impl Entry {
    fn by_creation(&self, other: &Self) -> Ordering {
        self.created
            .cmp(&other.created)
            .then(self.task.id.cmp(&other.task.id))
    }

    fn by_day_rank(&self, other: &Self) -> Ordering {
        cmp_present_first(
            self.task.day_rank.as_deref(),
            other.task.day_rank.as_deref(),
        )
        .then_with(|| self.by_creation(other))
    }
}

/// The local date and instant of a due value in `zone` (a plain date is due
/// at local midnight).
fn due_in(due: Due, zone: &impl LocalZone) -> Option<(Date, OffsetDateTime)> {
    match due {
        Due::Date(d) => Some((d, resolve_local(d.midnight(), zone)?)),
        Due::DateTime(at) => Some((local_day(at, zone)?, at)),
    }
}

/// Heute for the local date `today` in `zone` (design 5.1/5.2). Candidates
/// are live tasks with an open status (subtasks and dev projects included):
/// 1. `planned_on == today`, by `day_rank` (missing last), creation, ID;
/// 2. `planned_on < today`, by `planned_on`, `day_rank`, creation, ID;
/// 3. no `planned_on` and due by today, by weight (Brand, Glut, none,
///    Rauch, Asche), due instant, then list order (`rank`, else
///    [`default_rank`]), creation, ID: as `byDue` in the web client.
pub fn today_list(state: &State, today: Date, zone: &impl LocalZone) -> TodayList {
    let mut planned = Vec::new();
    let mut left_over = Vec::new();
    let mut due = Vec::new();
    let mut has_plan = false;
    let mut plan_open = false;
    // The running flame; of several, the latest started, then the smallest ID.
    let mut flame: Option<(OffsetDateTime, Id)> = None;
    for task in state.live_tasks() {
        if let (true, Some(started)) = (flame_running(&task), task.flame_started_at) {
            if flame.is_none_or(|(s, id)| started > s || (started == s && task.id < id)) {
                flame = Some((started, task.id));
            }
        }
        let open = !task.status.is_closed();
        if task.planned_on == Some(today) {
            has_plan = true;
            plan_open |= open;
        }
        if !open {
            continue;
        }
        let Some(created) = state.created_at(EntityKind::Task, task.id).cloned() else {
            continue;
        };
        match task.planned_on {
            Some(day) if day == today => planned.push(Entry {
                task,
                created,
                due_at: None,
            }),
            Some(day) if day < today => left_over.push(Entry {
                task,
                created,
                due_at: None,
            }),
            Some(_) => {}
            None => {
                if let Some((day, at)) = task.due.and_then(|d| due_in(d, zone)) {
                    if day <= today {
                        due.push(Entry {
                            task,
                            created,
                            due_at: Some(at),
                        });
                    }
                }
            }
        }
    }
    planned.sort_by(Entry::by_day_rank);
    left_over.sort_by(|a, b| {
        a.task
            .planned_on
            .cmp(&b.task.planned_on)
            .then_with(|| a.by_day_rank(b))
    });
    due.sort_by(|a, b| {
        weight_rank(a.task.weight)
            .cmp(&weight_rank(b.task.weight))
            .then(a.due_at.cmp(&b.due_at))
            .then_with(|| sort_key(&a.task, &a.created).cmp(&sort_key(&b.task, &b.created)))
            .then_with(|| a.by_creation(b))
    });
    let items: Vec<Task> = planned
        .into_iter()
        .chain(left_over)
        .chain(due)
        .map(|e| e.task)
        .collect();
    let plan_done = has_plan && !plan_open;
    let focus = match flame {
        Some((_, id)) => Some(id),
        None if plan_done => None,
        None => items
            .iter()
            .find(|t| {
                matches!(t.status, TaskStatus::Open | TaskStatus::InProgress)
                    && t.waiting_on.is_none()
            })
            .map(|t| t.id),
    };
    TodayList {
        items: items.iter().map(|t| t.id).collect(),
        focus,
        has_plan,
        plan_done,
    }
}

// ----- Ziele ----------------------------------------------------------------

/// Input of one goal in one week.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GoalInput {
    pub goal_id: Id,
    /// Done tasks with this effective goal (moves, "Züge").
    pub done: u32,
    /// Flame minutes on tasks with this effective goal.
    pub minutes: u32,
    pub lead_per_week: Option<u32>,
    /// `lead_per_week` is set and `done` reaches it.
    pub reached: bool,
}

/// Input per active goal in one ISO week (design 6.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WeekGoalInput {
    pub week: IsoWeek,
    /// Monday of the week (`JJJJ-MM-TT`).
    #[serde(serialize_with = "date")]
    pub from: Date,
    /// Sunday of the week (`JJJJ-MM-TT`).
    #[serde(serialize_with = "date")]
    pub to: Date,
    /// Live active goals by `rank` (missing last), then name, then ID.
    pub goals: Vec<GoalInput>,
}

/// Input of every live active goal in `week` in `zone`:
/// - `done`: live tasks with status `done` whose `status_changed_at` falls
///   into the week and whose effective goal it is (subtasks one by one, dev
///   projects included, dropped tasks not);
/// - `minutes`: flames of live tasks with this effective goal (any status)
///   that started in the week, the running one until `now`.
pub fn goal_input(
    state: &State,
    week: IsoWeek,
    zone: &impl LocalZone,
    now: OffsetDateTime,
) -> WeekGoalInput {
    let from = week.monday();
    let to = from.saturating_add(Duration::days(6));
    let mut goals: Vec<_> = state
        .live_goals()
        .filter(|g| g.state == GoalState::Active)
        .collect();
    goals.sort_by(|a, b| {
        cmp_present_first(a.rank.as_deref(), b.rank.as_deref())
            .then_with(|| a.name.cmp(&b.name))
            .then(a.id.cmp(&b.id))
    });
    let mut rows: Vec<GoalInput> = goals
        .iter()
        .map(|g| GoalInput {
            goal_id: g.id,
            done: 0,
            minutes: 0,
            lead_per_week: g.lead_per_week,
            reached: false,
        })
        .collect();
    let row_of: HashMap<Id, usize> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| (r.goal_id, i))
        .collect();
    let own: HashMap<Id, Option<Id>> = state.tasks().map(|t| (t.id, t.goal_id)).collect();
    for task in state.live_tasks() {
        let Some(&i) = effective_goal_in(state, &own, &task).and_then(|g| row_of.get(&g)) else {
            continue;
        };
        let row = &mut rows[i];
        if task.status == TaskStatus::Done && week_of_completion(state, &task, zone) == Some(week) {
            row.done += 1;
        }
        for (start, minutes) in flames(&task, now) {
            if local_day(start, zone).is_some_and(|day| from <= day && day <= to) {
                row.minutes = row.minutes.saturating_add(minutes);
            }
        }
    }
    for row in &mut rows {
        row.reached = row.lead_per_week.is_some_and(|n| row.done >= n);
    }
    WeekGoalInput {
        week,
        from,
        to,
        goals: rows,
    }
}

// ----- Logbuch --------------------------------------------------------------

/// A task in the Logbuch with its flame minutes on that day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LogEntry {
    pub task_id: Id,
    pub minutes: u32,
}

/// One day of the Logbuch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogDay {
    #[serde(serialize_with = "date")]
    pub date: Date,
    /// Flame minutes of both lists.
    pub minutes: u32,
    /// Done that day, the latest first.
    pub done: Vec<LogEntry>,
    /// Flames that day but not done that day, most minutes first, then ID.
    pub worked: Vec<LogEntry>,
}

#[derive(Default)]
struct DayLists {
    /// (status_changed_at, entry)
    done: Vec<(Hlc, LogEntry)>,
    worked: Vec<LogEntry>,
}

/// The Logbuch from `to` back to `from` (both included) in `zone` (design
/// 7); days without entries are left out. Tasks in dev projects never
/// appear.
/// - `done`: live tasks with status `done` whose `status_changed_at` falls
///   on the day, with their flame minutes of that day;
/// - `worked`: live tasks (not dropped) with flame minutes that day that are
///   not in that day's `done`.
pub fn done_log(
    state: &State,
    from: Date,
    to: Date,
    zone: &impl LocalZone,
    now: OffsetDateTime,
) -> Vec<LogDay> {
    let mut days: BTreeMap<Date, DayLists> = BTreeMap::new();
    let in_range = |day: Date| from <= day && day <= to;
    for task in state.live_tasks() {
        if task.status == TaskStatus::Dropped || !personal(state, &task) {
            continue;
        }
        let mut per_day: BTreeMap<Date, u32> = BTreeMap::new();
        for (start, minutes) in flames(&task, now) {
            if let Some(day) = local_day(start, zone).filter(|d| in_range(*d)) {
                let sum = per_day.entry(day).or_default();
                *sum = sum.saturating_add(minutes);
            }
        }
        let done_day = (task.status == TaskStatus::Done)
            .then(|| day_of_status_change(state, &task, zone))
            .flatten()
            .filter(|d| in_range(*d));
        if let (Some(day), Some(changed)) = (done_day, state.status_changed_at(task.id)) {
            let minutes = per_day.get(&day).copied().unwrap_or(0);
            days.entry(day).or_default().done.push((
                changed.clone(),
                LogEntry {
                    task_id: task.id,
                    minutes,
                },
            ));
        }
        for (day, minutes) in per_day {
            if minutes > 0 && Some(day) != done_day {
                days.entry(day).or_default().worked.push(LogEntry {
                    task_id: task.id,
                    minutes,
                });
            }
        }
    }
    days.into_iter()
        .rev()
        .map(|(date, mut lists)| {
            lists
                .done
                .sort_by(|(a, x), (b, y)| b.cmp(a).then(x.task_id.cmp(&y.task_id)));
            lists
                .worked
                .sort_by(|a, b| b.minutes.cmp(&a.minutes).then(a.task_id.cmp(&b.task_id)));
            let done: Vec<LogEntry> = lists.done.into_iter().map(|(_, e)| e).collect();
            let minutes = done
                .iter()
                .chain(&lists.worked)
                .fold(0u32, |sum, e| sum.saturating_add(e.minutes));
            LogDay {
                date,
                minutes,
                done,
                worked: lists.worked,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use time::macros::{date, datetime};
    use time::UtcOffset;

    use super::*;
    use crate::builder::{NewTask, OpBatch};
    use crate::hlc::{Clock, NodeId};
    use crate::model::FlameInterval;
    use crate::recurrence::FixedZone;

    fn cet() -> FixedZone {
        FixedZone(UtcOffset::from_hms(1, 0, 0).unwrap())
    }

    fn task_with_flames(log: Vec<FlameInterval>) -> Task {
        let mut clock = Clock::new(NodeId::new("t").unwrap());
        let mut batch = OpBatch::new(&mut clock, 1_790_000_000_000);
        let id = batch.create_task(NewTask::new("Flamme"));
        let state = State::from_ops(&batch.into_ops());
        Task {
            flame_log: log,
            ..state.task(id).unwrap()
        }
    }

    #[test]
    fn flame_runs_until_it_ends_after_its_start() {
        let mut task = task_with_flames(vec![]);
        assert!(!flame_running(&task));
        task.flame_started_at = Some(datetime!(2026-11-23 08:00 UTC));
        assert!(flame_running(&task));
        task.flame_ended_at = Some(datetime!(2026-11-22 08:00 UTC));
        assert!(
            flame_running(&task),
            "an older end belongs to an earlier flame"
        );
        task.flame_ended_at = Some(datetime!(2026-11-23 08:00 UTC));
        assert!(!flame_running(&task), "ended at its start");
    }

    #[test]
    fn minutes_round_down_per_flame_and_the_running_one_counts_until_now() {
        let mut task = task_with_flames(vec![
            FlameInterval {
                start: datetime!(2026-11-23 08:00 UTC),
                end: datetime!(2026-11-23 08:00:59 UTC),
            },
            FlameInterval {
                start: datetime!(2026-11-23 09:00 UTC),
                end: datetime!(2026-11-23 09:10:30 UTC),
            },
        ]);
        task.flame_started_at = Some(datetime!(2026-11-23 10:00 UTC));
        let now = datetime!(2026-11-23 10:05:59 UTC);
        let minutes: Vec<u32> = flames(&task, now).map(|(_, m)| m).collect();
        assert_eq!(minutes, [0, 10, 5]);
        // A clock behind the start does not count negative minutes.
        let before = datetime!(2026-11-23 09:59 UTC);
        assert_eq!(flames(&task, before).map(|(_, m)| m).last(), Some(0));
    }

    /// A goal, a deleted goal, a root with the goal and subtasks below it
    /// (one with the deleted goal of its own).
    fn goal_state() -> (State, Id, [Id; 4]) {
        let mut clock = Clock::new(NodeId::new("t").unwrap());
        let mut batch = OpBatch::new(&mut clock, 1_790_000_000_000);
        let (goal, gone) = (Id::new(), Id::new());
        batch.set_goal(goal, "name", "Sport");
        batch
            .set_goal(gone, "name", "Weg")
            .set_goal(gone, "deleted", true);
        let root = batch.create_task(NewTask::new("Wurzel"));
        let child = batch.create_task(NewTask::subtask("Kind", root));
        let own = batch.create_task(NewTask::subtask("Eigenes Ziel", root));
        let grandchild = batch.create_task(NewTask::subtask("Enkel", child));
        batch.set_task(root, "goal_id", goal.to_string());
        batch.set_task(own, "goal_id", gone.to_string());
        (
            State::from_ops(&batch.into_ops()),
            goal,
            [root, child, own, grandchild],
        )
    }

    #[test]
    fn effective_goal_is_the_own_or_the_roots_and_a_deleted_goal_is_none() {
        let (state, goal, [root, child, own, grandchild]) = goal_state();
        assert_eq!(effective_goal(&state, root), Some(goal));
        assert_eq!(effective_goal(&state, child), Some(goal));
        assert_eq!(effective_goal(&state, grandchild), Some(goal));
        // A deleted own goal counts as none: the root's goal applies.
        assert_eq!(effective_goal(&state, own), Some(goal));
        assert_eq!(effective_goal(&state, Id::new()), None);
        // The bulk variant follows the same rule.
        let map: HashMap<Id, Option<Id>> = state.tasks().map(|t| (t.id, t.goal_id)).collect();
        let own_task = state.task(own).unwrap();
        assert_eq!(effective_goal_in(&state, &map, &own_task), Some(goal));
    }

    #[test]
    fn a_root_with_a_deleted_goal_has_none() {
        let mut clock = Clock::new(NodeId::new("t").unwrap());
        let mut batch = OpBatch::new(&mut clock, 1_790_000_000_000);
        let gone = Id::new();
        batch
            .set_goal(gone, "name", "Weg")
            .set_goal(gone, "deleted", true);
        let root = batch.create_task(NewTask::new("Wurzel"));
        let child = batch.create_task(NewTask::subtask("Kind", root));
        batch.set_task(root, "goal_id", gone.to_string());
        let state = State::from_ops(&batch.into_ops());
        assert_eq!(effective_goal(&state, root), None);
        assert_eq!(effective_goal(&state, child), None);
    }

    #[test]
    fn default_rank_matches_the_web_client() {
        // Values of defaultRank in web/src/sync/rank.ts.
        assert_eq!(default_rank(0), "000000000i");
        assert_eq!(default_rank(1), "000000001i");
        assert_eq!(default_rank(35), "00000000zi");
        assert_eq!(default_rank(1_790_000_000_000), "0mubbs7i8i");
        assert_eq!(default_rank(1_793_185_200_123), "0mvs063nfi");
        assert_eq!(default_rank(99_999_999_999_999), "zg3d62r5ri");
        // Wider than nine digits: no truncation.
        assert_eq!(default_rank(200_000_000_000_000), "1yw6qc5ibki");
    }

    #[test]
    fn an_empty_state_has_no_plan_no_input_rows_and_no_log() {
        let state = State::new();
        let today = today_list(&state, date!(2026 - 11 - 23), &cet());
        assert_eq!(
            today,
            TodayList {
                items: vec![],
                focus: None,
                has_plan: false,
                plan_done: false
            }
        );
        let now = datetime!(2026-11-23 12:00 UTC);
        let week = IsoWeek::parse("2026-W48").unwrap();
        let input = serde_json::to_value(goal_input(&state, week, &cet(), now)).unwrap();
        assert_eq!(
            input,
            serde_json::json!({"week": "2026-W48", "from": "2026-11-23", "to": "2026-11-29", "goals": []})
        );
        assert!(done_log(
            &state,
            date!(2026 - 11 - 01),
            date!(2026 - 11 - 30),
            &cet(),
            now
        )
        .is_empty());
        // `from` after `to` is an empty range, not an error.
        assert!(done_log(
            &state,
            date!(2026 - 11 - 30),
            date!(2026 - 11 - 01),
            &cet(),
            now
        )
        .is_empty());
    }
}
