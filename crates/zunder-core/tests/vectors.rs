//! Runs the shared test vectors in `tests/vectors/*.json` (contract section
//! 2.6). The same files are consumed by the TypeScript client tests.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Map, Value};
use zunder_core::{validate_push, Hlc, Op, State};

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Vector {
    Merge {
        name: String,
        ops: Vec<Op>,
        expected: BTreeMap<String, Map<String, Value>>,
    },
    HlcOrder {
        name: String,
        sorted: Vec<String>,
    },
    /// `state` is merged first, then `push` is validated at `now_ms`.
    /// `expected_rejected` maps each rejected entity ID to the code of its
    /// cause (the first rejection that is not `entity_rejected`); every
    /// operation of such an entity must be rejected, all others accepted.
    Validate {
        name: String,
        now_ms: u64,
        state: Vec<Op>,
        push: Vec<Op>,
        expected_rejected: BTreeMap<String, String>,
    },
}

fn vector_files() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors");
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    files
}

fn load(path: &Path) -> Vector {
    let raw = fs::read_to_string(path).unwrap();
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Winning values per entity in the `expected` shape of the vectors.
fn materialize(state: &State) -> BTreeMap<String, Map<String, Value>> {
    state
        .entities()
        .map(|e| {
            let fields = e
                .fields()
                .iter()
                .map(|(name, entry)| (name.clone(), entry.value.clone()))
                .collect();
            (format!("{}/{}", e.kind(), e.id()), fields)
        })
        .collect()
}

/// Deterministic pseudo-random permutations (no external RNG needed).
fn shuffled(ops: &[Op], seed: u64) -> Vec<Op> {
    let mut out = ops.to_vec();
    let mut x = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    for i in (1..out.len()).rev() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let j = (x % (i as u64 + 1)) as usize;
        out.swap(i, j);
    }
    out
}

fn permutations(ops: &[Op]) -> Vec<(String, Vec<Op>)> {
    let mut perms = vec![
        ("as listed".to_owned(), ops.to_vec()),
        ("reversed".to_owned(), ops.iter().rev().cloned().collect()),
    ];
    let mut by_hlc = ops.to_vec();
    by_hlc.sort_by(|a, b| a.hlc.cmp(&b.hlc).then(a.op_id.cmp(&b.op_id)));
    perms.push(("ascending hlc".to_owned(), by_hlc.clone()));
    by_hlc.reverse();
    perms.push(("descending hlc".to_owned(), by_hlc));
    for k in 1..ops.len() {
        let mut rotated = ops.to_vec();
        rotated.rotate_left(k);
        perms.push((format!("rotated by {k}"), rotated));
    }
    for seed in 0..25 {
        perms.push((format!("shuffle seed {seed}"), shuffled(ops, seed)));
    }
    let mut doubled = ops.to_vec();
    doubled.extend(shuffled(ops, 99));
    perms.push(("all ops twice".to_owned(), doubled));
    perms
}

#[test]
fn shared_vectors() {
    let files = vector_files();
    let mut merge_count = 0;
    let mut hlc_count = 0;
    let mut validate_count = 0;
    for path in &files {
        let file_name = path.file_name().unwrap().to_string_lossy().into_owned();
        match load(path) {
            Vector::Merge {
                name,
                ops,
                expected,
            } => {
                merge_count += 1;
                assert!(
                    file_name.contains(&name),
                    "{file_name}: name {name:?} does not match"
                );
                assert!(!ops.is_empty(), "{name}: no ops");
                for (label, perm) in permutations(&ops) {
                    let state = State::from_ops(&perm);
                    assert_eq!(
                        materialize(&state),
                        expected,
                        "vector {name}, order: {label}"
                    );
                }
                // Snapshot round trip yields the same winners.
                let state = State::from_ops(&ops);
                let restored = State::from_snapshot(&state.to_snapshot());
                assert_eq!(materialize(&restored), expected, "vector {name}: snapshot");
            }
            Vector::Validate {
                name,
                now_ms,
                state,
                push,
                expected_rejected,
            } => {
                validate_count += 1;
                assert!(
                    file_name.contains(&name),
                    "{file_name}: name {name:?} does not match"
                );
                let state = State::from_ops(&state);
                let outcome = validate_push(&state, &push, now_ms);
                let mut causes: BTreeMap<String, String> = BTreeMap::new();
                for r in &outcome.rejected {
                    let op = push.iter().find(|o| o.op_id == r.op_id).unwrap();
                    let code = r.code.as_str();
                    if code != "entity_rejected" {
                        causes
                            .entry(op.entity_id.to_string())
                            .or_insert_with(|| code.to_owned());
                    }
                }
                assert_eq!(causes, expected_rejected, "vector {name}: {outcome:?}");
                for op in &push {
                    let rejected = outcome.rejected.iter().any(|r| r.op_id == op.op_id);
                    assert_eq!(
                        rejected,
                        expected_rejected.contains_key(&op.entity_id.to_string()),
                        "vector {name}: op {} ({})",
                        op.op_id,
                        op.field
                    );
                }
            }
            Vector::HlcOrder { name, sorted } => {
                hlc_count += 1;
                assert!(
                    file_name.contains(&name),
                    "{file_name}: name {name:?} does not match"
                );
                let parsed: Vec<Hlc> = sorted
                    .iter()
                    .map(|s| Hlc::parse(s).unwrap_or_else(|e| panic!("{name}: {e}")))
                    .collect();
                for (s, h) in sorted.iter().zip(&parsed) {
                    assert_eq!(&h.to_string(), s, "{name}: format round trip");
                }
                for i in 0..parsed.len() {
                    for j in i + 1..parsed.len() {
                        assert!(
                            parsed[i] < parsed[j],
                            "{name}: {} !< {}",
                            sorted[i],
                            sorted[j]
                        );
                        assert!(sorted[i] < sorted[j], "{name}: string order");
                    }
                }
            }
        }
    }
    assert!(
        merge_count >= 10,
        "expected at least 10 merge vectors, found {merge_count}"
    );
    assert!(hlc_count >= 1, "expected at least one hlc_order vector");
    assert!(
        validate_count >= 10,
        "expected at least 10 validate vectors, found {validate_count}"
    );
}
