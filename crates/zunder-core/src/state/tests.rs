use super::*;
use crate::testutil::{h, op, uid};
use proptest::prelude::*;
use serde_json::json;

const T: EntityKind = EntityKind::Task;
const P: EntityKind = EntityKind::Project;
const A: EntityKind = EntityKind::Area;

fn apply_all(ops: &[Op]) -> State {
    State::from_ops(ops)
}

#[test]
fn later_hlc_wins_in_any_order() {
    let a = op(1, T, 10, "title", json!("alt"), h(1000, 0, "a"));
    let b = op(2, T, 10, "title", json!("neu"), h(2000, 0, "b"));
    for ops in [vec![a.clone(), b.clone()], vec![b.clone(), a.clone()]] {
        let s = apply_all(&ops);
        assert_eq!(s.task(uid(10)).unwrap().title, "neu");
    }
}

#[test]
fn equal_hlc_is_broken_by_op_id() {
    // Same HLC string is only possible for buggy clients, but must be stable.
    let a = op(5, T, 10, "title", json!("fünf"), h(1000, 0, "a"));
    let b = op(9, T, 10, "title", json!("neun"), h(1000, 0, "a"));
    assert_eq!(
        apply_all(&[a.clone(), b.clone()])
            .task(uid(10))
            .unwrap()
            .title,
        "neun"
    );
    assert_eq!(apply_all(&[b, a]).task(uid(10)).unwrap().title, "neun");
}

#[test]
fn identical_hlc_and_op_id_still_converge() {
    let a = op(5, T, 10, "title", json!("a"), h(1000, 0, "a"));
    let b = op(5, T, 10, "title", json!("b"), h(1000, 0, "a"));
    assert_eq!(apply_all(&[a.clone(), b.clone()]), apply_all(&[b, a]));
}

#[test]
fn apply_reports_changes_and_is_idempotent() {
    let mut s = State::new();
    let a = op(1, T, 10, "title", json!("x"), h(1000, 0, "a"));
    assert!(s.apply(&a));
    assert!(!s.apply(&a));
    let older = op(2, T, 10, "title", json!("y"), h(500, 0, "a"));
    // Loses the register but lowers created_at.
    assert!(s.apply(&older));
    assert_eq!(s.task(uid(10)).unwrap().title, "x");
    assert!(!s.apply(&older));
    assert!(s.knows_op(&uid(2)));
    assert!(!s.knows_op(&uid(3)));
}

#[test]
fn null_resets_to_default() {
    let ops = [
        op(1, T, 10, "title", json!("x"), h(1, 0, "a")),
        op(2, T, 10, "status", json!("done"), h(2, 0, "a")),
        op(3, T, 10, "due", json!("2026-10-12"), h(3, 0, "a")),
        op(4, T, 10, "status", json!(null), h(4, 0, "a")),
        op(5, T, 10, "due", json!(null), h(5, 0, "a")),
    ];
    let s = apply_all(&ops);
    let task = s.task(uid(10)).unwrap();
    assert_eq!(task.status, TaskStatus::Open);
    assert_eq!(task.due, None);
    let e = s.entity(T, uid(10)).unwrap();
    assert_eq!(e.field("due").unwrap().value, Value::Null);
    assert_eq!(e.value("due"), None);
}

#[test]
fn tombstones_hide_but_keep_entities() {
    let ops = [
        op(1, T, 10, "title", json!("x"), h(1, 0, "a")),
        op(2, T, 10, "deleted", json!(true), h(2, 0, "a")),
        op(3, T, 11, "title", json!("y"), h(1, 0, "a")),
    ];
    let s = apply_all(&ops);
    assert!(s.task(uid(10)).unwrap().deleted);
    assert_eq!(s.tasks().count(), 2);
    assert_eq!(
        s.live_tasks().map(|t| t.id).collect::<Vec<_>>(),
        vec![uid(11)]
    );
    assert!(s.live_entity(T, uid(10)).is_none());
    assert!(!s.is_visible_by_default(uid(10), 0));
}

#[test]
fn derived_timestamps() {
    let ops = [
        op(1, T, 10, "title", json!("x"), h(1000, 0, "a")),
        op(2, T, 10, "title", json!("y"), h(3000, 0, "a")),
        op(3, T, 10, "status", json!("in_progress"), h(2000, 0, "a")),
    ];
    let s = apply_all(&ops);
    assert_eq!(s.created_at(T, uid(10)), Some(&h(1000, 0, "a")));
    assert_eq!(s.updated_at(T, uid(10)), Some(&h(3000, 0, "a")));
    assert_eq!(s.status_changed_at(uid(10)), Some(&h(2000, 0, "a")));

    let fresh = apply_all(&ops[..1]);
    assert_eq!(fresh.status_changed_at(uid(10)), Some(&h(1000, 0, "a")));
    assert_eq!(s.created_at(T, uid(99)), None);
}

#[test]
fn typed_views_apply_defaults() {
    let ops = [
        op(1, A, 1, "name", json!("Arbeit"), h(1, 0, "a")),
        op(2, P, 2, "name", json!("Zunder"), h(1, 1, "a")),
        op(3, P, 2, "area_id", json!(uid(1).to_string()), h(1, 2, "a")),
        op(4, P, 3, "name", json!("Dev"), h(1, 3, "a")),
        op(5, P, 3, "area_id", json!(uid(1).to_string()), h(1, 4, "a")),
        op(6, P, 3, "kind", json!("dev"), h(1, 5, "a")),
        op(7, P, 3, "repo", json!("rubeen/zunder"), h(1, 6, "a")),
        op(8, T, 4, "title", json!("Aufgabe"), h(1, 7, "a")),
        op(9, T, 5, "description", json!("ohne Titel"), h(1, 8, "a")),
    ];
    let s = apply_all(&ops);
    let area = s.area(uid(1)).unwrap();
    assert_eq!(area.state, AreaState::Active);
    assert_eq!(area.vault_path, None);
    let p = s.project(uid(2)).unwrap();
    assert_eq!(
        (p.kind, p.status, p.view),
        (
            ProjectKind::Standard,
            ProjectStatus::Active,
            ProjectView::List
        )
    );
    let dev = s.project(uid(3)).unwrap();
    assert_eq!(dev.view, ProjectView::Board);
    assert_eq!(dev.repo.as_deref(), Some("rubeen/zunder"));
    let t = s.task(uid(4)).unwrap();
    assert_eq!(t.task_type, TaskType::Aufgabe);
    assert_eq!(t.status, TaskStatus::Open);
    assert_eq!(t.location, Location::Inbox);
    assert!(t.vault_links.is_empty() && t.links.is_empty());
    // Missing required field: no typed view, but the raw entity exists.
    assert!(s.task(uid(5)).is_none());
    assert!(s.entity(T, uid(5)).is_some());
    assert_eq!(s.areas().count(), 1);
    assert_eq!(s.projects().count(), 2);
}

#[test]
fn task_view_serializes_with_contract_names() {
    let ops = [
        op(1, T, 4, "title", json!("Aufgabe"), h(1, 0, "a")),
        op(
            2,
            T,
            4,
            "phoenix",
            json!({"role": "flamme", "fire": "2026-W45"}),
            h(1, 1, "a"),
        ),
        op(
            3,
            T,
            4,
            "remind_at",
            json!("2026-10-12T09:00:00+02:00"),
            h(1, 2, "a"),
        ),
        op(
            4,
            T,
            4,
            "undated_ack",
            json!("2026-10-12T08:00:00Z"),
            h(1, 3, "a"),
        ),
        op(5, T, 4, "weight", json!("glut"), h(1, 4, "a")),
        op(6, T, 4, "for_whom", json!("andere"), h(1, 5, "a")),
        op(7, T, 4, "deferrals", json!(2), h(1, 6, "a")),
        op(8, T, 4, "waiting_on", json!("Anna"), h(1, 7, "a")),
    ];
    let s = apply_all(&ops);
    let task = s.task(uid(4)).unwrap();
    assert_eq!(task.weight, Some(crate::Weight::Glut));
    assert_eq!(task.for_whom, Some(crate::ForWhom::Andere));
    assert_eq!(task.deferrals, 2);
    assert_eq!(task.waiting_on.as_deref(), Some("Anna"));
    let v = serde_json::to_value(task).unwrap();
    assert_eq!(v["type"], json!("aufgabe"));
    assert_eq!(v["location"], json!({"kind": "inbox"}));
    assert_eq!(v["phoenix"], json!({"role": "flamme", "fire": "2026-W45"}));
    assert_eq!(v["remind_at"], json!("2026-10-12T09:00:00+02:00"));
    assert_eq!(v["undated_ack"], json!("2026-10-12T08:00:00Z"));
    assert_eq!(v["weight"], json!("glut"));
    assert_eq!(v["for_whom"], json!("andere"));
    assert_eq!(v["deferrals"], json!(2));
    assert_eq!(v["waiting_on"], json!("Anna"));
    assert_eq!(v["parent_id"], Value::Null);
}

fn tree_ops() -> Vec<Op> {
    let project = json!({"kind": "project", "id": uid(2).to_string()});
    vec![
        op(1, A, 1, "name", json!("Arbeit"), h(1, 0, "a")),
        op(2, P, 2, "name", json!("Zunder"), h(1, 1, "a")),
        op(3, P, 2, "area_id", json!(uid(1).to_string()), h(1, 2, "a")),
        op(4, T, 10, "title", json!("Wurzel"), h(1, 3, "a")),
        op(5, T, 10, "location", project, h(1, 4, "a")),
        op(6, T, 11, "title", json!("Kind"), h(1, 5, "a")),
        op(
            7,
            T,
            11,
            "parent_id",
            json!(uid(10).to_string()),
            h(1, 6, "a"),
        ),
        op(8, T, 11, "location", json!({"kind": "loose"}), h(1, 7, "a")),
        op(9, T, 12, "title", json!("Enkel"), h(1, 8, "a")),
        op(
            10,
            T,
            12,
            "parent_id",
            json!(uid(11).to_string()),
            h(1, 9, "a"),
        ),
        op(11, T, 13, "title", json!("Kind 2"), h(1, 10, "a")),
        op(
            12,
            T,
            13,
            "parent_id",
            json!(uid(10).to_string()),
            h(1, 11, "a"),
        ),
        op(13, T, 13, "status", json!("done"), h(1, 12, "a")),
        op(14, T, 14, "title", json!("Kind 3"), h(1, 13, "a")),
        op(
            15,
            T,
            14,
            "parent_id",
            json!(uid(10).to_string()),
            h(1, 14, "a"),
        ),
        op(16, T, 14, "status", json!("dropped"), h(1, 15, "a")),
        op(
            17,
            T,
            15,
            "title",
            json!("Kind 4 (gelöscht)"),
            h(1, 16, "a"),
        ),
        op(
            18,
            T,
            15,
            "parent_id",
            json!(uid(10).to_string()),
            h(1, 17, "a"),
        ),
        op(19, T, 15, "deleted", json!(true), h(1, 18, "a")),
    ]
}

#[test]
fn effective_location_uses_root() {
    let s = apply_all(&tree_ops());
    let project = Location::Project { id: uid(2) };
    assert_eq!(s.effective_location(uid(12)), Some(project));
    assert_eq!(s.root_task(uid(12)), Some(uid(10)));
    assert_eq!(s.effective_project(uid(11)), Some(uid(2)));
    assert_eq!(s.task(uid(11)).unwrap().location, Location::Loose);
    assert_eq!(s.effective_location(uid(99)), None);
}

#[test]
fn cycles_and_dangling_parents() {
    let ops = [
        op(1, T, 10, "title", json!("a"), h(1, 0, "a")),
        op(
            2,
            T,
            10,
            "parent_id",
            json!(uid(11).to_string()),
            h(1, 1, "a"),
        ),
        op(3, T, 11, "title", json!("b"), h(1, 2, "a")),
        op(
            4,
            T,
            11,
            "parent_id",
            json!(uid(10).to_string()),
            h(1, 3, "a"),
        ),
        op(5, T, 12, "title", json!("c"), h(1, 4, "a")),
        op(
            6,
            T,
            12,
            "parent_id",
            json!(uid(10).to_string()),
            h(1, 5, "a"),
        ),
        op(7, T, 13, "title", json!("d"), h(1, 6, "a")),
        op(
            8,
            T,
            13,
            "parent_id",
            json!(uid(99).to_string()),
            h(1, 7, "a"),
        ),
        op(9, T, 13, "location", json!({"kind": "loose"}), h(1, 8, "a")),
    ];
    let s = apply_all(&ops);
    assert_eq!(s.effective_location(uid(10)), None);
    assert_eq!(s.effective_location(uid(12)), None);
    assert_eq!(s.effective_location(uid(13)), Some(Location::Loose));
    // descendants terminate despite the cycle
    let d = s.descendants(uid(10));
    assert!(d.contains(&uid(11)) && d.contains(&uid(12)));
    assert!(!d.contains(&uid(10)));
}

#[test]
fn children_progress_and_descendants() {
    let s = apply_all(&tree_ops());
    assert_eq!(s.children(uid(10)), vec![uid(11), uid(13), uid(14)]);
    assert_eq!(s.child_ids(uid(10)).count(), 4);
    assert_eq!(
        s.descendants(uid(10)),
        vec![uid(11), uid(12), uid(13), uid(14)]
    );
    assert_eq!(s.progress(uid(10)), Progress { done: 1, total: 2 });
    assert_eq!(s.open_subtask_count(uid(10)), 1);
    assert_eq!(s.progress(uid(12)), Progress::default());
}

#[test]
fn children_index_follows_lww_winner() {
    let mut ops = tree_ops();
    // An older re-parenting of 12 must not win over the existing parent.
    ops.push(op(
        30,
        T,
        12,
        "parent_id",
        json!(uid(13).to_string()),
        h(0, 0, "a"),
    ));
    // A newer one moves 11 to the top level.
    ops.push(op(31, T, 11, "parent_id", json!(null), h(5, 0, "a")));
    for order in [ops.clone(), ops.iter().rev().cloned().collect()] {
        let s = apply_all(&order);
        assert_eq!(s.children(uid(11)), vec![uid(12)]);
        assert!(s.children(uid(13)).is_empty());
        assert_eq!(s.children(uid(10)), vec![uid(13), uid(14)]);
        assert_eq!(s.effective_location(uid(12)), Some(Location::Loose));
    }
}

#[test]
fn default_visibility_hides_closed_after_three_days() {
    let s = apply_all(&tree_ops());
    let changed = s.status_changed_at(uid(13)).unwrap().wall_ms();
    assert!(s.is_visible_by_default(uid(13), changed));
    assert!(s.is_visible_by_default(uid(13), changed + CLOSED_VISIBLE_MS - 1));
    assert!(!s.is_visible_by_default(uid(13), changed + CLOSED_VISIBLE_MS));
    assert!(!s.is_visible_by_default(uid(14), changed + CLOSED_VISIBLE_MS));
    assert!(s.is_visible_by_default(uid(10), u64::MAX));
    assert!(!s.is_visible_by_default(uid(15), 0));
    assert!(!s.is_visible_by_default(uid(99), 0));
}

#[test]
fn clickup_lookup() {
    let mut ops = tree_ops();
    ops.push(op(40, T, 12, "clickup_id", json!("86abc"), h(9, 0, "a")));
    let s = apply_all(&ops);
    assert_eq!(s.task_by_clickup_id("86abc").unwrap().id, uid(12));
    assert!(s.task_by_clickup_id("nope").is_none());
}

#[test]
fn snapshot_roundtrip_and_shape() {
    let mut ops = tree_ops();
    ops.push(op(50, T, 10, "title", json!("Wurzel neu"), h(7, 0, "b")));
    let s = apply_all(&ops);
    let snapshot = s.to_snapshot();
    let json = serde_json::to_value(&snapshot).unwrap();
    let first_task = json
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["entity"] == "task" && e["entity_id"] == uid(10).to_string())
        .unwrap();
    assert_eq!(first_task["created_hlc"], json!("0000000000001:00003:a"));
    assert_eq!(first_task["fields"]["title"]["value"], json!("Wurzel neu"));
    assert_eq!(
        first_task["fields"]["title"]["hlc"],
        json!("0000000000007:00000:b")
    );
    assert_eq!(
        first_task["fields"]["title"]["op_id"],
        json!(uid(50).to_string())
    );

    let parsed: Vec<SnapshotEntity> = serde_json::from_value(json).unwrap();
    let restored = State::from_snapshot(&parsed);
    assert_eq!(restored, s);
    assert_eq!(restored.children(uid(10)), s.children(uid(10)));
    assert!(restored.knows_op(&uid(50)));
    // A losing op ID is not part of the snapshot.
    assert!(!restored.knows_op(&uid(4)));
}

#[test]
fn snapshot_merge_is_lww() {
    let ops = tree_ops();
    let newer = op(60, T, 10, "title", json!("neuer"), h(100, 0, "b"));
    let mut a = apply_all(&ops);
    a.apply(&newer);
    let b = apply_all(&ops);
    // Merging an older snapshot into a newer state changes nothing.
    let mut merged = a.clone();
    merged.merge_snapshot(&b.to_snapshot());
    assert_eq!(merged, a);
    // And the other way around brings the newer value.
    let mut merged = b.clone();
    merged.merge_snapshot(&a.to_snapshot());
    assert_eq!(merged, a);
}

#[test]
fn snapshot_without_created_hlc_uses_smallest_field() {
    let raw = json!([{
        "entity": "task",
        "entity_id": uid(1).to_string(),
        "fields": {
            "title": {"value": "x", "hlc": "0000000000005:00000:a", "op_id": uid(2).to_string()},
            "status": {"value": null, "hlc": "0000000000003:00000:a", "op_id": uid(3).to_string()}
        }
    }]);
    let parsed: Vec<SnapshotEntity> = serde_json::from_value(raw).unwrap();
    let s = State::from_snapshot(&parsed);
    assert_eq!(s.created_at(T, uid(1)), Some(&h(3, 0, "a")));
}

#[test]
fn json_total_order() {
    use std::cmp::Ordering::*;
    assert_eq!(cmp_json(&json!(null), &json!(false)), Less);
    assert_eq!(
        cmp_json(&json!({"b": 1, "a": 2}), &json!({"a": 2, "b": 1})),
        Equal
    );
    assert_eq!(cmp_json(&json!([1, 2]), &json!([1, 2, 3])), Less);
    assert_eq!(cmp_json(&json!("b"), &json!("a")), Greater);
}

// ----- properties -----------------------------------------------------------

fn arb_op() -> impl Strategy<Value = Op> {
    let field_value = prop_oneof![
        prop::sample::select(vec!["a", "b", "c"]).prop_map(|v| ("title", json!(v))),
        prop::sample::select(vec!["open", "done", "dropped"]).prop_map(|v| ("status", json!(v))),
        prop_oneof![
            Just(Value::Null),
            (20u128..23).prop_map(|n| json!(uid(n).to_string()))
        ]
        .prop_map(|v| ("parent_id", v)),
        prop_oneof![
            Just(json!({"kind": "inbox"})),
            Just(json!({"kind": "loose"})),
            Just(json!({"kind": "project", "id": uid(1).to_string()})),
        ]
        .prop_map(|v| ("location", v)),
        prop::bool::ANY.prop_map(|b| ("deleted", json!(b))),
    ];
    (
        0u128..12, // small op_id pool: also produces malformed duplicates
        prop_oneof![Just(EntityKind::Task), Just(EntityKind::Area)],
        20u128..23,
        field_value,
        0u64..4,
        0u32..3,
        prop::sample::select(vec!["a", "b"]),
    )
        .prop_map(
            |(op_id, kind, entity, (field, value), wall, counter, node)| {
                op(op_id, kind, entity, field, value, h(wall, counter, node))
            },
        )
}

/// Ops plus a shuffled copy with some ops duplicated.
fn arb_ops_and_permutation() -> impl Strategy<Value = (Vec<Op>, Vec<Op>)> {
    prop::collection::vec(arb_op(), 0..40).prop_flat_map(|ops| {
        let n = ops.len();
        let dupes = prop::collection::vec(0..n.max(1), 0..=n);
        (Just(ops), dupes).prop_flat_map(|(ops, dupes)| {
            let mut permuted = ops.clone();
            if !ops.is_empty() {
                permuted.extend(dupes.iter().map(|&i| ops[i].clone()));
            }
            (Just(ops), Just(permuted).prop_shuffle())
        })
    })
}

fn assert_index_consistent(s: &State) {
    for e in s.entities_of(T) {
        let expected: Vec<Id> = s
            .entities_of(T)
            .filter(|c| c.parent_id() == Some(e.id()))
            .map(|c| c.id())
            .collect();
        assert_eq!(s.child_ids(e.id()).collect::<Vec<_>>(), expected);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn any_permutation_with_duplicates_yields_same_state((ops, permuted) in arb_ops_and_permutation()) {
        let a = apply_all(&ops);
        let b = apply_all(&permuted);
        prop_assert_eq!(&a, &b);
        prop_assert_eq!(
            serde_json::to_value(a.to_snapshot()).unwrap(),
            serde_json::to_value(b.to_snapshot()).unwrap()
        );
        assert_index_consistent(&a);
        assert_index_consistent(&b);
        for e in a.entities_of(T) {
            prop_assert_eq!(a.children(e.id()), b.children(e.id()));
            prop_assert_eq!(a.effective_location(e.id()), b.effective_location(e.id()));
        }
    }

    #[test]
    fn merge_is_associative_via_snapshots(
        ops in prop::collection::vec(arb_op(), 0..40),
        split1 in 0usize..40,
        split2 in 0usize..40,
    ) {
        let (i, j) = (split1.min(split2).min(ops.len()), split1.max(split2).min(ops.len()));
        let (x, y, z) = (apply_all(&ops[..i]), apply_all(&ops[i..j]), apply_all(&ops[j..]));
        // (x ∪ y) ∪ z
        let mut left = x.clone();
        left.merge_snapshot(&y.to_snapshot());
        left.merge_snapshot(&z.to_snapshot());
        // x ∪ (y ∪ z)
        let mut yz = y.clone();
        yz.merge_snapshot(&z.to_snapshot());
        let mut right = x.clone();
        right.merge_snapshot(&yz.to_snapshot());
        let whole = apply_all(&ops);
        prop_assert_eq!(&left, &whole);
        prop_assert_eq!(&right, &whole);
        // idempotent
        let mut again = whole.clone();
        again.merge_snapshot(&whole.to_snapshot());
        prop_assert_eq!(&again, &whole);
        assert_index_consistent(&left);
    }
}
