# Zunder – Implementierungsvertrag

- **Datum:** 2026-10-11
- **Bezug:** [`../specs/2026-10-11-zunder-design.md`](../specs/2026-10-11-zunder-design.md)

Dieses Dokument legt die Schnittstellen fest, an die sich alle Bausteine halten. Wo die Spec offen ist, entscheidet dieses Dokument. Änderungen am Vertrag werden hier nachgezogen, nicht nur im Code.

## 1. Repository-Layout

```
Cargo.toml                 Workspace (resolver 2, edition 2021)
crates/zunder-core/        Domäne, Validierung, HLC, Merge (keine I/O-Abhängigkeiten)
crates/zunder-server/      axum, SQLite (rusqlite, bundled), Sync-API, /mcp, OIDC, Timing
crates/zunder-import/      CLI für den ClickUp-Import
web/                       zunder-web: SolidJS + Vite + TypeScript, Vitest, Playwright
apps/zunder-app/           Tauri 2 (E4); src-tauri ist NICHT Workspace-Mitglied (braucht webkit)
deploy/                    compose.yaml, stack.env (Vorlage), generate-secrets.sh (Portainer/Traefik)
Dockerfile                 Multi-Stage: Web bauen, Server bauen, schlankes Runtime-Image
.github/workflows/ci.yml   fmt, clippy -D warnings, Tests, Web, Playwright, GHCR
docs/                      Spec, dieser Vertrag, betrieb.md, umstieg.md, entwicklung.md
```

`deploy/stack.env` ist eine eingecheckte Vorlage mit Platzhaltern; `deploy/generate-secrets.sh` schreibt das befüllte Ergebnis nach `deploy/stack.env.local` (von Git ignoriert). Die Tauri-App wird in der CI nicht gebaut (Begründung als Kommentar in `ci.yml`).

Code-Bezeichner sind englisch, Doku und UI deutsch.

## 2. Wire-Format

### 2.1 IDs

UUIDv7 als kleingeschriebener Bindestrich-String. Clients erzeugen IDs selbst. Nur diese kanonische Form ist gültig (36 Zeichen, klein, mit Bindestrichen); andere Schreibweisen werden schon beim Einlesen abgelehnt.

Deterministische Import-IDs: UUIDv5 im Zunder-Namespace `f9d16046-2e7f-4f97-b1be-e789d21e4fec` über den Namen `clickup:<entity>:<clickup_id>` (z. B. `clickup:task:86abc123`), siehe `zunder_core::ids::clickup_entity_id`.

### 2.2 Hybrid Logical Clock

String `"<wall_ms>:<counter>:<node>"`:

- `wall_ms`: Unix-Millisekunden, **13 Stellen**, mit Nullen aufgefüllt
- `counter`: **5 Stellen**, dezimal, mit Nullen aufgefüllt (0–99999)
- `node`: Knoten-ID, `[a-z0-9-]{1,36}` (z. B. `server`, `web-3f9a…`, `import`)

Ordnung = **lexikografischer Vergleich des ganzen Strings** (durch feste Breiten identisch mit Tupelvergleich). Gleichstand ist bei verschiedenen Knoten unmöglich; bei identischem HLC entscheidet `op_id` lexikografisch (größer gewinnt).

Regeln: `now()` liefert `max(physisch, letzter)`; gleicher `wall_ms` → `counter + 1`. `receive(remote)` hebt die lokale Uhr auf `max(lokal, remote)` an. Läuft der Zähler über (99999), rückt `wall_ms` um 1 vor und der Zähler beginnt bei 0. Der Server lehnt Operationen ab, deren `wall_ms` mehr als 24 h in der Zukunft liegt.

### 2.3 Operation

```json
{
  "op_id": "01929c3e-…",
  "entity": "task",
  "entity_id": "01929c3e-…",
  "field": "title",
  "value": "Zahnarzt anrufen",
  "hlc": "1728650000000:00000:web-3f9a"
}
```

- `entity` ∈ `area` | `project` | `task`
- Eine Entität entsteht durch ihre erste Operation. Anlegen = mehrere Feld-Operationen mit derselben `entity_id` in einem Push.
- Löschen = Feld `deleted` mit Wert `true` (Tombstone). Gelöschte Entitäten bleiben im Zustand, werden aber nirgends angezeigt. `deleted: false` (oder `null`) stellt eine Entität wieder her (Rückgängig); danach gelten alle Regeln erneut.
- `value: null` setzt ein optionales Feld zurück. Der Schlüssel `value` muss immer vorhanden sein. Pflichtfelder lassen sich nicht auf `null` setzen.
- Merge: **Last-Writer-Wins pro (Entität, Feld)** nach HLC, Gleichstand nach `op_id`. Merge ist kommutativ, assoziativ und idempotent. (Sind sogar HLC und `op_id` gleich, entscheidet eine feste Ordnung auf dem JSON-Wert, damit auch fehlerhafte Duplikate konvergieren.)

### 2.4 Felder

**area**: `name` (String, Pflicht, nicht leer, ≤ 500 Zeichen), `state` (`active`|`archived`, Default `active`), `vault_path` (optional, nur Info), `deleted`.

Die Area-`id` **ist** die `zunder_id` im Vault-Frontmatter. Gleiches gilt für Projekte.

**project**: `name` (Pflicht, nicht leer, ≤ 500 Zeichen), `area_id` (Pflicht, existierende Area), `status` (`active`|`paused`|`completed`, Default `active`), `kind` (`standard`|`dev`, Default `standard`), `repo` (`owner/name` im GitHub-Format, nur bei `dev`, optional; bei `standard` abgelehnt), `description` (Markdown), `view` (`list`|`board`, Default: `board` bei dev, sonst `list`), `deleted`.

**task**:

| Feld | Wert |
|---|---|
| `title` | String, Pflicht, nicht leer, ≤ 500 Zeichen |
| `description` | Markdown-String |
| `type` | `aufgabe` (Default) \| `bug` \| `feature` |
| `status` | `open` (Default) \| `in_progress` \| `review` \| `blocked` \| `done` \| `dropped` |
| `location` | `{"kind":"inbox"}` \| `{"kind":"loose"}` \| `{"kind":"area","id":…}` \| `{"kind":"project","id":…}`; Default inbox |
| `parent_id` | Task-ID oder `null` |
| `due` | `"YYYY-MM-DD"` oder RFC 3339 mit Offset |
| `remind_at` | RFC 3339 |
| `vault_links` | Array nicht-leerer Strings (Vault-Pfade oder `obsidian://`-URIs) |
| `links` | Array von `{"url": String, "title"?: String}`; `url` absolut (beliebiges Schema), keine weiteren Schlüssel |
| `phoenix` | `null` oder `{"role":"feuer_schritt"\|"flamme","fire":"2026-W45"}`; `fire` ist eine gültige ISO-Woche `JJJJ-Www` |
| `claimed_by`, `claim_expires_at` | String, RFC 3339 |
| `plan` | Markdown-String |
| `branch` | String |
| `pr_url` | http(s)-URL |
| `timing_ref` | String (ID des Timing-Eintrags) |
| `flame_started_at`, `flame_ended_at` | RFC 3339 (Fallback, Abschnitt 6 der Spec) |
| `clickup_id` | String |
| `rank` | String, lexikografisch sortierbar (Reihenfolge in Liste/Spalte) |
| `deleted` | `true` (Tombstone); `false`/`null` = wiederherstellen |

Unteraufgaben: Die wirksame Stelle einer Unteraufgabe ist die Stelle ihrer Wurzel-Aufgabe; das eigene `location` einer Unteraufgabe wird ignoriert.

`location` erlaubt genau die gezeigten Schlüssel. Bei Unteraufgaben wird das eigene `location` weder ausgewertet noch auf gültige Referenzen geprüft.

Abgeleitet (nicht als Feld gespeichert): `created_at` (kleinster HLC aller je angewendeten Operationen der Entität; im Snapshot als `created_hlc` mitgeliefert), `updated_at` (größter HLC), `status_changed_at` (HLC der gewinnenden `status`-Operation, ohne `status`-Operation = `created_at`), Fortschritt der Unteraufgaben (`done`/`gesamt`, `dropped` zählt nicht mit).

### 2.5 Validierung (im Kern)

Der Server validiert jeden Push gegen seinen aktuellen Zustand. Der Merge selbst validiert nicht.

- Unbekanntes Feld, falscher Typ oder ungültiger Wert → Ablehnung.
- Pflichtfelder: Eine neue Entität muss nach Anwendung des Pushs ihre Pflichtfelder haben.
- `bug`/`feature` und `review` nur, wenn die wirksame Stelle ein Dev-Projekt ist.
- Referenzen (`location`, `area_id`, `parent_id`) müssen auf existierende, nicht gelöschte Entitäten zeigen. `parent_id` ohne Zyklen.
- Ein Projekt darf nur von `dev` auf `standard`, wenn es keine `bug`/`feature`/`review`-Aufgaben enthält; eine Aufgabe mit `bug`/`feature`/`review` darf nicht aus einem Dev-Projekt heraus verschoben werden.
- Statusübergänge sind frei (auch `done → open`), außer den Dev-Regeln.
- **Hinweise statt Ablehnung:** `done` bei offenen Unteraufgaben erzeugt eine Warnung.
- Granularität: Ist eine Entität nach dem Push ungültig, werden **alle** Operationen dieses Pushs für diese Entität abgelehnt, jede mit Grund. Andere Entitäten sind nicht betroffen.

Präzisierungen (umgesetzt in `zunder_core::validate`):

- Pflichtfelder gelten für jede berührte, nicht gelöschte Entität (bestehende haben sie ohnehin). Gelöschte Entitäten werden nicht weiter geprüft.
- **Löschen** ist nur erlaubt, wenn danach keine lebende Entität mehr auf das Gelöschte verweist (Projekte eines Bereichs, Aufgaben eines Bereichs/Projekts, Unteraufgaben). Die Abhängigen werden im selben Push mitgelöscht (`OpBatch::delete_cascade`) oder vorher verschoben. So zeigen Referenzen nach jedem Push auf lebende Entitäten.
- Verschieben einer `bug`/`feature`/`review`-Aufgabe in ein **anderes** Dev-Projekt ist erlaubt; verboten ist nur, dass sie danach außerhalb eines Dev-Projekts liegt.
- Verletzungen an nicht berührten Nachbarn (z. B. Bug-Unteraufgabe, deren Eltern verschoben werden; Aufgaben eines Projekts, das auf `standard` wechselt; Unteraufgaben einer gelöschten Aufgabe) werden der berührten Entität zugeordnet, die sie verursacht hat. Bereits vorher bestehende, nicht durch den Push verursachte Inkonsistenzen blockieren keine fremden Änderungen.
- Wird eine Entität abgelehnt, wird der Push ohne sie erneut geprüft (z. B. wird eine Aufgabe in einem abgelehnten neuen Projekt ebenfalls abgelehnt).
- Eine ID gehört genau einer Entitätsart.
- Bereits bekannte `op_id`s (auch Wiederholungen innerhalb des Pushs) gelten als angenommen und werden nicht erneut angewendet.

Ablehnungscodes (`code`):

| Code | Bedeutung |
|---|---|
| `unknown_field` | Feld gibt es für diese Entitätsart nicht |
| `invalid_value` | falscher Typ oder ungültiger Wert (auch `null` für Pflichtfelder) |
| `missing_required` | Pflichtfeld fehlt nach dem Push |
| `dev_only` | `bug`/`feature`/`review` außerhalb eines Dev-Projekts, Verschieben aus einem Dev-Projekt heraus, `repo` bei `standard` |
| `dev_tasks_present` | Projekt `dev → standard` oder Verschieben einer Eltern-Aufgabe würde Dev-Aufgaben außerhalb eines Dev-Projekts zurücklassen |
| `invalid_reference` | `location`, `area_id` oder `parent_id` zeigt auf eine fehlende, gelöschte oder falsche Entitätsart |
| `has_dependents` | Löschen, obwohl noch lebende Entitäten darauf verweisen |
| `cycle` | `parent_id` erzeugt einen Zyklus |
| `future_hlc` | `wall_ms` mehr als 24 h in der Zukunft |
| `kind_mismatch` | ID gehört bereits zu einer anderen Entitätsart |
| `entity_rejected` | andere Operation derselben Entität in diesem Push war ungültig |

Warnungscodes: `open_subtasks` (Aufgabe auf `done` gesetzt, obwohl Unteraufgaben weder `done` noch `dropped` sind).

Ablehnungsgründe sind maschinenlesbar (`code`) plus deutscher Text (`message`).

### 2.6 Gemeinsame Testvektoren

`crates/zunder-core/tests/vectors/*.json`, je Datei ein Objekt:

```json
{
  "name": "lww-gleichstand-op-id",
  "kind": "merge",
  "ops": [Op, …],
  "expected": { "task/<entity_id>": { "title": "…", "status": "done" } }
}
```

`expected` enthält je Entität die gewinnenden Werte aller gesetzten Felder (`null`-Werte erscheinen als `null`). Ein optionales Feld `description` erklärt den Fall in Worten. Eine Datei mit `"kind": "hlc_order"` enthält statt `ops`/`expected` das Feld `"sorted": [hlc, …]` in aufsteigender Ordnung. Rust- und TypeScript-Tests wenden die Operationen in mehreren Permutationen an und vergleichen mit `expected`.

## 3. HTTP-API (Server)

Basis: `ZUNDER_PUBLIC_URL`. JSON, UTF-8. Fehler: `{"error":{"code":…,"message":…}}`.

| Methode, Pfad | Zweck |
|---|---|
| `GET /healthz` | `{"status":"ok","version":…}`, ohne Auth |
| `GET /auth/login` | OIDC-Login (Authorization Code + PKCE) starten |
| `GET /auth/callback` | OIDC-Callback, setzt Session-Cookie `zunder_session` (HttpOnly, Secure, SameSite=Lax) |
| `POST /auth/logout` | Session beenden |
| `GET /api/me` | `{"user_id","name","email"}` |
| `GET /api/snapshot` | `{"cursor":N,"entities":[{"entity","entity_id","created_hlc","fields":{f:{"value","hlc","op_id"}}}]}` (Format von `zunder_core::state::SnapshotEntity`; `op_id` für den Gleichstand nach `op_id`, `created_hlc` für `created_at`) |
| `POST /api/sync/push` | Body `{"client_id","ops":[Op]}` → `{"accepted":[op_id],"rejected":[{"op_id","code","message"}],"warnings":[{"entity_id","code","message"}],"cursor":N}` |
| `GET /api/sync/pull?since=N&limit=M` | `{"ops":[{"seq":N,"op":Op}],"cursor":N,"has_more":bool}`; `limit` Default 1000 |
| `GET /api/sync/events` | Server-Sent Events, Ereignis `cursor` mit neuem Cursor bei jeder Änderung |
| `GET /api/tasks/{id}/history` | Änderungsprotokoll: `[{"seq","op","source"}]` |
| `POST /api/tasks/{id}/flame/start` | Flamme starten (Timing-Brücke, Fallback-Zeitstempel) |
| `POST /api/tasks/{id}/flame/stop` | Flamme beenden |
| `GET /api/tokens` / `POST /api/tokens` / `DELETE /api/tokens/{id}` | API-Tokens verwalten (nur Session-Auth) |
| `POST /mcp` (+ `GET`, `DELETE`) | MCP Streamable HTTP |

- Idempotenz: Eine bereits bekannte `op_id` gilt als `accepted`, wird aber nicht erneut gespeichert.
- `seq` ist ein monotoner Server-Zähler; der Cursor ist das größte vergebene `seq`.
- Operationen, die der Server selbst erzeugt (MCP, Timing, Import über API), laufen durch dieselbe Validierung und dasselbe Log und tragen eine `source` (`web`, `mcp:<token-name>`, `import`, `server`).

**Auth:** Session-Cookie (Browser) oder `Authorization: Bearer zt_<32 Byte base64url>` (API-Token, in der DB nur als SHA-256 gespeichert, einzeln widerrufbar, `last_used_at`). Konfiguration über Umgebungsvariablen:

| Variable | Bedeutung |
|---|---|
| `ZUNDER_BIND` | Default `0.0.0.0:8080` |
| `ZUNDER_DATA_DIR` | Default `./data` (SQLite `zunder.db`, Backups in `backups/`) |
| `ZUNDER_PUBLIC_URL` | z. B. `https://zunder.rubeen.dev` |
| `ZUNDER_WEB_DIR` | Default `./web/dist` |
| `ZUNDER_OIDC_ISSUER`, `ZUNDER_OIDC_CLIENT_ID`, `ZUNDER_OIDC_CLIENT_SECRET` | Pocket-ID |
| `ZUNDER_OIDC_ALLOWED_SUBJECT` oder `ZUNDER_OIDC_ALLOWED_EMAIL` | Wer sich anmelden darf (eine Person) |
| `ZUNDER_SESSION_SECRET` | Signatur der Session-Cookies |
| `ZUNDER_TIMING_TOKEN`, `ZUNDER_TIMING_PROJECT` | Timing-Web-API (optional) |
| `ZUNDER_BOOTSTRAP_TOKEN` | optionaler fester API-Token für Erstinbetriebnahme und Tests |
| `ZUNDER_DEV_AUTH` | `true` = Login ohne OIDC (nur lokal, Warnung im Log) |
| `ZUNDER_BACKUP_HOUR` | Stunde (lokal) für das nächtliche Backup, Default 3 |

Leere Werte gelten als nicht gesetzt (Compose und Portainer reichen optionale Variablen wie `ZUNDER_TIMING_TOKEN` als leeren String durch). „Lokal“ heißt: Zeitzone des Prozesses, im Container über `TZ` (z. B. `Europe/Berlin`).

**Persistenz:** SQLite im WAL-Modus. Tabellen mindestens `users`, `ops` (seq, op_id unique, entity, entity_id, field, value_json, hlc, source, received_at, owner_id), `entities` (materialisierter Zustand je Entität als JSON inkl. Feld-HLCs), `api_tokens`, `sessions`. Der Server hält den materialisierten Zustand zusätzlich im Speicher. Nächtliches Backup per `VACUUM INTO` nach `data/backups/`, 14 Stück aufbewahren.

**Container** (umgesetzt im `Dockerfile`, beschrieben in `docs/betrieb.md`):

- Image `ghcr.io/<owner>/zunder` für `linux/amd64` und `linux/arm64`. Binärdateien `/usr/local/bin/zunder-server` und `/usr/local/bin/zunder-import`, Web-UI unter `/app/web`, Daten unter `/data` (Volume), Nutzer `zunder` (UID/GID 10001), Port 8080. Im Image gesetzt: `ZUNDER_BIND=0.0.0.0:8080`, `ZUNDER_DATA_DIR=/data`, `ZUNDER_WEB_DIR=/app/web`.
- `tini` ist PID 1. `zunder-server` beendet sich bei `SIGTERM` (und `SIGINT`) sauber: keine neuen Verbindungen, laufende Anfragen abschließen, SQLite schließen.
- Healthcheck: `curl -fsS http://127.0.0.1:8080/healthz`. `/healthz` antwortet erst mit 200, wenn die Datenbank geöffnet ist.
- Keine OpenSSL-/native-tls-Abhängigkeit (rustls verwenden), damit der Cross-Build ohne Ziel-`libssl` auskommt. SQLite kommt gebündelt über `rusqlite`.
- Logs gehen nach stdout/stderr.

## 4. MCP

Streamable HTTP unter `/mcp`, JSON-RPC 2.0, Protokollversion `2025-06-18` (akzeptiert auch `2025-03-26`). Auth per Bearer-Token. Antworten als `application/json` (SSE-Antwort nicht nötig).

Werkzeuge (Namen und Schemas sind Vertrag, per Snapshot-Test eingefroren):

| Werkzeug | Zweck |
|---|---|
| `filter_tasks` | Filter: `location` (inbox/loose/area/project), `area_id`, `project_id`, `status[]`, `type[]`, `phoenix_role`, `phoenix_fire`, `due_before`, `due_after`, `include_closed`, `parent_id`, `claimed_by`, `limit` |
| `get_task` | `task_id` oder `clickup_id`; liefert Aufgabe mit Unteraufgaben-Baum und Fortschritt |
| `create_task` | Felder wie oben, `title` Pflicht; ohne Stelle → Inbox |
| `update_task` | `task_id`/`clickup_id` + zu ändernde Felder |
| `move_task` | neue Stelle und/oder neuer `parent_id` |
| `delete_task` | Tombstone |
| `search` | Volltext über Titel, Beschreibung, Plan, auch erledigte; Treffer über Aufgaben, Projekte, Bereiche |
| `claim_task` | `task_id`, `agent`, `ttl_minutes` (Default 120); schlägt fehl, wenn fremd und nicht abgelaufen |
| `release_task` | Reservierung aufheben |
| `decompose_task` | legt mehrere Unteraufgaben in einem Aufruf an |
| `list_structure` | Bereiche und Projekte (für Zuordnung und Vault-Abgleich Stufe 1) |
| `create_area`, `update_area`, `create_project`, `update_project` | Struktur pflegen (Vault-Abgleich Stufe 1) |
| `start_flame`, `stop_flame` | Timing-Brücke |
| `get_task_history` | Änderungsprotokoll |

Jedes Werkzeug, das eine Aufgabe adressiert, akzeptiert `task_id` **oder** `clickup_id`. Statusnamen im Werkzeug sind die englischen Codes. Werkzeugergebnisse enthalten `structuredContent` und einen kompakten JSON-Text in `content`.

**Offener Punkt – claude.ai:** Claude Code und andere Agenten senden den Bearer-Token als Header. Benutzerdefinierte Connectors in claude.ai bieten dagegen nur OAuth (MCP-Authorization-Spezifikation) oder gar keine Authentifizierung an, aber keinen frei setzbaren Header. Für claude.ai braucht `/mcp` deshalb zusätzlich einen OAuth-Weg (z. B. Pocket-ID als Authorization-Server oder ein schlanker eigener Authorization-Endpoint, der nach dem OIDC-Login einen API-Token ausstellt). Das ist noch nicht Teil dieses Vertrags und muss vor E3 entschieden werden (siehe `docs/betrieb.md`).

## 5. Web-UI

SolidJS + Vite + TypeScript (strict). Kein UI-Framework mit großem Laufzeitanteil; eigenes, schlankes CSS mit Variablen, hell/dunkel.

- **Sync-Client** spricht dasselbe Protokoll: Snapshot beim Start, dann `pull` (ausgelöst über SSE, Fallback Polling 15 s). Änderungen werden lokal sofort angewendet (optimistisch), in eine Warteschlange gelegt und gepusht. Abgelehnte Operationen werden lokal zurückgerollt und als Meldung angezeigt. Merge-Logik in TypeScript identisch zum Kern; gemeinsame Testvektoren in `crates/zunder-core/tests/vectors/*.json` werden von Rust- **und** TS-Tests geprüft.
- **Sichten:** Heute (Start), Inbox, Lose, Bereiche → Projekte (Liste/Board), Dev, Phönix, Suche (⌘K), plus Login- und Token-Seite.
- **Schnellerfassung:** Taste `n` bzw. großes ＋ mobil; Kurzschreibweise `Zahnarzt anrufen morgen 9:00 #gesundheit` (Datum: heute, morgen, übermorgen, Wochentage, `12.10.`, `12.10.2026`; Uhrzeit `9:00`/`9 Uhr` → `remind_at` und `due`; `#slug` → Bereich oder Projekt, Projekt gewinnt bei Gleichstand).
- **Aufgabendetail:** Seitenpanel, alles inline editierbar, Unteraufgaben-Baum, Änderungsprotokoll.
- **Tastatur:** `j/k` bewegen, `Enter` öffnen, `n` erfassen, `x` erledigt, `⌘K` Palette, in der Inbox `a` zuordnen, `l` lose, `d` verwerfen, `Esc` schließen, `g h/i/l/b/d/p` Sichtwechsel.
- **Mobil:** Tabs unten, Wisch-Gesten in der Inbox.
- Erledigte/verworfene Aufgaben verschwinden **3 Tage** nach `status_changed_at` aus den Standardansichten, bleiben in der Suche.
- Playwright-Abläufe: erfassen, Inbox abarbeiten, Board verschieben, Flamme starten; Tempo-Budget (Interaktion < 100 ms, Kaltstart < 1 s) wird gemessen.
- Mit `ZUNDER_E2E_BACKEND=real` startet die Playwright-Konfiguration (`webServer`) den echten Server: Binärdatei aus `ZUNDER_SERVER_BIN` (Default `../target/debug/zunder-server`), mit `ZUNDER_DEV_AUTH=true`, einem frischen temporären `ZUNDER_DATA_DIR` und `ZUNDER_WEB_DIR=dist` (vorher `npm run build`). Die CI baut den Server im Release-Profil und setzt `ZUNDER_SERVER_BIN` entsprechend.

## 6. Import

`zunder-import` (CLI, clap): `plan` (Probelauf → `mapping.toml`), `report` (Mengen, unbekannte Status, Aufgaben ohne Ziel), `run --mapping mapping.toml` (schreibt über `POST /api/sync/push` mit API-Token, Knoten `import`), `backup --out <dir>` (Roh-JSON inkl. Zeiteinträgen, Standard-Ziel außerhalb des Repos). Idempotent über `clickup_id` (erneuter Lauf aktualisiert statt dupliziert; IDs werden deterministisch aus der `clickup_id` abgeleitet: UUIDv5 im Zunder-Namespace oder Lookup).

Konfiguration (jeweils auch als gleichnamige Option, z. B. `--clickup-token`):

| Variable | Bedeutung |
|---|---|
| `CLICKUP_TOKEN` | persönliches ClickUp-API-Token (`pk_…`), für `plan`, `report`, `run` und `backup` |
| `CLICKUP_TEAM_ID` | ClickUp-Workspace; nur nötig, wenn das Token mehrere Workspaces sieht |
| `ZUNDER_URL` | Basis-URL der Zunder-Instanz, z. B. `https://zunder.rubeen.dev` (nur `run`) |
| `ZUNDER_TOKEN` | Zunder-API-Token `zt_…` (nur `run`) |

`plan` schreibt nach `--out` (Default `mapping.toml` im aktuellen Verzeichnis), `report` und `run` lesen `--mapping`. Das Runbook steht in `docs/umstieg.md`.

## 7. Etappen im Repo

E1–E2 werden vollständig gebaut. E3 ist überwiegend Betrieb (Runbook in `docs/umstieg.md`). E4 (Tauri) wird als lauffähiges Gerüst angelegt, das `zunder-core` für lokale SQLite und Sync nutzt; Mobile-Builds werden in dieser Umgebung nicht verifiziert.
