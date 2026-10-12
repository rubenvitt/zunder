//! Project keys and task references (`ZUN-42`).
//!
//! A project may carry a `key`. Every live task whose effective location is
//! such a project gets a reference `<KEY>-<n>`. Numbers count per key, start
//! at 1 and are never reused (tombstoned tasks keep theirs).
//!
//! Only the server assigns references: clients work offline and would hand
//! out the same number twice. A client push that writes `ref` is rejected
//! (see [`crate::validate::Author`]). Once set, a reference stays, even when
//! the task moves to another project or the project's key changes, so
//! branch names, commits and notes keep pointing to the right task.

use std::collections::{BTreeMap, HashMap, HashSet};

use uuid::Uuid;

use crate::model::{fields, Location, Task};
use crate::op::{EntityKind, Op};
use crate::state::{Entity, State};

/// Maximum length of a project key.
pub const MAX_KEY_CHARS: usize = 10;

/// Largest task number (nine digits).
const MAX_NUMBER: u32 = 999_999_999;

/// A project key: an uppercase letter, then uppercase letters or digits, at
/// most [`MAX_KEY_CHARS`] characters (`ZUN`, `APP2`).
pub fn is_valid_key(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=MAX_KEY_CHARS).contains(&b.len())
        && b[0].is_ascii_uppercase()
        && b.iter()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// Splits a reference `KEY-N` into key and number. The number is at least 1,
/// has no leading zeros and at most nine digits.
pub fn parse_ref(s: &str) -> Option<(&str, u32)> {
    let (key, number) = s.split_once('-')?;
    let digits_ok = !number.is_empty()
        && number.len() <= 9
        && !number.starts_with('0')
        && number.bytes().all(|c| c.is_ascii_digit());
    if !is_valid_key(key) || !digits_ok {
        return None;
    }
    Some((key, number.parse().ok()?))
}

/// Canonical form of a reference typed by a person or agent (`zun-42`,
/// ` ZUN-42 `), or `None` if it is none.
pub fn normalize_ref(input: &str) -> Option<String> {
    let s = input.trim().to_ascii_uppercase();
    parse_ref(&s).is_some().then_some(s)
}

/// The reference of number `n` under `key`.
pub fn format_ref(key: &str, n: u32) -> String {
    format!("{key}-{n}")
}

/// Which tasks [`pending_refs`] looks at.
#[derive(Debug, Clone, Copy)]
pub enum Scope<'a> {
    /// Every task (e.g. at startup).
    All,
    /// Tasks a push may have brought into a project with a key: the touched
    /// tasks with their subtasks, and the tasks of projects whose key was set
    /// or that were restored.
    Ops(&'a [Op]),
}

impl State {
    /// Finds a task by its reference (canonical form), tombstoned ones
    /// included only if `include_deleted`.
    pub fn task_by_ref(&self, reference: &str, include_deleted: bool) -> Option<Task> {
        self.entities_of(EntityKind::Task)
            .find(|e| {
                (include_deleted || !e.is_deleted())
                    && e.str_value(fields::task::REF) == Some(reference)
            })
            .and_then(|e| self.task(e.id()))
    }

    /// The key of a live project, if it has one.
    pub fn project_key(&self, project_id: Uuid) -> Option<&str> {
        self.live_entity(EntityKind::Project, project_id)?
            .str_value(fields::project::KEY)
    }

    /// The live project with this key, if any.
    pub fn project_by_key(&self, key: &str) -> Option<Uuid> {
        self.entities_of(EntityKind::Project)
            .find(|e| !e.is_deleted() && e.str_value(fields::project::KEY) == Some(key))
            .map(Entity::id)
    }
}

/// References to assign now, as `(task, reference)` in assignment order:
/// every live task in `scope` without a reference whose effective project
/// has a key. Per key, tasks are numbered in creation order, continuing
/// after the largest number ever given under that key.
pub fn pending_refs(state: &State, scope: Scope<'_>) -> Vec<(Uuid, String)> {
    let candidates: Vec<Uuid> = match scope {
        Scope::All => state
            .entities_of(EntityKind::Task)
            .map(Entity::id)
            .collect(),
        Scope::Ops(ops) => candidates_of(state, ops),
    };
    // key -> tasks to number
    let mut needed: BTreeMap<String, Vec<&Entity>> = BTreeMap::new();
    for id in candidates {
        let Some(task) = state.live_entity(EntityKind::Task, id) else {
            continue;
        };
        if task.value(fields::task::REF).is_some() {
            continue;
        }
        let Some(project) = state.effective_project(id) else {
            continue;
        };
        if let Some(key) = state.project_key(project) {
            needed.entry(key.to_owned()).or_default().push(task);
        }
    }
    if needed.is_empty() {
        return Vec::new();
    }
    let mut last: HashMap<String, u32> = needed.keys().map(|k| (k.clone(), 0)).collect();
    for e in state.entities_of(EntityKind::Task) {
        if let Some((key, n)) = e.str_value(fields::task::REF).and_then(parse_ref) {
            if let Some(max) = last.get_mut(key) {
                *max = (*max).max(n);
            }
        }
    }
    let mut out = Vec::new();
    for (key, mut tasks) in needed {
        tasks.sort_by(|a, b| a.created_at().cmp(b.created_at()).then(a.id().cmp(&b.id())));
        let mut n = last[&key];
        for task in tasks {
            if n >= MAX_NUMBER {
                break;
            }
            n += 1;
            out.push((task.id(), format_ref(&key, n)));
        }
    }
    out
}

fn candidates_of(state: &State, ops: &[Op]) -> Vec<Uuid> {
    let mut seen: HashSet<Uuid> = HashSet::new();
    let mut out = Vec::new();
    let mut add = |id: Uuid, out: &mut Vec<Uuid>| {
        if seen.insert(id) {
            out.push(id);
        }
    };
    let mut projects: HashSet<Uuid> = HashSet::new();
    // Tasks whose subtree was added (a push holds many ops per task).
    let mut expanded: HashSet<Uuid> = HashSet::new();
    for op in ops {
        match op.entity {
            EntityKind::Task if expanded.insert(op.entity_id) => {
                add(op.entity_id, &mut out);
                for d in state.descendants(op.entity_id) {
                    add(d, &mut out);
                }
            }
            EntityKind::Project
                if op.field == fields::project::KEY || op.field == fields::DELETED =>
            {
                projects.insert(op.entity_id);
            }
            _ => {}
        }
    }
    if !projects.is_empty() {
        for task in state.entities_of(EntityKind::Task) {
            let in_project = matches!(
                state.effective_location(task.id()),
                Some(Location::Project { id }) if projects.contains(&id)
            );
            if in_project {
                add(task.id(), &mut out);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{NewArea, NewProject, NewTask, OpBatch};
    use crate::hlc::{Clock, NodeId};

    #[test]
    fn keys_and_refs() {
        for ok in ["Z", "ZUN", "APP2", "ABCDEFGHIJ"] {
            assert!(is_valid_key(ok), "{ok}");
        }
        for bad in ["", "zun", "2UN", "ZU-N", "ZÜN", "ABCDEFGHIJK", "ZUN "] {
            assert!(!is_valid_key(bad), "{bad}");
        }
        assert_eq!(parse_ref("ZUN-42"), Some(("ZUN", 42)));
        assert_eq!(parse_ref("A1-999999999"), Some(("A1", 999_999_999)));
        for bad in [
            "ZUN-0",
            "ZUN-042",
            "ZUN-",
            "ZUN42",
            "zun-1",
            "ZUN-1-2",
            "ZUN-1000000000",
        ] {
            assert_eq!(parse_ref(bad), None, "{bad}");
        }
        assert_eq!(normalize_ref(" zun-7 "), Some("ZUN-7".into()));
        assert_eq!(normalize_ref("Zahnarzt"), None);
    }

    #[test]
    fn numbers_tasks_of_keyed_projects_in_creation_order() {
        let mut clock = Clock::new(NodeId::new("t").unwrap());
        let mut b = OpBatch::new(&mut clock, 1_760_000_000_000);
        let area = b.create_area(NewArea::new("A"));
        let zun = b.create_project(NewProject {
            key: Some("ZUN".into()),
            ..NewProject::new("Zunder", area)
        });
        let plain = b.create_project(NewProject::new("Ohne", area));
        let first = b.create_task(NewTask::new("1").at(Location::Project { id: zun }));
        let sub = b.create_task(NewTask::subtask("1.1", first));
        let other = b.create_task(NewTask::new("ohne").at(Location::Project { id: plain }));
        let inbox = b.create_task(NewTask::new("inbox"));
        let mut state = State::from_ops(b.ops());

        let refs = pending_refs(&state, Scope::All);
        assert_eq!(
            refs,
            vec![(first, "ZUN-1".to_owned()), (sub, "ZUN-2".to_owned())]
        );
        let mut b = OpBatch::new(&mut clock, 1_760_000_000_000);
        for (id, r) in &refs {
            b.set_task(*id, fields::task::REF, r.clone());
        }
        // The inbox task moves in; the other project gets a key as well.
        b.move_task(inbox, Location::Project { id: zun });
        b.set_project(plain, fields::project::KEY, "OHN");
        let ops = b.into_ops();
        for op in &ops {
            state.apply(op);
        }
        assert_eq!(
            pending_refs(&state, Scope::Ops(&ops)),
            vec![(other, "OHN-1".to_owned()), (inbox, "ZUN-3".to_owned())]
        );
        assert_eq!(state.task_by_ref("ZUN-2", false).unwrap().id, sub);
        assert_eq!(state.project_by_key("ZUN"), Some(zun));
    }

    #[test]
    fn numbers_are_never_reused() {
        let mut clock = Clock::new(NodeId::new("t").unwrap());
        let mut b = OpBatch::new(&mut clock, 1_760_000_000_000);
        let area = b.create_area(NewArea::new("A"));
        let zun = b.create_project(NewProject {
            key: Some("ZUN".into()),
            ..NewProject::new("Zunder", area)
        });
        let old = b.create_task(NewTask::new("alt").at(Location::Project { id: zun }));
        b.set_task(old, fields::task::REF, "ZUN-7");
        b.delete(EntityKind::Task, old);
        let new = b.create_task(NewTask::new("neu").at(Location::Project { id: zun }));
        let state = State::from_ops(b.ops());
        assert_eq!(
            pending_refs(&state, Scope::All),
            vec![(new, "ZUN-8".to_owned())]
        );
    }
}
