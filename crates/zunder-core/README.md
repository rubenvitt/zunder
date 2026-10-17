# zunder-core

Der gemeinsame Kern von Zunder: Domänenmodell, Hybrid Logical Clock, Operationen, Last-Writer-Wins-Merge und Validierung. Server, Import und Tauri-App nutzen dieselbe Crate. Sie hat keine I/O-, Async- oder Datenbankabhängigkeiten; Zeit wird immer als Parameter übergeben, damit alles testbar bleibt.

Verbindlich ist der [Implementierungsvertrag](../../docs/superpowers/plans/2026-10-11-zunder-implementation.md), Abschnitt 2.

## Module

| Modul | Inhalt |
|---|---|
| `ids` | `Id` (CUID2, 24 Zeichen, `Copy`), `new_id()`, `deterministic_id()` / `clickup_entity_id()` (über UUIDv5 im festen `ZUNDER_NAMESPACE`), `Id::from_legacy` für alte UUIDs (beim Einlesen automatisch) |
| `hlc` | `Hlc` mit Format `<13 Stellen wall_ms>:<5 Stellen counter>:<node>`, `NodeId`, `Clock::now(physical_ms)` und `Clock::receive(remote)` |
| `op` | `Op` und `EntityKind` im Wire-Format, `upgrade_legacy_value` (alte UUIDs in ID-Feldern) |
| `model` | Feldnamen (`fields::task::TITLE` …), Enums und Werte (`TaskType`, `TaskStatus`, `Location`, `Phoenix`, `Recurrence` …), typisierte Sichten `Area`, `Project`, `Task`, Wertvalidierung `validate_field_value` |
| `state` | `State`: LWW-Register je (Entität, Feld), abgeleitete Daten (`created_at`, `updated_at`, `status_changed_at`, wirksame Stelle, Unteraufgaben, Fortschritt, Sichtbarkeit), Snapshot-Export und -Import |
| `validate` | `validate_push(state, ops, now_ms)` → `PushOutcome` mit `accepted`, `rejected`, `warnings` und den neu zu speichernden `new_ops`; `validate_push_as(…, Author::Server)` für Felder, die nur der Server schreibt (`ref`, `recurrence_prev`) |
| `numbering` | Projektschlüssel und Aufgabenkennungen (`ZUN-42`): `pending_refs`, `parse_ref`, `normalize_ref`, `State::task_by_ref` |
| `recurrence` | Termin der Folgeinstanz einer Serie (Vertrag 3.1): `next_occurrence` (neue `due`, `remind_at` und der zu schreibende Rhythmus), `next_due_date` auf lokalen Daten, `series_recurrence` (ergänzt `day` bei monatlichen Serien); die Zeitzone kommt als `LocalZone` (Offset zu einem Zeitpunkt, z. B. `FixedZone` oder eine Closure), `resolve_local` wendet die Regeln für Lücke und Doppel der Zeitumstellung an; Auslöser: `CompletionsBefore::capture` vor dem Anwenden eines Pushs, `follow_ups` danach liefert die anzulegenden Folgeinstanzen (`FollowUp::write` schreibt sie in einen `OpBatch`) |
| `balance` | Wochenbilanz (Vertrag 4.1, `week_balance`, und 5.1, Wochenspiegel): `week_balance(state, week, zone)` zählt erledigte, lebende Aufgaben außerhalb von Dev-Projekten nach dem lokalen Kalendertag von `status_changed_at`, je Gewicht und Für wen (`none` = ohne Wert) |
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
- Property-Tests (proptest): die Folgeinstanz bei `from: "due"` ist der früheste Termin der Serie nach der alten Fälligkeit, der nicht vor heute liegt (gegen ein unabhängiges Orakel); monatliche und jährliche Serien wandern nicht; `from: "done"` hängt nur vom Erledigungsdatum ab; Ortszeit und Abstand der Erinnerung bleiben über die Zeitumstellung; beliebige Permutationen und Duplikate derselben Operationen ergeben denselben Zustand; Merge über Snapshots ist assoziativ und idempotent; Stringordnung der HLCs entspricht der Tupelordnung; die Uhr ist streng monoton; nach beliebigen validierten Pushes verletzt der Zustand keine Invariante.
- `tests/vectors/*.json`: gemeinsame Testvektoren für Rust und TypeScript (Vertrag 2.6). `kind: "merge"` enthält `ops` und `expected` (gewinnende Werte je `"<entity>/<entity_id>"`, `null` bleibt `null`), `kind: "hlc_order"` enthält `sorted`, `kind: "validate"` enthält `state`, `push`, `now_ms` und `expected_rejected` (je abgelehnter Entität der Code der Ursache) und prüft die clientseitige Validierung der Web-UI gegen `validate_push`. `kind: "week_balance"` enthält `time_zone` (Europe/Berlin), `week`, `state` und `expected` (Form von `balance::WeekBalance`) und prüft die Wochenbilanz der Web-UI gegen `balance::week_balance`. Das optionale Feld `description` erklärt den Fall. `tests/vectors.rs` wendet jede Merge-Datei in vielen Reihenfolgen an.
