# zunder-core

Der gemeinsame Kern von Zunder: Domänenmodell, Hybrid Logical Clock, Operationen, Last-Writer-Wins-Merge und Validierung. Server, Import und Tauri-App nutzen dieselbe Crate. Sie hat keine I/O-, Async- oder Datenbankabhängigkeiten; Zeit wird immer als Parameter übergeben, damit alles testbar bleibt.

Verbindlich ist der [Implementierungsvertrag](../../docs/superpowers/plans/2026-10-11-zunder-implementation.md), Abschnitt 2.

## Module

| Modul | Inhalt |
|---|---|
| `ids` | `new_id()` (UUIDv7), `deterministic_id()` / `clickup_entity_id()` (UUIDv5 im festen `ZUNDER_NAMESPACE`), strikte Serde-Hilfen für kanonische UUID-Strings |
| `hlc` | `Hlc` mit Format `<13 Stellen wall_ms>:<5 Stellen counter>:<node>`, `NodeId`, `Clock::now(physical_ms)` und `Clock::receive(remote)` |
| `op` | `Op` und `EntityKind` im Wire-Format |
| `model` | Feldnamen (`fields::task::TITLE` …), Enums (`TaskType`, `TaskStatus`, `Location`, `Phoenix` …), typisierte Sichten `Area`, `Project`, `Task`, Wertvalidierung `validate_field_value` |
| `state` | `State`: LWW-Register je (Entität, Feld), abgeleitete Daten (`created_at`, `updated_at`, `status_changed_at`, wirksame Stelle, Unteraufgaben, Fortschritt, Sichtbarkeit), Snapshot-Export und -Import |
| `validate` | `validate_push(state, ops, now_ms)` → `PushOutcome` mit `accepted`, `rejected`, `warnings` und den neu zu speichernden `new_ops` |
| `builder` | `OpBatch` zum Erzeugen von Operationen: `create_area/project/task`, `set_*`, `move_task`, `delete_cascade` … |
| `search` | Volltextsuche über Titel, Beschreibung, Plan und Namen; Groß-/Kleinschreibung egal, `ä` findet `a` und `ae` |

## Verwendung

```rust
use zunder_core::{validate_push, Clock, Location, NewTask, NodeId, OpBatch, State};
use zunder_core::hlc::system_time_ms;

let mut state = State::new();
let mut clock = Clock::new(NodeId::new("server").unwrap());

let now = system_time_ms();
let mut batch = OpBatch::new(&mut clock, now);
let task = batch.create_task(NewTask::new("Zahnarzt anrufen").at(Location::Inbox));
let ops = batch.into_ops();

let outcome = validate_push(&state, &ops, now);
// erst outcome.new_ops speichern, dann anwenden
for op in &outcome.new_ops {
    state.apply(op);
}
assert_eq!(state.task(task).unwrap().title, "Zahnarzt anrufen");
```

Hinweise für Aufrufer:

- `validate_push` verändert den Zustand nicht. Bereits bekannte `op_id`s (laut `State::knows_op`) gelten als angenommen, landen aber nicht in `new_ops`. Wer den Zustand aus einem Snapshot lädt, kennt nur die gewinnenden `op_id`s und muss Duplikate zusätzlich über das eigene Operations-Log erkennen (oder `State::mark_op_known` nutzen).
- Die eigene Uhr sollte jede angenommene fremde Operation per `Clock::receive` sehen, damit eigene Änderungen danach einen größeren HLC tragen.
- Gelöschte Entitäten bleiben im Zustand (`deleted: true`). Die typisierten Listen `live_tasks()` usw. blenden sie aus; `is_visible_by_default` blendet zusätzlich erledigte und verworfene Aufgaben drei Tage nach `status_changed_at` aus.
- Löschen ist nur erlaubt, wenn keine lebende Entität mehr auf das Ziel verweist. `OpBatch::delete_cascade` löscht die Abhängigen im selben Push mit.

## Tests

```sh
cargo test -p zunder-core
```

- Unit-Tests je Modul, Validierungstests für jede Regel.
- Property-Tests (proptest): beliebige Permutationen und Duplikate derselben Operationen ergeben denselben Zustand; Merge über Snapshots ist assoziativ und idempotent; Stringordnung der HLCs entspricht der Tupelordnung; die Uhr ist streng monoton; nach beliebigen validierten Pushes verletzt der Zustand keine Invariante.
- `tests/vectors/*.json`: gemeinsame Testvektoren für Rust und TypeScript (Vertrag 2.6). `kind: "merge"` enthält `ops` und `expected` (gewinnende Werte je `"<entity>/<entity_id>"`, `null` bleibt `null`), `kind: "hlc_order"` enthält `sorted`. Das optionale Feld `description` erklärt den Fall. `tests/vectors.rs` wendet jede Datei in vielen Reihenfolgen an.
