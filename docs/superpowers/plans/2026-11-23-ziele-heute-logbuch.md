# Ziele, Heute als Reihenfolge, Erledigt-Logbuch – Implementierungsplan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Zunder bekommt Ziele als synchronisierte Entität, Heute als Tagesreihenfolge mit Fokus, eine Ziele-Ansicht mit Wochen-Input und ein Logbuch „Erledigt“, alles über MCP für Claude pflegbar (ZUN-58).

**Architecture:** Weg 1 des Entwurfs. Neue Entität `goal` und neue Aufgabenfelder (`goal_id`, `planned_on`, `day_rank`, `flame_log`) laufen durch dieselbe Op-/LWW-Pipeline wie `comment`. Die drei Regeln (Heute-Liste, Ziel-Input, Logbuch) stehen einmal in `zunder-core` (neues Modul `planning`) und gespiegelt in `web/src/sync/planning.ts`; gemeinsame Testvektoren halten beide gleich (Muster `week_balance`). Nur der Server schreibt `flame_log` (Flammen-Dienst), einmalig aus dem Op-Log nachgefüllt.

**Tech Stack:** Rust (axum, SQLite, `zunder-core`), SolidJS/TypeScript (Vitest, Playwright), MCP im Server.

**Spec:** `docs/superpowers/specs/2026-11-23-ziele-heute-logbuch-design.md` (freigegeben 23.11.2026). Ausführende lesen Plan und Entwurf.

## Global Constraints

- Qualitätsschranken vor jedem Commit (`docs/entwicklung.md`): `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `(cd web && npm run typecheck && npm test && npm run build)`; vor dem PR-Abschluss zusätzlich `cargo build -p zunder-server && (cd web && npx playwright test && ZUNDER_E2E_BACKEND=real npx playwright test)`.
- Conventional Commits auf Deutsch, Bereiche `core`, `server`, `web`, `app`; Kennung `ZUN-58` im Commit-Body; Attribution-Zeilen der Sitzung.
- Oberfläche und Fehlermeldungen auf Deutsch, Begriffe aus dem Entwurf: „Fokus“, „Danach“, „Plan für heute erledigt.“, „Ziele“, „Erledigt“, „Daran gearbeitet“, „Für heute einplanen“, „Aus Heute nehmen“, „Ältere zeigen“.
- Gegen-Ziele: keine roten Anzeigen, keine Balken/Quoten/Streaks, keine Uhrzeiten in der Heute-Reihenfolge. `tone-danger` taucht in Heute nicht mehr auf.
- Tempo-Budget < 100 ms je Interaktion; Ziel-Input und Logbuch werden aus dem lokalen Zustand berechnet (keine Server-Anfrage in der Web-UI).
- „Heute“/Woche/Tag = Kalendertag in Ortszeit des Aufrufers (Web: Browser; Server/MCP: `ServerZone`, wie `week_balance`). Woche = ISO-Woche Mo–So.
- Vektoren modellieren nur `Europe/Berlin` (wie bisher).

## Review Focus

1. **Flamme über Mitternacht / Wochenwechsel:** Eine Flamme zählt ganz für den Tag ihres Starts (Ziel-Input und Logbuch), auch wenn sie nach 0:00 endet. Test in Task 4 (Vektor `goal_input`, Flamme So 23:30–Mo 0:30 zählt in der alten Woche).
2. **Zeitumstellung:** Logbuch-Tage und Wochen über die Sommer-/Winterzeitgrenze. Test in Task 4 (Vektor `done_log` am 25.10.2026 bzw. 29.03.2026).
3. **Ziel gelöscht oder geschlossen, Aufgaben behalten `goal_id`:** gelöschtes Ziel = kein Ziel (keine Zeile, kein Zeichen); geschlossenes Ziel zählt im Logbuch weiter mit Zeichen, erscheint aber nicht in der Ziele-Ansicht. Tests in Task 4 (Vektor) und Task 9 (Vitest).
4. **Wiederkehrende Aufgabe erledigt:** Die Folgeinstanz übernimmt `goal_id`, aber weder `planned_on`/`day_rank` noch `flame_log`. Test in Task 2 (`recurrence/follow/tests.rs`).
5. **`plan_day` mit unbekannter, gelöschter oder geschlossener Aufgabe in `task_ids`:** Der ganze Aufruf wird mit deutscher Meldung abgelehnt, nichts wird geschrieben. Test in Task 6.

---

### Task 1: Entität `goal` im Kern

**Files:**
- Modify: `crates/zunder-core/src/op.rs` (`EntityKind::Goal`, `"goal"`, Label „Ziel“, `ALL` auf 5)
- Modify: `crates/zunder-core/src/model.rs` (`fields::goal`, `GoalState`, `Goal`, Shapes `Date`, `PositiveCount`)
- Modify: `crates/zunder-core/src/state.rs` (`Entity::to_goal`, `State::goal`, `State::live_goals`)
- Modify: `crates/zunder-core/src/validate.rs` (Artikel/Labels für Goal in den Meldungs-Tabellen; keine Referenzen)
- Modify: `crates/zunder-core/src/builder.rs` (`set_goal`)
- Modify: `crates/zunder-core/src/search.rs` (Goal → `None`, wie Comment)
- Modify: `crates/zunder-core/src/lib.rs` (Re-Export `Goal`, `GoalState`)
- Create: `crates/zunder-core/tests/vectors/63-validierung-ziele.json`
- Test: `crates/zunder-core/src/model.rs` (mod tests), `crates/zunder-core/tests/vectors.rs`

**Interfaces:**
- Produces:
  - `fields::goal::{NAME, EMOJI, LEAD, LEAD_PER_WEEK, STORM, STARTS_ON, ENDS_ON, STATE, RANK, DELETED, ALL, REQUIRED = &[NAME]}` mit den Wire-Namen `name, emoji, lead, lead_per_week, storm, starts_on, ends_on, state, rank, deleted`.
  - `enum GoalState { Active, Closed }` (Codes `active`, `closed`, Default `Active`) über das bestehende Enum-Makro.
  - `pub struct Goal { id: Id, name: String, emoji: Option<String>, lead: Option<String>, lead_per_week: Option<u32>, storm: Option<String>, starts_on: Option<Date>, ends_on: Option<Date>, state: GoalState, rank: Option<String>, deleted: bool }` (Serialize: Daten als `JJJJ-MM-TT`).
  - `State::goal(&self, id: Id) -> Option<Goal>`, `State::live_goals(&self) -> impl Iterator<Item = Goal>`.
  - `Shape::Date` (strikt `JJJJ-MM-TT`, echter Kalendertag; `parse_plain_date`), `Shape::PositiveCount` (ganze Zahl 1–99).
  - `OpBuilder::set_goal(&mut self, id: Id, field: &str, value: impl Into<Value>)`.

- [ ] **Step 1: Failing tests schreiben**
  - `model.rs` mod tests: `fn goal_fields()` mit `ok(Goal, "name", "Sport")`, `bad(Goal, "name", "")`, `ok(Goal, "emoji", "🏃")`, `ok(Goal, "lead_per_week", 3)`, `bad(Goal, "lead_per_week", 0)`, `bad(Goal, "lead_per_week", 100)`, `ok(Goal, "starts_on", "2026-09-28")`, `bad(Goal, "starts_on", "2026-02-30")`, `bad(Goal, "starts_on", "2026-09-28T00:00:00Z")`, `ok(Goal, "state", "closed")`, `bad(Goal, "state", "archived")`, `bad(Goal, "unknown", "x")` → `unknown_field`.
  - Vektor 63 (`kind: validate`): Ziel ohne `name` → `missing_required`; gültiges Ziel mit allen Feldern → angenommen; `lead_per_week: 0` → `invalid_value`.
- [ ] **Step 2:** `cargo test -p zunder-core goal` und `cargo test -p zunder-core --test vectors` → FAIL (unbekannte Entität).
- [ ] **Step 3:** Implementieren wie oben; `upgrade_legacy_value` braucht für Goal nichts.
- [ ] **Step 4:** `cargo test -p zunder-core` → PASS. `cargo clippy -p zunder-core --all-targets -- -D warnings` → sauber.
- [ ] **Step 5: Commit** `feat(core): Ziele als eigene Entität`

### Task 2: Neue Aufgabenfelder im Kern

**Files:**
- Modify: `crates/zunder-core/src/model.rs` (`fields::task::{GOAL_ID, PLANNED_ON, DAY_RANK, FLAME_LOG}`, `Task` um `goal_id: Option<Id>`, `planned_on: Option<Date>`, `day_rank: Option<String>`, `flame_log: Vec<FlameInterval>`, `Shape::FlameLog`)
- Modify: `crates/zunder-core/src/state.rs` (Materialisierung)
- Modify: `crates/zunder-core/src/validate.rs` (`goal_id` → `broken_reference` auf lebendes Ziel, nur wenn geändert; `flame_log` für `Author::Client` reserviert)
- Modify: `crates/zunder-core/src/op.rs` (`upgrade_legacy_value`: `goal_id` wie andere ID-Felder)
- Modify: `crates/zunder-core/src/recurrence/follow.rs` (Folgeinstanz: `goal_id` übernehmen; `planned_on`, `day_rank`, `flame_log` nicht)
- Create: `crates/zunder-core/tests/vectors/64-validierung-aufgabe-ziel-und-plan.json`
- Test: `model.rs` tests, `validate/tests.rs`, `recurrence/follow/tests.rs`

**Interfaces:**
- Consumes: Task 1 (`EntityKind::Goal`, `Shape::Date`).
- Produces:
  - `pub struct FlameInterval { pub start: OffsetDateTime, pub end: OffsetDateTime }` (Serde: RFC 3339, `deny_unknown_fields`); Shape `FlameLog` = Liste, je `end > start`, höchstens 1000 Einträge.
  - Task-Felder wie oben; `day_rank` Shape `Text` (wie `rank`).

- [ ] **Step 1: Failing tests**
  - `model.rs`: `fn planning_fields()`: `ok(Task,"planned_on","2026-11-23")`, `bad(Task,"planned_on","23.11.2026")`, `ok(Task,"day_rank","a0")`, `ok(Task,"flame_log",[{"start":"2026-11-23T08:00:00Z","end":"2026-11-23T08:15:00Z"}])`, `bad(Task,"flame_log",[{"start":"2026-11-23T08:15:00Z","end":"2026-11-23T08:00:00Z"}])`, `ok(Task,"goal_id",ID)`, `bad(Task,"goal_id","ZUN-1")`.
  - Vektor 64 (`validate`): Client-Push `flame_log` → `invalid_value`; `goal_id` auf nicht existierendes Ziel → `broken_reference`; `goal_id` auf lebendes Ziel → angenommen; Ziel löschen, während Aufgaben darauf zeigen → angenommen (kein Rückverweis-Schutz).
  - `follow/tests.rs`: `fn follow_up_keeps_goal_but_not_plan()` – Instanz mit `goal_id`, `planned_on`, `day_rank`, `flame_log` erledigen → Folgeinstanz hat `goal_id`, die anderen drei fehlen.
- [ ] **Step 2:** `cargo test -p zunder-core` → FAIL.
- [ ] **Step 3:** Implementieren (Server-Schreibrecht für `flame_log` über `Author::Server`, analog `recurrence_prev`; Meldung: „Die Flammen-Historie („flame_log“) schreibt nur der Server, wenn eine Flamme endet.“).
- [ ] **Step 4:** `cargo test -p zunder-core` → PASS.
- [ ] **Step 5: Commit** `feat(core): Aufgaben mit Ziel, Tagesplan und Flammen-Historie`

### Task 3: Server speichert Ziele (Migration) und App zieht nach

**Files:**
- Create: `crates/zunder-server/migrations/0010_goals.sql` (`ops` und `entities` neu mit `CHECK (entity IN ('area','project','task','comment','goal'))`, exakt nach Muster `0006_comments.sql`, `seq` und `sqlite_sequence` erhalten)
- Modify: `crates/zunder-server/src/db.rs` (`MIGRATIONS` um 0010)
- Modify: `crates/zunder-server/src/views.rs` (`GoalView` mit allen Feldern + `id`; `TaskView` um `goal_id`, `goal: Option<GoalRef { id, name, emoji }>` (effektives Ziel, Task 4), `planned_on`, `day_rank`, `flame_log`)
- Modify: `apps/zunder-app/src-tauri/zunder-local/…` nur falls der Build durch die neue Entität bricht (zentrale `match`-Arme)
- Test: `crates/zunder-server/tests/api.rs` (`goal_roundtrip_via_sync`), Migrationstest im bestehenden Stil (`db.rs`-Tests: Schema-Version 10, alte `seq` bleiben)

**Interfaces:**
- Consumes: Task 1, 2.
- Produces: `GoalView` (JSON wie `Goal`), `TaskView` erweitert; `SCHEMA_VERSION == 10`.

- [ ] **Step 1: Failing tests:** `goal_roundtrip_via_sync` – Push eines Ziels über `/api/sync/push`, Snapshot enthält Entität `goal` mit `name`; Migrationstest: DB auf Version 9 mit Ops anlegen, migrieren, `seq` unverändert, neuer Push vergibt höhere `seq`.
- [ ] **Step 2:** `cargo test -p zunder-server goal_roundtrip` → FAIL (CHECK-Constraint).
- [ ] **Step 3:** Migration und Views implementieren.
- [ ] **Step 4:** `cargo test -p zunder-server` → PASS; `cd apps/zunder-app/src-tauri && cargo build -p zunder-local && cargo test -p zunder-local` → PASS.
- [ ] **Step 5: App-Verträglichkeit festhalten.** Entscheidung (Entwurf 9, dem Builder überlassen): **kein Kompatibilitätsweg**. Ältere App-Stände bekommen beim Pull einer `goal`-Op einen Protokollfehler, verlieren aber nichts (die Ops bleiben auf dem Server); die Desktop-App aktualisiert sich selbst, iOS wird lokal neu gebaut. Das kommt als Versionshinweis in den PR und in den Kommentar „PR offen“.
- [ ] **Step 6: Commit** `feat(server): Ziele speichern und in Ansichten zeigen`

### Task 4: Regeln Heute, Ziel-Input und Logbuch im Kern

**Files:**
- Create: `crates/zunder-core/src/planning.rs` (+ `mod tests`)
- Modify: `crates/zunder-core/src/lib.rs` (`pub mod planning`)
- Create: Vektoren `65-heute-liste.json`, `66-ziel-input-woche.json`, `67-logbuch-zeitumstellung.json`
- Modify: `crates/zunder-core/tests/vectors.rs` (Arme `TodayList`, `GoalInput`, `DoneLog`)

**Interfaces:**
- Consumes: Task 1, 2; `balance::counts`-Muster, `State::effective_location`, `State::is_dev_location`, `State::status_changed_at`, `recurrence::{to_local, LocalZone}`.
- Produces (alle `Serialize`, Feldnamen = JSON der Vektoren):
  - `fn effective_goal(state: &State, task_id: Id) -> Option<Id>` – eigenes `goal_id`, sonst das der Wurzel (Kette über `parent_id`); ein gelöschtes Ziel zählt als `None`.
  - `struct TodayList { items: Vec<Id>, focus: Option<Id>, has_plan: bool, plan_done: bool }`
  - `fn today_list(state: &State, today: Date, zone: &impl LocalZone) -> TodayList` – Regel Entwurf 5.1/5.2:
    - Kandidaten: lebend, Status `open|in_progress|blocked|review`.
    - Gruppe 1 `planned_on == today` nach `day_rank` (fehlend zuletzt), dann Erstellungs-HLC, dann ID.
    - Gruppe 2 `planned_on < today` nach `planned_on`, `day_rank`, Erstellung, ID.
    - Gruppe 3 `planned_on == None && due <= today` nach `weight_rank` (brand, glut, ohne, rauch, asche), dann Fälligkeitszeitpunkt (Datum ohne Uhrzeit = lokale Mitternacht), dann Erstellung, ID – gleiche Reihenfolge wie `byDue` in `web/src/state/selectors.ts`.
    - `has_plan`: irgendeine lebende Aufgabe (jeder Status) mit `planned_on == today`. `plan_done`: `has_plan` und keine davon in `open|in_progress|blocked|review`.
    - `focus`: laufende Flamme (lebend, `flame_started_at` gesetzt und `flame_ended_at` leer oder älter); sonst, wenn nicht `plan_done`, die erste Aufgabe aus `items` mit `open|in_progress` und ohne `waiting_on`; sonst `None`.
  - `struct GoalInput { goal_id: Id, done: u32, minutes: u32, lead_per_week: Option<u32>, reached: bool }`, `struct WeekGoalInput { week: IsoWeek, from: Date, to: Date, goals: Vec<GoalInput> }`
  - `fn goal_input(state: &State, week: IsoWeek, zone: &impl LocalZone, now: OffsetDateTime) -> WeekGoalInput` – nur `state == active`, sortiert nach `rank`, dann Name. `done`: lebend, `done`, Woche von `status_changed_at`, effektives Ziel (Dev-Projekte **nicht** ausgeschlossen). `minutes`: Summe ganzer Minuten (abgerundet je Intervall) aus `flame_log` plus laufende Flamme bis `now`, Tag des Starts in `zone`. `reached = lead_per_week.is_some_and(|n| done >= n)`.
  - `struct LogEntry { task_id: Id, minutes: u32 }`, `struct LogDay { date: Date, minutes: u32, done: Vec<LogEntry>, worked: Vec<LogEntry> }`
  - `fn done_log(state: &State, from: Date, to: Date, zone: &impl LocalZone, now: OffsetDateTime) -> Vec<LogDay>` – Tage `to` bis `from` absteigend, nur Tage mit Einträgen. `done`: lebend, `done`, Tag von `status_changed_at`, nicht in Dev-Projekt, nach `status_changed_at` absteigend; `minutes` = Flammen-Minuten dieser Aufgabe an diesem Tag. `worked`: Aufgaben mit Flammen-Minuten > 0 an diesem Tag, die nicht in `done` dieses Tages stehen, nicht in Dev-Projekt, nicht gelöscht, Status egal außer `dropped`; nach Minuten absteigend, dann ID. `minutes` des Tages = Summe beider Listen.

- [ ] **Step 1: Failing tests** (Vektoren, je alle Op-Permutationen wie `week_balance`):
  - 65 `today_list`, `today: "2026-11-23"`: geplant heute (zwei, `day_rank` b vor a getauscht), liegengeblieben vom 21.11., fällig 22.11. ohne Plan, fällig heute Glut vs. Rauch, erledigt heute geplant, `waiting_on` als erste geplante → `focus` ist die zweite; zweiter Fall „alles Geplante erledigt“ → `plan_done: true`, `focus: null`; dritter Fall laufende Flamme auf einer ungeplanten Aufgabe → `focus` = sie.
  - 66 `goal_input`, `week: "2026-W48"`: Ziel aktiv mit `lead_per_week: 1` und eine Erledigung → `reached: true`; Unteraufgabe ohne Ziel unter Wurzel mit Ziel zählt; Flamme So 29.11. 23:30–Mo 30.11. 00:30 MEZ zählt 60 min in W48; geschlossenes Ziel fehlt; gelöschtes Ziel fehlt; Dev-Aufgabe mit Ziel zählt.
  - 67 `done_log`, `from: "2026-10-24"`, `to: "2026-10-26"` (Winterzeit-Umstellung 25.10.): Erledigung So 25.10. 23:30 MEZ gehört zum 25.; „Daran gearbeitet“ mit offener Aufgabe; verworfene fehlt; Dev-Aufgabe fehlt; Aufgabe mit geschlossenem Ziel erscheint.
- [ ] **Step 2:** `cargo test -p zunder-core --test vectors` → FAIL.
- [ ] **Step 3:** `planning.rs` implementieren.
- [ ] **Step 4:** `cargo test -p zunder-core` → PASS.
- [ ] **Step 5: Commit** `feat(core): Regeln für Heute, Ziel-Input und Logbuch`

### Task 5: Flammen-Historie im Server

**Files:**
- Modify: `crates/zunder-server/src/flame.rs` (`stop` und das implizite Beenden in `start` hängen `FlameInterval { start: flame_started_at, end: now }` an `flame_log` an, im selben `mutate_atomic`)
- Create: `crates/zunder-server/src/flame_backfill.rs` (einmalige Nachfüllung)
- Modify: `crates/zunder-server/src/lib.rs` / `main.rs` (Nachfüllung beim Start nach den Migrationen)
- Test: `crates/zunder-server/tests/flame.rs`

**Interfaces:**
- Consumes: Task 2 (`fields::task::FLAME_LOG`, `FlameInterval`).
- Produces: `pub async fn backfill_flame_log(app: &App) -> AppResult<usize>` (Zahl der beschriebenen Aufgaben). Idempotent über die Einstellungszeile `backfill.flame_log` = `"1"`. Sie liest je Aufgabe aus dem Op-Log alle `flame_started_at`/`flame_ended_at`-Ops nach `seq`, paart jeden Start mit dem nächsten Ende danach, verwirft Paare ohne Ende und schreibt `flame_log` mit Quelle `Source::Server` nur bei Aufgaben, die noch keinen `flame_log` haben.

- [ ] **Step 1: Failing tests:** `stop_appends_to_flame_log` (zwei Flammen nacheinander auf derselben Aufgabe → zwei Einträge); `start_elsewhere_logs_previous_flame` (Start auf B beendet A → A hat einen Eintrag); `backfill_reconstructs_old_flames` (Ops wie vor der Änderung direkt schreiben: Start, Ende, Start, Ende, Start ohne Ende → zwei Einträge; zweiter Aufruf → 0); `client_cannot_write_flame_log` (Push → `invalid_value`).
- [ ] **Step 2:** `cargo test -p zunder-server --test flame` → FAIL.
- [ ] **Step 3:** Implementieren.
- [ ] **Step 4:** `cargo test -p zunder-server` → PASS.
- [ ] **Step 5: Commit** `feat(server): Flammen-Historie je Aufgabe, alte Flammen nachgefüllt`

### Task 6: MCP-Werkzeuge

**Files:**
- Modify: `crates/zunder-server/src/mcp/schema.rs`, `crates/zunder-server/src/mcp/tools.rs`, `crates/zunder-server/src/mcp/mod.rs` (Server-Anweisungen), `crates/zunder-server/src/mcp/args.rs` (Datum-Argument falls nötig)
- Modify: `crates/zunder-server/tests/snapshots/mcp_tools.json`
- Test: `crates/zunder-server/tests/mcp.rs`

**Interfaces:**
- Consumes: Task 3 (`GoalView`, `TaskView`), Task 4 (`today_list`, `goal_input`, `done_log`, `effective_goal`).
- Produces (Werkzeuge, Argumente, Rückgabe):
  - `list_goals { include_closed?: bool }` → `{ goals: [GoalView] }`
  - `create_goal { name, emoji?, lead?, lead_per_week?, storm?, starts_on?, ends_on?, rank? }` → `{ goal }`
  - `update_goal { goal_id, …dieselben Felder (null entfernt), state? }` → `{ goal }`
  - `plan_day { date?: "JJJJ-MM-TT" (Standard heute in ServerZone), task_ids: [string] }` → `{ date, today: TodayListView }`; setzt `planned_on`/`day_rank` (Ränge `a0, a1, …` per Rangschlüssel-Erzeugung wie im Client), setzt bei nicht geschlossenen, lebenden Aufgaben mit `planned_on <= date`, die nicht in der Liste stehen, `planned_on = null`; ein `mutate_atomic`. Ungültige ID/Kennung, gelöschte, `done`/`dropped` Aufgabe oder Doppelte → Fehler ohne Schreiben.
  - `goal_input { week?: "JJJJ-Www" }` → `WeekGoalInput` mit Zielname und -zeichen je Eintrag
  - `done_log { from?: date, to?: date }` (Standard: letzte 14 Tage bis heute; höchstens 92 Tage) → `{ days: [LogDay mit Titel, Kennung, Ziel-Zeichen je Eintrag] }`
  - `create_task`/`update_task`: `goal_id` (ID; `null` entfernt), `planned_on`, `day_rank`.
  - `filter_tasks`: `goal_id` (ID oder `"none"` = ohne effektives Ziel), `planned_on` (Datum).
  - `TodayListView = { items: [TaskView], focus: TaskView|null, has_plan, plan_done }`.

- [ ] **Step 1: Failing tests** in `mcp.rs`: `create_and_list_goals`, `update_goal_closes`, `plan_day_orders_and_clears_leftovers` (Reste vom Vortag nicht in der Liste → `planned_on` null; erledigte Aufgabe mit `planned_on` heute bleibt), `plan_day_rejects_unknown_or_closed_task_and_writes_nothing` (Review Focus 5), `goal_input_counts_done_and_flames`, `done_log_lists_done_and_worked`, `filter_tasks_by_goal_none`, `update_task_rejects_flame_log`. Snapshot `mcp_tools.json` aktualisieren.
- [ ] **Step 2:** `cargo test -p zunder-server --test mcp` → FAIL.
- [ ] **Step 3:** Implementieren; Server-Anweisungen ergänzen: Ziele pflegt Claude (Feuersturm aus dem Vault), jede neue Aufgabe bekommt ihr Ziel ausdrücklich, Ruben pflegt nichts; `plan_day` bestimmt den ganzen Tag, eine Glut-Aufgabe für mich bevorzugt nach vorn.
- [ ] **Step 4:** `cargo test -p zunder-server` → PASS.
- [ ] **Step 5: Commit** `feat(server): MCP für Ziele, Tagesplan, Ziel-Input und Logbuch`

### Task 7: Web-Sync kennt Ziele und neue Felder, Regeln gespiegelt

**Files:**
- Modify: `web/src/sync/types.ts` (`EntityKind` + `"goal"`, `GOAL_FIELDS`, `Goal`, `GoalState`, `FlameInterval`, Task-Felder)
- Modify: `web/src/sync/model.ts` (`toGoal`, Task-Felder), `web/src/sync/validate.ts` (Shapes `date`, `positive_count`, `flame_log`; Pflichtfelder, Labels „Ziel“; `goal_id`-Referenz; `flame_log` vom Client abgelehnt), `web/src/sync/ids.ts` (`goal_id` als ID-Feld)
- Modify: `web/src/state/store.ts` (`data.goals`, `goalList`)
- Create: `web/src/sync/planning.ts` (Spiegel von Task 4)
- Modify: `web/test/vectors.test.ts` (Arten `today_list`, `goal_input`, `done_log`)
- Modify: `web/e2e/mock-server.ts` (Entität `goal`, `flame_log` nur vom Server, Flammen-Stopp hängt an)
- Test: `web/test/planning.test.ts`, `web/test/vectors.test.ts`

**Interfaces:**
- Consumes: Vektoren 63–67.
- Produces (TS):
  - `effectiveGoal(index: TaskIndex, goals: Record<string, Goal>, task: Task): Goal | null`
  - `todayList(index: TaskIndex, today: string, nowMs: number): { items: Task[]; focus: Task | null; has_plan: boolean; plan_done: boolean }`
  - `goalInput(index, goals, week: string, nowMs: number): { week; from; to; goals: { goal: Goal; done: number; minutes: number; lead_per_week: number | null; reached: boolean }[] }`
  - `doneLog(index, from: string, to: string, nowMs: number): { date: string; minutes: number; done: { task: Task; minutes: number }[]; worked: { task: Task; minutes: number }[] }[]`
  - Vektorvergleich auf IDs (`items`/`focus` als IDs abbilden).

- [ ] **Step 1: Failing tests:** `vectors.test.ts` läuft für 63–67 rot; `planning.test.ts`: `focus skips waiting_on`, `closed goal keeps emoji in log but not in goal input` (Review Focus 3).
- [ ] **Step 2:** `cd web && npm test` → FAIL.
- [ ] **Step 3:** Implementieren.
- [ ] **Step 4:** `cd web && npm run typecheck && npm test` → PASS.
- [ ] **Step 5: Commit** `feat(web): Ziele und Tagesplan im Sync, Regeln wie im Kern`

### Task 8: Heute als Reihenfolge mit Fokus

**Files:**
- Modify: `web/src/views/Lists.tsx` (`TodayView` auf `todayList`), `web/src/state/selectors.ts` (`todayGroups` ersetzt durch `todaySections()`: `{ list, focus, has_plan, plan_done, reminders, followUps }`; Erinnerungen/Wiedervorlage ohne Aufgaben, die in `list` stehen)
- Create: `web/src/components/TodayFocus.tsx` (Fokus-Kasten; Fallback-Logik von „Erst für dich“ aus `FocusSection.tsx` übernommen, danach `FocusSection.tsx` löschen)
- Modify: `web/src/state/actions.ts` (`planForToday(id)`, `removeFromToday(id)`, `placeInToday(id, before, after)`, `deferTask` verschiebt `planned_on` mit, `chooseFocus` → `planForToday` vorn)
- Modify: `web/src/state/task-menu.ts` (Einträge „Für heute einplanen“ / „Aus Heute nehmen“), `web/src/components/TaskDetail.tsx` (dieselben Aktionen)
- Modify: Seitenleisten-Zahl (Ort der Heute-Zahl in `web/src/App.tsx`/`selectors.ts`)
- Modify: `web/src/labels.ts` (Texte), `web/src/styles.css`
- Create: `web/e2e/today-plan.spec.ts`; Modify: `web/e2e/focus.spec.ts` (an neues Verhalten anpassen, Fälle behalten)

**Interfaces:**
- Consumes: Task 7 (`todayList`), `rankBetween` (`web/src/sync/rank.ts`).
- Produces: `planForToday(id: string, position?: "front" | "back"): boolean`, `removeFromToday(id: string): boolean`, `placeInToday(id: string, before: Task | null, after: Task | null): boolean`.

Verhalten (Entwurf 5): eine Liste ohne Zwischenüberschriften, Fokus-Kasten oben („Fokus“), Rest unter „Danach“; kein roter Block; Ziehen wie im Board (HTML5 Drag & Drop) und `Shift+J/K` setzen `day_rank` (und `planned_on` = heute bei ungeplanten); Tag ohne Plan → „Erst für dich“-Regel im Kasten (passende Glut für mich ohne Klick, sonst leise Zeile + bis zu drei Wahlen, „Für heute ausblenden“ blendet nur den Vorschlag aus); Plan erledigt → „Plan für heute erledigt.“; Zeilen zeigen das Ziel-Zeichen.

- [ ] **Step 1: Failing E2E** `today-plan.spec.ts`: `geplante Aufgaben stehen in Reihenfolge, erste im Fokus`; `liegengebliebene und fällige stehen ohne Rot unter Danach` (kein `.tone-danger`); `ziehen ändert die Reihenfolge`; `Plan erledigt zeigt ruhige Zeile ohne Vorschläge`; `ohne Plan steht Glut für mich im Fokus`; `Heute nicht nimmt eine geplante Aufgabe auf morgen mit`; `Aus Heute nehmen per Kontextmenü`.
- [ ] **Step 2:** `cd web && npx playwright test today-plan focus` → FAIL.
- [ ] **Step 3:** Implementieren.
- [ ] **Step 4:** `cd web && npm run typecheck && npm test && npx playwright test` → PASS.
- [ ] **Step 5: Commit** `feat(web): Heute als Reihenfolge mit einer Aufgabe im Fokus`

### Task 9: Ziele-Ansicht und Ziel an der Aufgabe

**Files:**
- Create: `web/src/views/Goals.tsx` (Route `#/ziele`; Tastenkürzel siehe Step 3)
- Modify: `web/src/state/router.ts`, `web/src/App.tsx` (Seitenleiste „Ziele“), `web/src/keys.ts`, `web/src/labels.ts`, `web/src/styles.css`
- Modify: `web/src/components/TaskRow.tsx` (Ziel-Zeichen), `web/src/components/TaskDetail.tsx` (Ziel anzeigen und wählen, Auswahl nur aktive Ziele + „Ohne Ziel“)
- Create: `web/e2e/goals.spec.ts`

**Interfaces:**
- Consumes: Task 7 (`goalInput`, `effectiveGoal`, `data.goals`).

Verhalten (Entwurf 6): aktive Ziele nach `rank`, je Zeile Zeichen, Name, „n Zug/Züge“, Minuten (fehlen bei 0), `(n×)` ruhig, stilles Häkchen bei `reached`; darunter Feuersturm (`storm`) und „noch n Wochen“ bis `ends_on`; Klick öffnet `lead` und die Aufgaben der Woche mit diesem Ziel (erledigt und mit Flammen). Keine Farben für Nicht-Erreichen, keine Balken. Leerzustand ohne Ziele: „Noch keine Ziele. Claude legt sie aus dem Feuersturm an.“

- [ ] **Step 1: Failing E2E** `goals.spec.ts`: `zeigt Züge, Minuten und Leitkennzahl ruhig`; `Häkchen nur bei erreicht, sonst nichts`; `geschlossenes Ziel fehlt`; `Ziel im Detail setzen zeigt Zeichen in der Zeile`.
- [ ] **Step 2:** `cd web && npx playwright test goals` → FAIL.
- [ ] **Step 3:** Implementieren; Tastenkürzel: freien Buchstaben im Schema `g …` wählen (`g z` ist „Zusagen“) und in der Tastenkürzel-Übersicht eintragen.
- [ ] **Step 4:** `cd web && npm run typecheck && npm test && npx playwright test` → PASS.
- [ ] **Step 5: Commit** `feat(web): Ziele-Ansicht mit dem Input der Woche`

### Task 10: Logbuch „Erledigt“

**Files:**
- Create: `web/src/views/DoneLog.tsx` (Route `#/erledigt`)
- Modify: `web/src/state/router.ts`, `web/src/App.tsx`, `web/src/keys.ts`, `web/src/labels.ts`, `web/src/styles.css`
- Create: `web/e2e/done-log.spec.ts`

**Interfaces:**
- Consumes: Task 7 (`doneLog`).

Verhalten (Entwurf 7): Tage absteigend, Kopf mit Wochentag/Datum und Tagessumme (fehlt bei 0), Zeilen mit Haken, Titel, Ziel-Zeichen, Minuten; darunter „Daran gearbeitet“ mit eigenem Zeichen; anfangs 14 Tage, „Ältere zeigen“ lädt je 14 weitere; Tage ohne Einträge entfallen; Leerzustand ohne Einträge in 14 Tagen ohne Wertung („Hier erscheint, was du erledigst.“).

- [ ] **Step 1: Failing E2E** `done-log.spec.ts`: `zeigt erledigte Aufgaben mit Minuten und Tagessumme`; `Daran gearbeitet steht unter dem Erledigten`; `Dev-Aufgaben und verworfene fehlen`; `Ältere zeigen lädt weitere Tage`.
- [ ] **Step 2:** `cd web && npx playwright test done-log` → FAIL.
- [ ] **Step 3:** Implementieren.
- [ ] **Step 4:** `cd web && npm run typecheck && npm test && npx playwright test` → PASS.
- [ ] **Step 5: Commit** `feat(web): Logbuch „Erledigt“ mit Flammen-Minuten`

### Task 11: Vertrag, Doku, Gesamtprüfung

**Files:**
- Modify: `docs/superpowers/plans/2026-10-11-zunder-implementation.md` (Datenmodell: Entität `goal`, neue Felder; Sichten 5.1: Heute, Ziele, Erledigt; „Erst für dich“ als Fallback; MCP-Tabelle; Vektoren 63–67; Mock-Server-Umfang)
- Modify: `docs/superpowers/specs/2026-10-11-zunder-design.md` (Felder-Tabelle um die vier Aufgabenfelder, Verweis auf den neuen Entwurf)
- Modify: `README.md` nur falls die Etappen-Tabelle betroffen ist (sonst nicht)

- [ ] **Step 1:** Doku ergänzen.
- [ ] **Step 2:** Alle Qualitätsschranken aus „Global Constraints“ inklusive Playwright gegen Mock und echten Server → grün. Ausgabe festhalten.
- [ ] **Step 3: Commit** `docs: Vertrag um Ziele, Tagesplan und Logbuch ergänzt`
- [ ] **Step 4:** Push, PR #77 aktualisieren (Beschreibung: Änderungen, Versionshinweis App-Update aus Task 3, Migration 0010, Nachfüllung `flame_log`, geprüfte Gates), Zunder: Kommentar „PR offen“, Status `review`, `release_task`.
