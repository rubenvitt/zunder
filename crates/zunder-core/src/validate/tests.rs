use super::*;
use crate::builder::{NewArea, NewProject, NewTask, OpBatch};
use crate::hlc::{Clock, NodeId};
use crate::model::{ProjectKind, TaskStatus, TaskType};
use crate::testutil::{h, op, uid};
use serde_json::{json, Value};

const NOW: u64 = 1_760_000_000_000;

thread_local! {
    /// Number of entity-check passes run on this thread.
    pub(super) static CHECK_PASSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
const T: EntityKind = EntityKind::Task;
const P: EntityKind = EntityKind::Project;
const A: EntityKind = EntityKind::Area;

/// IDs of the fixture entities.
struct Ids {
    area: Id,
    area2: Id,
    standard: Id,
    dev: Id,
    dev2: Id,
    inbox_task: Id,
    bug: Id,
    parent: Id,
    feature_child: Id,
    standard_task: Id,
    area_task: Id,
}

fn fixture() -> (State, Ids) {
    let mut clock = Clock::new(NodeId::new("fixture").unwrap());
    let mut b = OpBatch::new(&mut clock, NOW - 60_000);
    let area = b.create_area(NewArea::new("Arbeit"));
    let area2 = b.create_area(NewArea::new("Privat"));
    let standard = b.create_project(NewProject::new("Standard", area));
    let dev = b.create_project(NewProject {
        kind: ProjectKind::Dev,
        repo: Some("rubeen/zunder".into()),
        ..NewProject::new("Dev", area)
    });
    let dev2 = b.create_project(NewProject {
        kind: ProjectKind::Dev,
        ..NewProject::new("Dev 2", area2)
    });
    let in_dev = Location::Project { id: dev };
    let inbox_task = b.create_task(NewTask::new("Inbox"));
    let bug = b.create_task(NewTask::new("Bug").at(in_dev).with_type(TaskType::Bug));
    let parent = b.create_task(NewTask::new("Epic").at(in_dev));
    let feature_child =
        b.create_task(NewTask::subtask("Feature", parent).with_type(TaskType::Feature));
    let standard_task =
        b.create_task(NewTask::new("Standard").at(Location::Project { id: standard }));
    let area_task = b.create_task(NewTask::new("Bereich").at(Location::Area { id: area }));
    let ops = b.into_ops();
    let outcome = validate_push(&State::new(), &ops, NOW);
    assert!(
        outcome.is_fully_accepted(),
        "fixture invalid: {:?}",
        outcome.rejected
    );
    let state = State::from_ops(&outcome.new_ops);
    (
        state,
        Ids {
            area,
            area2,
            standard,
            dev,
            dev2,
            inbox_task,
            bug,
            parent,
            feature_child,
            standard_task,
            area_task,
        },
    )
}

/// Builds ops on a fresh client clock. Each call uses a later physical time,
/// so batches built one after another in a test are ordered.
fn batch(build: impl FnOnce(&mut OpBatch)) -> Vec<Op> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static TICK: AtomicU64 = AtomicU64::new(0);
    let physical = NOW + TICK.fetch_add(1, Ordering::Relaxed);
    let mut clock = Clock::new(NodeId::new("client").unwrap());
    let mut b = OpBatch::new(&mut clock, physical);
    build(&mut b);
    b.into_ops()
}

fn push(state: &State, ops: &[Op]) -> PushOutcome {
    validate_push(state, ops, NOW)
}

fn assert_accepted(outcome: &PushOutcome) {
    assert!(
        outcome.is_fully_accepted(),
        "unexpected rejections: {:#?}",
        outcome.rejected
    );
}

/// Asserts every op was rejected with `code`.
fn assert_all_rejected(outcome: &PushOutcome, ops: &[Op], code: RejectCode) {
    assert!(
        outcome.accepted.is_empty(),
        "accepted: {:?}",
        outcome.accepted
    );
    assert_eq!(outcome.rejected.len(), ops.len());
    for r in &outcome.rejected {
        assert_eq!(r.code, code, "{r:?}");
        assert!(!r.message.is_empty());
    }
}

fn codes(outcome: &PushOutcome) -> Vec<RejectCode> {
    outcome.rejected.iter().map(|r| r.code).collect()
}

fn loc(location: Location) -> Value {
    location.to_value()
}

#[test]
fn plain_create_is_accepted() {
    let (state, _) = fixture();
    let ops = batch(|b| {
        b.create_task(NewTask::new("Zahnarzt anrufen"));
    });
    let outcome = push(&state, &ops);
    assert_accepted(&outcome);
    assert_eq!(
        outcome.accepted,
        ops.iter().map(|o| o.op_id).collect::<Vec<_>>()
    );
    assert_eq!(outcome.new_ops, ops);
    assert!(outcome.warnings.is_empty());
}

#[test]
fn known_op_ids_are_accepted_without_reapplying() {
    let (mut state, _) = fixture();
    let ops = batch(|b| {
        b.create_task(NewTask::new("A"));
    });
    for op in &ops {
        state.apply(op);
    }
    // Same op IDs again, even with different content: accepted, not new.
    let mut replay = ops.clone();
    replay[0].value = json!("anders");
    let outcome = push(&state, &replay);
    assert_accepted(&outcome);
    assert_eq!(outcome.accepted.len(), ops.len());
    assert!(outcome.new_ops.is_empty());

    // Repeats within one push count once.
    let fresh = batch(|b| {
        b.create_task(NewTask::new("B"));
    });
    let mut doubled = fresh.clone();
    doubled.extend(fresh.iter().cloned());
    let outcome = push(&state, &doubled);
    assert_accepted(&outcome);
    assert_eq!(outcome.accepted.len(), doubled.len());
    assert_eq!(outcome.new_ops, fresh);
}

#[test]
fn repeat_of_a_rejected_op_is_rejected_too() {
    let (state, _) = fixture();
    let ops = vec![
        op(951, T, 950, "title", json!("Irgendwo"), h(NOW, 0, "c")),
        op(
            952,
            T,
            950,
            "location",
            loc(Location::Project { id: uid(999) }),
            h(NOW, 1, "c"),
        ),
    ];
    let mut pushed = ops.clone();
    pushed.push(ops[1].clone());
    let outcome = push(&state, &pushed);
    assert!(outcome.accepted.is_empty(), "{:?}", outcome.accepted);
    assert!(outcome.new_ops.is_empty());
    let rejected: Vec<(Id, RejectCode)> =
        outcome.rejected.iter().map(|r| (r.op_id, r.code)).collect();
    assert_eq!(
        rejected,
        vec![
            (uid(951), RejectCode::InvalidReference),
            (uid(952), RejectCode::InvalidReference),
            (uid(952), RejectCode::InvalidReference),
        ]
    );
    assert_eq!(outcome.rejected[1].message, outcome.rejected[2].message);

    // A repeat of an accepted op is accepted and stored once.
    let valid = batch(|b| {
        b.create_task(NewTask::new("Einmal").at(Location::Inbox));
    });
    let mut pushed = valid.clone();
    pushed.push(valid[0].clone());
    let outcome = push(&state, &pushed);
    assert_accepted(&outcome);
    assert_eq!(outcome.accepted.len(), pushed.len());
    assert_eq!(outcome.new_ops, valid);
}

#[test]
fn unknown_field_rejects_whole_entity_only() {
    let (state, ids) = fixture();
    let ops = batch(|b| {
        b.set_task(ids.inbox_task, "title", "neu");
        b.set_task(ids.inbox_task, "priority", 1);
        b.set_task(ids.standard_task, "title", "unberührt");
    });
    let outcome = push(&state, &ops);
    assert_eq!(
        codes(&outcome),
        vec![RejectCode::EntityRejected, RejectCode::UnknownField]
    );
    assert_eq!(outcome.rejected[0].op_id, ops[0].op_id);
    assert!(outcome.rejected[0].message.contains("priority"));
    assert_eq!(outcome.accepted, vec![ops[2].op_id]);
    assert_eq!(outcome.new_ops, vec![ops[2].clone()]);
}

#[test]
fn invalid_values_are_rejected() {
    let (state, ids) = fixture();
    for (field, value) in [
        ("status", json!("closed")),
        ("title", json!("")),
        ("title", json!(null)),
        ("due", json!("morgen")),
        ("remind_at", json!("2026-10-12")),
        ("undated_ack", json!(true)),
        ("phoenix", json!({"role": "flamme", "fire": "2026-W60"})),
        ("pr_url", json!("not a url")),
        ("links", json!([{"url": "relative/path"}])),
        ("parent_id", json!("nope")),
        ("location", json!({"kind": "project"})),
        ("deleted", json!("yes")),
    ] {
        let ops = batch(|b| {
            b.set_task(ids.inbox_task, field, value.clone());
        });
        assert_all_rejected(&push(&state, &ops), &ops, RejectCode::InvalidValue);
    }
    let ops = batch(|b| {
        b.set_project(ids.dev, "repo", "kein-repo");
    });
    assert_all_rejected(&push(&state, &ops), &ops, RejectCode::InvalidValue);
}

#[test]
fn new_entities_need_required_fields() {
    let (state, ids) = fixture();
    let task_without_title = batch(|b| {
        b.set_task(uid(500), "status", "open");
        b.set_task(uid(500), "description", "kein Titel");
    });
    assert_all_rejected(
        &push(&state, &task_without_title),
        &task_without_title,
        RejectCode::MissingRequired,
    );

    let project_without_area = batch(|b| {
        b.set_project(uid(501), "name", "Ohne Bereich");
    });
    assert_all_rejected(
        &push(&state, &project_without_area),
        &project_without_area,
        RejectCode::MissingRequired,
    );

    let area_without_name = batch(|b| {
        b.set_area(uid(502), "state", "active");
    });
    assert_all_rejected(
        &push(&state, &area_without_name),
        &area_without_name,
        RejectCode::MissingRequired,
    );

    // Existing entities already have them: partial updates are fine.
    let update = batch(|b| {
        b.set_project(ids.standard, "status", "paused");
    });
    assert_accepted(&push(&state, &update));
}

#[test]
fn dev_only_types_and_review() {
    let (state, ids) = fixture();
    let cases = [
        NewTask::new("Bug in Inbox").with_type(TaskType::Bug),
        NewTask::new("Feature lose")
            .at(Location::Loose)
            .with_type(TaskType::Feature),
        NewTask::new("Review im Standardprojekt")
            .at(Location::Project { id: ids.standard })
            .with_status(TaskStatus::Review),
        NewTask::new("Bug im Bereich")
            .at(Location::Area { id: ids.area })
            .with_type(TaskType::Bug),
        NewTask::subtask("Bug unter Inbox-Aufgabe", ids.inbox_task).with_type(TaskType::Bug),
    ];
    for new in cases {
        let ops = batch(|b| {
            b.create_task(new.clone());
        });
        assert_all_rejected(&push(&state, &ops), &ops, RejectCode::DevOnly);
    }
    let allowed = [
        NewTask::new("Bug")
            .at(Location::Project { id: ids.dev })
            .with_type(TaskType::Bug),
        NewTask::new("Review")
            .at(Location::Project { id: ids.dev2 })
            .with_status(TaskStatus::Review),
        NewTask::subtask("Feature-Unteraufgabe", ids.parent).with_type(TaskType::Feature),
    ];
    for new in allowed {
        let ops = batch(|b| {
            b.create_task(new.clone());
        });
        assert_accepted(&push(&state, &ops));
    }
    // Setting the type on an existing standard task.
    let ops = batch(|b| {
        b.set_task(ids.standard_task, "type", "bug");
    });
    assert_all_rejected(&push(&state, &ops), &ops, RejectCode::DevOnly);
}

#[test]
fn status_transitions_are_free() {
    let (state, ids) = fixture();
    let ops = batch(|b| {
        b.set_status(ids.inbox_task, TaskStatus::Done);
        b.set_status(ids.inbox_task, TaskStatus::Open);
        b.set_status(ids.standard_task, TaskStatus::Blocked);
        b.set_status(ids.standard_task, TaskStatus::Dropped);
    });
    assert_accepted(&push(&state, &ops));
}

#[test]
fn dev_tasks_cannot_leave_dev_projects() {
    let (state, ids) = fixture();
    let ops = batch(|b| {
        b.move_task(ids.bug, Location::Project { id: ids.standard });
    });
    let outcome = push(&state, &ops);
    assert_all_rejected(&outcome, &ops, RejectCode::DevOnly);
    assert!(
        outcome.rejected[0].message.contains("Dev-Projekt heraus"),
        "{}",
        outcome.rejected[0].message
    );

    let ops = batch(|b| {
        b.move_task(ids.bug, Location::Inbox);
    });
    assert_all_rejected(&push(&state, &ops), &ops, RejectCode::DevOnly);

    // Detaching a feature subtask into the inbox.
    let ops = batch(|b| {
        b.move_task(ids.feature_child, Location::Inbox);
    });
    assert_all_rejected(&push(&state, &ops), &ops, RejectCode::DevOnly);

    // Into another dev project is fine.
    let ops = batch(|b| {
        b.move_task(ids.bug, Location::Project { id: ids.dev2 });
    });
    assert_accepted(&push(&state, &ops));

    // Moving the bug together with changing its type is fine.
    let ops = batch(|b| {
        b.move_task(ids.bug, Location::Inbox);
        b.set_task(ids.bug, "type", "aufgabe");
    });
    assert_accepted(&push(&state, &ops));
}

#[test]
fn moving_a_parent_with_dev_subtasks_is_blamed_on_the_parent() {
    let (state, ids) = fixture();
    let ops = batch(|b| {
        b.set_task(ids.parent, "location", loc(Location::Loose));
        b.set_task(ids.feature_child, "title", "nur umbenannt");
        b.set_task(ids.inbox_task, "title", "unbeteiligt");
    });
    let outcome = push(&state, &ops);
    assert_eq!(codes(&outcome), vec![RejectCode::DevTasksPresent]);
    assert_eq!(outcome.rejected[0].op_id, ops[0].op_id);
    assert!(outcome.rejected[0].message.contains("nur umbenannt"));
    assert_eq!(outcome.accepted, vec![ops[1].op_id, ops[2].op_id]);

    // Re-parenting the epic under a standard task fails the same way.
    let ops = batch(|b| {
        b.set_parent(ids.parent, Some(ids.standard_task));
    });
    assert_all_rejected(&push(&state, &ops), &ops, RejectCode::DevTasksPresent);
}

#[test]
fn dev_project_to_standard_requires_no_dev_tasks() {
    let (state, ids) = fixture();
    let ops = batch(|b| {
        b.set_project(ids.dev, "kind", "standard");
        b.clear(P, ids.dev, "repo");
    });
    let outcome = push(&state, &ops);
    assert_all_rejected(&outcome, &ops, RejectCode::DevTasksPresent);

    // dev2 has no tasks.
    let ops = batch(|b| {
        b.set_project(ids.dev2, "kind", "standard");
    });
    assert_accepted(&push(&state, &ops));

    // Downgrading together with fixing all dev tasks in the same push.
    let ops = batch(|b| {
        b.set_project(ids.dev, "kind", "standard");
        b.clear(P, ids.dev, "repo");
        b.set_task(ids.bug, "type", "aufgabe");
        b.set_task(ids.feature_child, "type", "aufgabe");
    });
    assert_accepted(&push(&state, &ops));
}

#[test]
fn repo_only_on_dev_projects() {
    let (state, ids) = fixture();
    let ops = batch(|b| {
        b.set_project(ids.standard, "repo", "rubeen/zunder");
    });
    assert_all_rejected(&push(&state, &ops), &ops, RejectCode::DevOnly);

    // Downgrade keeping the repo is rejected; dev2 has no tasks.
    let mut clock = Clock::new(NodeId::new("c").unwrap());
    let mut b = OpBatch::new(&mut clock, NOW);
    b.set_project(ids.dev2, "repo", "rubeen/other");
    let mut state2 = state.clone();
    for op in b.ops() {
        state2.apply(op);
    }
    let ops = batch(|b| {
        b.set_project(ids.dev2, "kind", "standard");
    });
    assert_all_rejected(&push(&state2, &ops), &ops, RejectCode::DevOnly);
}

#[test]
fn references_must_resolve_to_live_entities_of_the_right_kind() {
    let (mut state, ids) = fixture();
    // A deleted, dependency-free project.
    let deleted_project = uid(600);
    for op in batch(|b| {
        b.create_project(NewProject {
            id: Some(deleted_project),
            ..NewProject::new("Weg", ids.area)
        });
        b.delete(P, deleted_project);
    }) {
        state.apply(&op);
    }
    let bad_locations = [
        Location::Project { id: uid(999) },
        Location::Project {
            id: deleted_project,
        },
        Location::Area { id: uid(999) },
        // Wrong kind: an area ID used as project.
        Location::Project { id: ids.area },
    ];
    for location in bad_locations {
        let ops = batch(|b| {
            b.set_task(ids.inbox_task, "location", location);
        });
        assert_all_rejected(&push(&state, &ops), &ops, RejectCode::InvalidReference);
        let ops = batch(|b| {
            b.create_task(NewTask::new("neu").at(location));
        });
        assert_all_rejected(&push(&state, &ops), &ops, RejectCode::InvalidReference);
    }
    let ops = batch(|b| {
        b.set_parent(ids.inbox_task, Some(uid(999)));
    });
    assert_all_rejected(&push(&state, &ops), &ops, RejectCode::InvalidReference);
    let ops = batch(|b| {
        b.set_project(ids.standard, "area_id", uid(999).to_string());
    });
    assert_all_rejected(&push(&state, &ops), &ops, RejectCode::InvalidReference);
    let ops = batch(|b| {
        b.create_project(NewProject::new("x", ids.standard));
    });
    assert_all_rejected(&push(&state, &ops), &ops, RejectCode::InvalidReference);

    // Moving a project to another area is fine.
    let ops = batch(|b| {
        b.set_project(ids.standard, "area_id", ids.area2.to_string());
    });
    assert_accepted(&push(&state, &ops));
}

#[test]
fn own_location_of_subtasks_is_ignored() {
    let (state, ids) = fixture();
    let ops = batch(|b| {
        b.set_task(
            ids.feature_child,
            "location",
            loc(Location::Project { id: uid(999) }),
        );
    });
    assert_accepted(&push(&state, &ops));
}

#[test]
fn parent_cycles_are_rejected() {
    let (state, ids) = fixture();
    let ops = batch(|b| {
        b.set_parent(ids.parent, Some(ids.feature_child));
    });
    assert_all_rejected(&push(&state, &ops), &ops, RejectCode::Cycle);
    let ops = batch(|b| {
        b.set_parent(ids.inbox_task, Some(ids.inbox_task));
    });
    assert_all_rejected(&push(&state, &ops), &ops, RejectCode::Cycle);

    // Swapping parent and child in one push.
    let ops = batch(|b| {
        b.set_parent(ids.feature_child, None);
        b.set_task(
            ids.feature_child,
            "location",
            loc(Location::Project { id: ids.dev }),
        );
        b.set_parent(ids.parent, Some(ids.feature_child));
    });
    assert_accepted(&push(&state, &ops));
}

#[test]
fn deleting_requires_no_live_dependents() {
    let (state, ids) = fixture();
    for (kind, id) in [
        (A, ids.area),
        (P, ids.dev),
        (P, ids.standard),
        (T, ids.parent),
    ] {
        let ops = batch(|b| {
            b.delete(kind, id);
        });
        let outcome = push(&state, &ops);
        assert_all_rejected(&outcome, &ops, RejectCode::HasDependents);
    }
    // Area with only a directly located task.
    let ops = batch(|b| {
        b.set_project(ids.standard, "area_id", ids.area2.to_string());
        b.set_project(ids.dev, "area_id", ids.area2.to_string());
        b.delete(A, ids.area);
    });
    let outcome = push(&state, &ops);
    assert_eq!(codes(&outcome), vec![RejectCode::HasDependents]);

    // Cascades are fine.
    for (kind, id) in [
        (A, ids.area),
        (P, ids.dev),
        (T, ids.parent),
        (T, ids.area_task),
    ] {
        let ops = batch(|b| {
            b.delete_cascade(&state, kind, id);
        });
        assert_accepted(&push(&state, &ops));
    }
    // Leaf deletion and moving dependents away first.
    let ops = batch(|b| {
        b.move_task(ids.area_task, Location::Inbox);
        b.delete(T, ids.inbox_task);
    });
    assert_accepted(&push(&state, &ops));
}

#[test]
fn restoring_requires_valid_references() {
    let (mut state, ids) = fixture();
    for op in batch(|b| {
        b.delete_cascade(&state, T, ids.parent);
    }) {
        state.apply(&op);
    }
    let ops = batch(|b| {
        b.set_task(ids.feature_child, "deleted", false);
    });
    assert_all_rejected(&push(&state, &ops), &ops, RejectCode::InvalidReference);
    let ops = batch(|b| {
        b.set_task(ids.parent, "deleted", false);
        b.set_task(ids.feature_child, "deleted", false);
    });
    assert_accepted(&push(&state, &ops));
}

#[test]
fn future_hlcs_are_rejected() {
    let (state, ids) = fixture();
    let hour = 3_600_000;
    let far = op(700, T, 0, "title", json!("x"), h(NOW + 25 * hour, 0, "c"));
    let far = Op {
        entity_id: ids.inbox_task,
        ..far
    };
    let near = Op {
        op_id: uid(701),
        hlc: h(NOW + 23 * hour, 0, "c"),
        ..far.clone()
    };
    let outcome = push(&state, std::slice::from_ref(&far));
    assert_eq!(codes(&outcome), vec![RejectCode::FutureHlc]);
    assert_accepted(&push(&state, &[near]));
}

#[test]
fn entity_ids_are_unique_across_kinds() {
    let (state, ids) = fixture();
    let ops = batch(|b| {
        b.set_area(ids.inbox_task, "name", "Bereich mit Aufgaben-ID");
    });
    assert_all_rejected(&push(&state, &ops), &ops, RejectCode::KindMismatch);
    let ops = batch(|b| {
        b.set_area(uid(800), "name", "x");
        b.set_task(uid(800), "title", "x");
    });
    assert_all_rejected(&push(&state, &ops), &ops, RejectCode::KindMismatch);
}

#[test]
fn rejection_cascades_to_entities_depending_on_rejected_ones() {
    let (state, ids) = fixture();
    let ops = batch(|b| {
        // Project without area → missing_required ...
        b.set_project(uid(900), "name", "Kaputt");
        // ... so the task inside it has a dangling reference.
        b.create_task(NewTask::new("Drin").at(Location::Project { id: uid(900) }));
        b.set_task(ids.inbox_task, "title", "unabhängig");
    });
    let outcome = push(&state, &ops);
    let by_entity: HashMap<Id, RejectCode> = outcome
        .rejected
        .iter()
        .map(|r| {
            (
                ops.iter().find(|o| o.op_id == r.op_id).unwrap().entity_id,
                r.code,
            )
        })
        .collect();
    assert_eq!(by_entity[&uid(900)], RejectCode::MissingRequired);
    let task_id = ops[1].entity_id;
    assert_eq!(by_entity[&task_id], RejectCode::InvalidReference);
    assert_eq!(outcome.accepted, vec![ops.last().unwrap().op_id]);
}

#[test]
fn deep_dependency_chains_are_rejected_in_one_pass() {
    let (mut state, _) = fixture();
    // A larger state, so that per-pass costs would show.
    let filler = batch(|b| {
        for i in 0..2_000 {
            b.create_task(NewTask::new(format!("Füller {i}")));
        }
    });
    for op in &filler {
        state.apply(op);
    }
    const DEPTH: usize = 200;
    let mut ops = batch(|b| {
        let mut parent = b.create_task(NewTask::new("Wurzel"));
        for i in 0..DEPTH {
            parent = b.create_task(NewTask::subtask(format!("Ebene {i}"), parent));
        }
    });
    let root = ops[0].entity_id;
    // Without a title the root is rejected (missing_required) ...
    ops.retain(|o| !(o.entity_id == root && o.field == "title"));

    CHECK_PASSES.with(|n| n.set(0));
    let outcome = push(&state, &ops);
    let passes = CHECK_PASSES.with(|n| n.get());
    // ... and with it the whole chain below it.
    assert!(outcome.accepted.is_empty());
    assert!(outcome.new_ops.is_empty());
    assert_eq!(outcome.rejected.len(), ops.len());
    let mut causes: HashMap<Id, RejectCode> = HashMap::new();
    for r in &outcome.rejected {
        let entity = ops.iter().find(|o| o.op_id == r.op_id).unwrap().entity_id;
        causes.entry(entity).or_insert(r.code);
        if entity != root && r.code == RejectCode::InvalidReference {
            assert!(
                r.message.contains("übergeordnete Aufgabe") && r.message.contains("Pflichtfeld"),
                "{}",
                r.message
            );
        }
    }
    assert_eq!(causes.len(), DEPTH + 1);
    assert_eq!(causes[&root], RejectCode::MissingRequired);
    assert!(causes
        .iter()
        .all(|(id, code)| *id == root || *code == RejectCode::InvalidReference));
    assert!(passes <= 2, "{passes} check passes");

    // With the title the same chain is accepted in a single pass.
    let valid = batch(|b| {
        let mut parent = b.create_task(NewTask::new("Wurzel"));
        for i in 0..DEPTH {
            parent = b.create_task(NewTask::subtask(format!("Ebene {i}"), parent));
        }
    });
    CHECK_PASSES.with(|n| n.set(0));
    assert_accepted(&push(&state, &valid));
    assert_eq!(CHECK_PASSES.with(|n| n.get()), 1);
}

#[test]
fn chains_into_rejected_projects_and_areas_cascade() {
    let (state, ids) = fixture();
    let ops = batch(|b| {
        // Area without name → missing_required; a project in it, a task in
        // the project and a subtask of that task depend on it.
        b.set_area(uid(960), "state", "active");
        b.create_project(NewProject::new("Im Bereich", uid(960)));
        b.set_task(ids.inbox_task, "title", "unabhängig");
    });
    let project = ops
        .iter()
        .find(|o| o.entity == P)
        .map(|o| o.entity_id)
        .unwrap();
    let mut ops = ops;
    ops.extend(batch(|b| {
        let t = b.create_task(NewTask::new("Im Projekt").at(Location::Project { id: project }));
        b.create_task(NewTask::subtask("Darunter", t));
    }));
    CHECK_PASSES.with(|n| n.set(0));
    let outcome = push(&state, &ops);
    assert!(CHECK_PASSES.with(|n| n.get()) <= 2);
    let accepted: Vec<Id> = ops
        .iter()
        .filter(|o| o.entity_id == ids.inbox_task)
        .map(|o| o.op_id)
        .collect();
    assert_eq!(outcome.accepted, accepted);
    let mut causes: HashMap<Id, RejectCode> = HashMap::new();
    for r in &outcome.rejected {
        let entity = ops.iter().find(|o| o.op_id == r.op_id).unwrap().entity_id;
        if r.code != RejectCode::EntityRejected {
            causes.entry(entity).or_insert(r.code);
        }
    }
    assert_eq!(causes[&uid(960)], RejectCode::MissingRequired);
    assert_eq!(causes.len(), 4);
    assert!(causes
        .iter()
        .all(|(id, code)| *id == uid(960) || *code == RejectCode::InvalidReference));
}

#[test]
fn a_small_push_copies_only_what_it_touches() {
    let (mut state, ids) = fixture();
    let filler = batch(|b| {
        for i in 0..500 {
            b.create_task(NewTask::new(format!("Füller {i}")));
        }
    });
    for op in &filler {
        state.apply(op);
    }
    let ops = batch(|b| {
        b.set_task(ids.inbox_task, "title", "neu");
    });
    let mut overlay = Overlay::new(&state);
    for op in &ops {
        overlay.apply(op);
    }
    assert_eq!(overlay.copied(), 1);
    assert_accepted(&push(&state, &ops));
}

#[test]
fn done_with_open_subtasks_warns() {
    let (state, ids) = fixture();
    let ops = batch(|b| {
        b.set_status(ids.parent, TaskStatus::Done);
    });
    let outcome = push(&state, &ops);
    assert_accepted(&outcome);
    assert_eq!(outcome.warnings.len(), 1);
    assert_eq!(outcome.warnings[0].entity_id, ids.parent);
    assert_eq!(outcome.warnings[0].code, WarningCode::OpenSubtasks);
    assert!(outcome.warnings[0].message.contains("1 offene"));

    let ops = batch(|b| {
        b.set_status(ids.feature_child, TaskStatus::Done);
        b.set_status(ids.parent, TaskStatus::Done);
    });
    let outcome = push(&state, &ops);
    assert_accepted(&outcome);
    assert!(outcome.warnings.is_empty());

    let ops = batch(|b| {
        b.set_status(ids.feature_child, TaskStatus::Dropped);
        b.set_status(ids.parent, TaskStatus::Done);
    });
    assert!(push(&state, &ops).warnings.is_empty());
}

#[test]
fn preexisting_inconsistencies_are_not_blamed_on_unrelated_changes() {
    let (mut state, ids) = fixture();
    // Unvalidated merge: a bug in a standard project.
    state.apply(&Op {
        op_id: uid(1000),
        entity: T,
        entity_id: ids.standard_task,
        field: "type".into(),
        value: json!("bug"),
        hlc: h(NOW - 1, 0, "legacy"),
    });
    let ops = batch(|b| {
        b.set_project(ids.standard, "name", "Umbenannt");
    });
    assert_accepted(&push(&state, &ops));
    // Touching the inconsistent task itself requires fixing it.
    let ops = batch(|b| {
        b.set_task(ids.standard_task, "title", "neu");
    });
    assert_all_rejected(&push(&state, &ops), &ops, RejectCode::DevOnly);
}

#[test]
fn outcome_serializes_like_the_push_response() {
    let (state, ids) = fixture();
    let ops = batch(|b| {
        b.set_status(ids.parent, TaskStatus::Done);
        b.set_task(ids.inbox_task, "nope", 1);
    });
    let v = serde_json::to_value(push(&state, &ops)).unwrap();
    assert_eq!(v["accepted"], json!([ops[0].op_id.to_string()]));
    assert_eq!(v["rejected"][0]["op_id"], json!(ops[1].op_id.to_string()));
    assert_eq!(v["rejected"][0]["code"], json!("unknown_field"));
    assert_eq!(v["warnings"][0]["entity_id"], json!(ids.parent.to_string()));
    assert_eq!(v["warnings"][0]["code"], json!("open_subtasks"));
    assert!(v.get("new_ops").is_none());
    assert_eq!(v.as_object().unwrap().len(), 3);
}

#[test]
fn reject_codes_are_stable_strings() {
    for code in [
        RejectCode::UnknownField,
        RejectCode::InvalidValue,
        RejectCode::MissingRequired,
        RejectCode::DevOnly,
        RejectCode::DevTasksPresent,
        RejectCode::InvalidReference,
        RejectCode::HasDependents,
        RejectCode::Cycle,
        RejectCode::FutureHlc,
        RejectCode::KindMismatch,
        RejectCode::EntityRejected,
    ] {
        assert_eq!(serde_json::to_value(code).unwrap(), json!(code.as_str()));
    }
    assert_eq!(
        serde_json::to_value(WarningCode::OpenSubtasks).unwrap(),
        json!("open_subtasks")
    );
}

#[test]
fn empty_push() {
    let (state, _) = fixture();
    assert_eq!(push(&state, &[]), PushOutcome::default());
}

// ----- properties -----------------------------------------------------------

mod properties {
    use super::*;
    use proptest::prelude::*;

    /// All violations in `state`, treating every entity and field as touched.
    fn invariant_violations(state: &State) -> BTreeMap<Key, Violation> {
        let mut all_fields: HashSet<&str> = HashSet::new();
        for kind in EntityKind::ALL {
            all_fields.extend(fields::all(kind));
        }
        let changed: HashMap<Key, HashSet<&str>> = state
            .entities()
            .map(|e| ((e.kind(), e.id()), all_fields.clone()))
            .collect();
        let seeds: BTreeSet<Key> = changed.keys().copied().collect();
        check_entities(state, &Overlay::new(state), &changed, &seeds)
    }

    const AREAS: [u128; 2] = [1, 2];
    const PROJECTS: [u128; 3] = [11, 12, 13];
    const TASKS: [u128; 5] = [21, 22, 23, 24, 25];

    fn id_value(pool: &'static [u128]) -> impl Strategy<Value = Value> {
        prop::sample::select(pool).prop_map(|n| json!(uid(n).to_string()))
    }

    fn arb_op() -> impl Strategy<Value = (EntityKind, u128, &'static str, Value)> {
        let area = (
            prop::sample::select(&AREAS[..]),
            prop_oneof![
                Just(("name", json!("Bereich"))),
                prop::bool::ANY.prop_map(|b| ("deleted", json!(b))),
            ],
        )
            .prop_map(|(id, (f, v))| (A, id, f, v));
        let project = (
            prop::sample::select(&PROJECTS[..]),
            prop_oneof![
                Just(("name", json!("Projekt"))),
                id_value(&AREAS).prop_map(|v| ("area_id", v)),
                prop::sample::select(vec!["standard", "dev"]).prop_map(|k| ("kind", json!(k))),
                prop_oneof![Just(Value::Null), Just(json!("a/b"))].prop_map(|v| ("repo", v)),
                prop::bool::ANY.prop_map(|b| ("deleted", json!(b))),
            ],
        )
            .prop_map(|(id, (f, v))| (P, id, f, v));
        let location = prop_oneof![
            Just(json!({"kind": "inbox"})),
            prop::sample::select(&AREAS[..])
                .prop_map(|n| json!({"kind": "area", "id": uid(n).to_string()})),
            prop::sample::select(&PROJECTS[..])
                .prop_map(|n| json!({"kind": "project", "id": uid(n).to_string()})),
        ];
        let task = (
            prop::sample::select(&TASKS[..]),
            prop_oneof![
                Just(("title", json!("Aufgabe"))),
                location.prop_map(|v| ("location", v)),
                prop_oneof![Just(Value::Null), id_value(&TASKS)].prop_map(|v| ("parent_id", v)),
                prop::sample::select(vec!["aufgabe", "bug", "feature"])
                    .prop_map(|t| ("type", json!(t))),
                prop::sample::select(vec!["open", "review", "done"])
                    .prop_map(|s| ("status", json!(s))),
                prop::bool::ANY.prop_map(|b| ("deleted", json!(b))),
            ],
        )
            .prop_map(|(id, (f, v))| (T, id, f, v));
        prop_oneof![1 => area, 2 => project, 4 => task]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// Whatever clients push, the validated state never violates an
        /// invariant: every rejection is attributed so that the accepted
        /// remainder is consistent.
        #[test]
        fn validated_state_stays_consistent(
            pushes in prop::collection::vec(prop::collection::vec(arb_op(), 1..8), 1..25)
        ) {
            let mut state = State::new();
            let mut clock = Clock::new(NodeId::new("p").unwrap());
            let mut next_op = 0u128;
            for (round, push_ops) in pushes.into_iter().enumerate() {
                let ops: Vec<Op> = push_ops
                    .into_iter()
                    .map(|(kind, id, field, value)| {
                        next_op += 1;
                        Op {
                            op_id: uid(10_000 + next_op),
                            entity: kind,
                            entity_id: uid(id),
                            field: field.to_owned(),
                            value,
                            hlc: clock.now(NOW + round as u64),
                        }
                    })
                    .collect();
                let outcome = validate_push(&state, &ops, NOW + 100);
                prop_assert_eq!(outcome.accepted.len() + outcome.rejected.len(), ops.len());
                for op in &outcome.new_ops {
                    state.apply(op);
                }
                let violations = invariant_violations(&state);
                prop_assert!(violations.is_empty(), "round {}: {:?}", round, violations);
            }
        }
    }
}

#[test]
fn comments_need_a_live_task_but_do_not_block_deleting_it() {
    let (state, ids) = fixture();
    let mut comment = None;
    let ops = batch(|b| {
        comment = Some(b.add_comment(ids.inbox_task, "Stand: angefangen.", Some("claude".into())));
    });
    let outcome = push(&state, &ops);
    assert_accepted(&outcome);
    let comment = comment.unwrap();
    let mut state = state;
    for op in &outcome.new_ops {
        state.apply(op);
    }
    assert_eq!(state.comments_of(ids.inbox_task).len(), 1);
    assert_eq!(
        state.comment(comment).unwrap().author.as_deref(),
        Some("claude")
    );

    // Deleting the task leaves the comment alone; it is hidden with the task.
    let ops = batch(|b| {
        b.delete_cascade(&state, T, ids.inbox_task);
    });
    assert_eq!(ops.len(), 1);
    let outcome = push(&state, &ops);
    assert_accepted(&outcome);
    for op in &outcome.new_ops {
        state.apply(op);
    }

    // Editing a comment of a deleted task is harmless; moving or restoring
    // one onto a deleted task is not.
    let ops = batch(|b| {
        b.set_comment(comment, "body", "nachgetragen");
    });
    assert_accepted(&push(&state, &ops));
    let ops = batch(|b| {
        b.add_comment(ids.inbox_task, "zu spät", None);
    });
    assert_all_rejected(&push(&state, &ops), &ops, RejectCode::InvalidReference);
    let ops = batch(|b| {
        b.set_comment(comment, "task_id", ids.bug.to_string());
    });
    assert_accepted(&push(&state, &ops));
}

#[test]
fn comments_on_a_rejected_new_task_are_rejected_with_it() {
    let (state, ids) = fixture();
    let ops = batch(|b| {
        let task = b.create_task(NewTask::new("Bug").with_type(TaskType::Bug));
        b.add_comment(task, "Notiz", None);
        b.set_task(ids.inbox_task, "title", "unabhängig");
    });
    let outcome = push(&state, &ops);
    let comment_ops: Vec<&Rejection> = outcome
        .rejected
        .iter()
        .filter(|r| {
            ops.iter()
                .any(|o| o.op_id == r.op_id && o.entity == EntityKind::Comment)
        })
        .collect();
    assert_eq!(comment_ops.len(), 2);
    for r in comment_ops {
        assert_eq!(r.code, RejectCode::InvalidReference);
        assert!(r.message.contains("die Aufgabe"), "{}", r.message);
    }
    assert_eq!(outcome.accepted, vec![ops.last().unwrap().op_id]);
}

#[test]
fn comment_values_are_checked() {
    let (state, ids) = fixture();
    let ops = batch(|b| {
        b.add_comment(ids.inbox_task, " \n ", None);
    });
    assert!(codes(&push(&state, &ops)).contains(&RejectCode::InvalidValue));
    let ops = batch(|b| {
        b.add_comment(ids.inbox_task, "ok", Some(String::new()));
    });
    assert!(codes(&push(&state, &ops)).contains(&RejectCode::InvalidValue));
    let ops = batch(|b| {
        b.set_comment(uid(901), "task_id", ids.inbox_task.to_string());
    });
    assert_all_rejected(&push(&state, &ops), &ops, RejectCode::MissingRequired);
}
