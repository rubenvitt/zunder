# Timing-Stunden pro Ziel – Implementierungsplan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Zu jedem Ziel die Timing-Zeiteinträge der Woche zeigen (Ziele-Ansicht, App, MCP `goal_input`), Zuordnung über das neue Ziel-Feld `timing_projects`.

**Architecture:** `timing_projects` ist ein synchronisiertes Feld der Entität `goal` (Rust-Kern und TS gespiegelt, gemeinsamer Testvektor). Der Server fragt Timings `GET /report` einmal je Woche für alle zugeordneten Projekte ab, verteilt die Sekunden auf die Ziele, hält das Ergebnis kurz im Speicher und liefert es über `GET /api/goal-hours`. Web-Oberfläche und App holen die Zahlen wie den PR-Status und zeigen still nichts, wenn keine kommen.

**Tech Stack:** Rust (axum, reqwest, time), SolidJS/TypeScript, Tauri (`zunder-local`), Playwright.

**Spec:** `docs/superpowers/specs/2026-11-24-timing-stunden-pro-ziel-design.md`

## Global Constraints

- Es zählen nur Zeiteinträge: `include_app_usage` nie setzen; `include_child_projects=0`.
- Self-Link-Form `/projects/<Ziffern>`; Liste ohne Duplikate, höchstens 50 Einträge.
- Zwischenspeicher: laufende Woche 5 min, vergangene Wochen 60 min, Fehler nie.
- Wochengrenzen: ISO-Woche in `ServerZone`, an Timing als RFC 3339 mit Offset.
- Anzeige: ab 60 min „3,5 h Timing“ (höchstens eine Nachkommastelle, Komma), darunter „40 min Timing“; bei 0, ohne Zuordnung, `not_configured`, `unavailable`, offline, Fehler: nichts.
- Keine Titel oder Notizen von Zeiteinträgen abfragen oder speichern.
- Deutsche Oberfläche und Meldungen; Conventional Commits auf Deutsch (`feat(core)`, `feat(server)`, `feat(local)`, `feat(app)`, `feat(web)`, `docs`).
- Qualitätsschranken `docs/entwicklung.md`: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `npm run typecheck`, `npm test`, Playwright gegen Mock.

## Review Focus

1. Timing antwortet langsam oder gar nicht: Die Ziele-Ansicht zeigt Züge und Minuten sofort, `goal_input` antwortet nach höchstens 5 s mit `timing_status: "unavailable"` (Test in Task 4).
2. Flammen-Projekt steht doch in einer Zuordnung (Einstellung später geändert): Rechnung lässt es weg (Test in Task 3).
3. Ein Ziel wird geschlossen oder gelöscht: Seine Projekte zählen nicht mehr und werden nicht abgefragt (Test in Task 3).
4. Älterer Server ohne `/api/goal-hours` (404): App meldet `unsupported`, Oberfläche zeigt still nichts (Test in Task 5).
5. Projekt in zwei aktiven Zielen (über die App oder alte Daten): zählt für beide, kein Absturz; MCP verhindert neue Doppelungen (Tests in Task 3 und 4).

---

### Task 1: Feld `timing_projects` am Ziel (Kern und TS)

**Files:**
- Modify: `crates/zunder-core/src/model.rs` (Feldkonstante, `ALL`, `Shape::TimingProjects`, `check_shape`, `Goal.timing_projects`, Tests `goal_fields`)
- Modify: `crates/zunder-core/src/state.rs` (`to_goal`)
- Modify: `crates/zunder-server/src/views.rs` (`GoalView.timing_projects`), alle `Goal { … }`-Literale in Tests
- Modify: `web/src/sync/types.ts` (`GOAL_FIELDS`, `Goal`), `web/src/sync/model.ts` (`toGoal`), `web/src/sync/validate.ts` (Shape `timing_projects`, Label)
- Create: `crates/zunder-core/tests/vectors/69-validierung-ziel-timing-projekte.json`

**Interfaces:**
- Produces: `fields::goal::TIMING_PROJECTS = "timing_projects"`; `pub const MAX_TIMING_PROJECTS: usize = 50`; `Goal.timing_projects: Vec<String>` (fehlend/null = leer); `pub fn is_timing_project_ref(s: &str) -> bool` in `model.rs`; TS `Goal.timing_projects: string[]`.

- [ ] Test `goal_fields` erweitern: ok `["/projects/1","/projects/37"]`, ok `[]`, ok `null`; invalid `["/projects/1","/projects/1"]`, `["projects/1"]`, `["/projects/abc"]`, `["/projects/"]`, `"/projects/1"`, 51 Einträge.
- [ ] Vektor 69: Ziel A mit `timing_projects: ["/projects/1","/projects/1"]` → `invalid_value`, Ziel B mit `["/projects/3708302322996078848"]` angenommen (Form wie Vektor 63-…-werte).
- [ ] Rust und TS umsetzen; `toGoal` liefert `[]` für alles außer einer gültigen Liste.
- [ ] `cargo test -p zunder-core`, `npm test` (vectors, validate) grün.
- [ ] Commit `feat(core): Ziele nennen ihre Timing-Projekte (timing_projects)`.

### Task 2: Timing-Bericht in der Brücke

**Files:**
- Modify: `crates/zunder-server/src/timing.rs`

**Interfaces:**
- Produces: `pub struct ProjectTime { pub project: String, pub seconds: f64 }`; `TimingApi::report<'a>(&'a self, from: OffsetDateTime, to: OffsetDateTime, projects: &'a [String]) -> BoxFuture<'a, Result<Vec<ProjectTime>, TimingError>>`; `pub fn project_path(s: &str) -> Option<String>` (normalisiert `/projects/1`, `https://…/api/v1/projects/1/`, `"1"` auf `/projects/1`).
- Request: `GET /report?start_date_min=<rfc3339>&start_date_max=<rfc3339>&columns[]=project&include_child_projects=0&projects[]=…` (Kodierung über `url::Url::query_pairs_mut`). Antwort `{"data":[{"duration":<s>,"project":{"self":"/projects/1"}|null}]}`; Zeilen ohne Projekt verwerfen, Self-Links über `project_path` normalisieren.

- [ ] Unit-Tests: `project_path` (drei Formen, Müll → None); Query-Aufbau einer Hilfsfunktion `report_query(from, to, projects) -> Vec<(String, String)>` (enthält genau einmal `columns[]=project`, `include_child_projects=0`, kein `include_app_usage`, Zeitpunkte mit Offset).
- [ ] Umsetzen (`TimingClient::report`, Trait-Methode).
- [ ] Commit `feat(server): Timing-Bericht über die Brücke abfragen`.

### Task 3: `GET /api/goal-hours` mit Verteilung und Zwischenspeicher

**Files:**
- Create: `crates/zunder-server/src/goal_hours.rs` (Verteilung, Zwischenspeicher, Wochengrenzen)
- Modify: `crates/zunder-server/src/app.rs` (`App::goal_hours`, Cache-Feld in `Inner`), `src/lib.rs`, `src/http/api.rs` (Route)
- Create: `crates/zunder-server/tests/goal_hours.rs` (Mock-Timing nach dem Muster von `tests/flame.rs`)

**Interfaces:**
- Consumes: `TimingApi::report`, `project_path`, `Goal.timing_projects`.
- Produces:
  - `pub fn week_bounds(week: IsoWeek, zone: &impl LocalZone) -> (OffsetDateTime, OffsetDateTime)` (Mo 00:00 bis So 23:59:59 lokal).
  - `pub fn distribute(goals: &[(Id, Vec<String>)], times: &[ProjectTime], exclude: Option<&str>) -> BTreeMap<Id, u32>` (Minuten abgerundet nach Summe; Ziele mit 0 fehlen).
  - `#[derive(Serialize)] pub struct GoalHours { week: String, status: GoalHoursStatus, goals: BTreeMap<Id, u32>, #[serde(skip_serializing_if = "Option::is_none")] message: Option<String> }`, `enum GoalHoursStatus { Ok, NotConfigured, Unavailable }` (snake_case).
  - `App::goal_hours(&self, week: IsoWeek) -> GoalHours` (nie Fehler; Timeout 5 s um den Timing-Aufruf).
  - `App::flame_project_ref(&self, bridge: &TimingBridge) -> Option<String>`: Self-Link der Einstellung oder per `projects()` aufgelöster Titel.
  - Route `GET /api/goal-hours?week=JJJJ-Www` (ohne `week`: laufende Woche; ungültig → 400 `invalid_request`).
- Nur aktive, lebende Ziele mit nicht leerer Zuordnung; keine Zuordnung → kein Aufruf, `status: ok`, `goals: {}`.

- [ ] Unit-Tests `distribute`: zwei Projekte je Ziel summiert; Projekt in zwei Zielen zählt für beide; `exclude` wird weggelassen; 59 s → fehlt; `week_bounds` in Winter- und Sommerzeit (FixedZone).
- [ ] Integrationstests (`tests/goal_hours.rs`): ohne Token `not_configured`; mit Mock → Minuten je Ziel, Request enthält beide Projekte und `include_child_projects=0`; zweiter Abruf aus dem Zwischenspeicher (Mock zählt 1 Aufruf); geschlossenes Ziel wird nicht abgefragt; Mock 500 → `unavailable` mit Meldung, danach erneuter Aufruf (Fehler nicht gespeichert); Flammen-Projekt in Zuordnung wird nicht gezählt; `week=quatsch` → 400.
- [ ] Umsetzen.
- [ ] Commit `feat(server): Timing-Stunden pro Ziel und Woche (GET /api/goal-hours)`.

### Task 4: MCP

**Files:**
- Modify: `crates/zunder-server/src/mcp/schema.rs` (`goal_fields`, Beschreibung `goal_input`, Server-Anweisungen), `src/mcp/tools.rs` (`GOAL_FIELDS`, create/update_goal, goal_input), `src/views.rs`
- Test: `crates/zunder-server/tests/mcp.rs`

**Interfaces:**
- Consumes: `project_path`, `App::goal_hours`, `App::flame_project_ref`, `TimingApi::projects`.
- `timing_projects` in create/update_goal: Liste von Texten oder `null`; jeder Eintrag über `project_path` normalisiert (unlesbar → `invalid_arguments`), Duplikate entfernt. Ablehnung (`invalid_arguments`, deutsch): Flammen-Projekt; Projekt eines anderen aktiven Ziels („… ist schon dem Ziel „Name“ zugeordnet.“); bei erreichbarem Timing ein Projekt, das nicht in `projects()` steht. Timing nicht erreichbar → speichern, `warnings: ["Timing war nicht erreichbar; die Projekte sind ungeprüft."]`.
- `goal_input`: je Ziel `timing_minutes` (Zahl oder `null`), oben `timing_status`, bei `unavailable` `timing_message`.

- [ ] Tests: Normalisierung (URL, Zahl) gespeichert als `/projects/N`; Flammen-Projekt abgelehnt; doppelte Zuordnung zu zweitem aktivem Ziel abgelehnt, zu geschlossenem erlaubt; `null` entfernt; `goal_input` ohne Token → `timing_status: "not_configured"`, `timing_minutes: null`.
- [ ] Umsetzen; `list_goals` zeigt `timing_projects` über `GoalView`.
- [ ] Commit `feat(server): Timing-Projekte über MCP zuordnen, goal_input mit Timing-Minuten`.

### Task 5: App-Befehl `goal_hours`

**Files:**
- Modify: `apps/zunder-app/src-tauri/zunder-local/src/client.rs`, `apps/zunder-app/src-tauri/src/commands.rs`, `apps/zunder-app/src-tauri/src/lib.rs`
- Modify: `web/src/sync/transport.ts`, `web/src/platform/tauri.ts`, `web/src/sync/types.ts`

**Interfaces:**
- Produces: `HttpClient::goal_hours(&self, week: Option<&str>) -> Result<serde_json::Value>` (`GET api/goal-hours[?week=…]`, 404 → `Error::Unsupported`); Tauri-Befehl `goal_hours(week: Option<String>)`; TS `Api.goalHours(week?: string): Promise<GoalHours>`; `interface GoalHours { week: string; status: "ok" | "not_configured" | "unavailable"; goals: Record<string, number>; message?: string }`.

- [ ] Test in `client.rs` nach `pr_status_passes_through_and_404_is_unsupported`: Request-Zeile `GET /zunder/api/goal-hours?week=2026-W48 HTTP/1.1`, 404 → `unsupported`.
- [ ] Umsetzen.
- [ ] Commit `feat(local): Timing-Stunden pro Ziel vom Server holen`.

### Task 6: Ziele-Ansicht

**Files:**
- Create: `web/src/state/goal-hours.ts`
- Modify: `web/src/views/Goals.tsx`, `web/src/labels.ts`, `web/src/styles.css` (Spalte), `web/e2e/mock-server.ts`, `web/e2e/goals.spec.ts`
- Test: `web/test/labels.test.ts` (oder bestehende Label-Tests), `web/test/goal-hours.test.ts`

**Interfaces:**
- Consumes: `api().goalHours(week)`.
- Produces: `timingLabel(minutes: number): string` (`40` → „40 min Timing“, `60` → „1 h Timing“, `210` → „3,5 h Timing“, `367` → „6,1 h Timing“); `createGoalHours(fetch: (week: string) => Promise<GoalHours>, now?: () => number)` mit `minutes(goalId): number | undefined`, `watch(week: string): () => void` (sofort und alle 5 min, bei `visibilitychange` sichtbar erneut; Fehler/`unsupported`/`not_configured`/`unavailable` → leer); Modul-Singleton `goalHours` wie `prs.ts`.
- Ziele-Zeile: neue Spalte `<span class="goal-timing">` nach `goal-minutes`, leer wenn `minutes` fehlt oder 0. Detail: Zeile „Timing: 6 h“ (gleiche Formatierung ohne „Timing“-Suffix) wenn vorhanden.
- Mock-Server: `GET /api/goal-hours` → `{week, status:"ok", goals}` mit 210 min für jedes aktive Ziel, dessen `timing_projects` nicht leer ist.

- [ ] Tests `timingLabel`; Store: Fehler → `undefined`, Werte gesetzt, erneuter Abruf nach 5 min (Fake-Zeit).
- [ ] E2E (nur `isMock`): Ziel mit `timing_projects: ["/projects/1"]` zeigt „3,5 h Timing“, Ziel ohne zeigt nichts davon.
- [ ] Umsetzen.
- [ ] Commit `feat(web): Ziele zeigen die Timing-Zeit der Woche`.

### Task 7: Doku

**Files:**
- Modify: `docs/superpowers/specs/2026-11-23-ziele-heute-logbuch-design.md` (Nicht-Ziel mit Verweis), `docs/superpowers/plans/2026-10-11-zunder-implementation.md` (Ziel-Feld, Route, MCP), `docs/betrieb.md` (Token auch für Berichte; iOS vor erster Zuordnung)

- [ ] Ändern, Commit `docs: Timing-Stunden pro Ziel im Vertrag und Betrieb`.
