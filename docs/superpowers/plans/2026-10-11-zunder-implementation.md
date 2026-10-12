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
apps/zunder-app/           Tauri 2 (E4); src-tauri ist NICHT Workspace-Mitglied (braucht webkit),
                           sondern eigener Workspace mit zunder-local (ohne webkit testbar)
deploy/                    compose.yaml, .env.example (Vorlage), generate-secrets.sh (Compose/Traefik)
Dockerfile                 Multi-Stage: Web bauen, Server bauen, schlankes Runtime-Image
.github/workflows/ci.yml   fmt, clippy -D warnings, Tests, Web, Playwright, Image (PR: nur Build, main/Tags: GHCR)
docs/                      Spec, dieser Vertrag, betrieb.md, umstieg.md, entwicklung.md
```

`deploy/.env.example` ist eine eingecheckte Vorlage mit Platzhaltern; `deploy/generate-secrets.sh` schreibt das befüllte Ergebnis nach `deploy/.env` (von Git ignoriert), die `compose.yaml` per `env_file` lädt. Auf r-server liegt der Ordner unter `~/server/zunder/` und wird von Hand mit `docker compose` betrieben; nach jedem grünen Push auf `main` legt die CI ein GitHub-Deployment für den deploy-agent des Servers an (`payload.tag` = voller Commit-SHA, als Image-Tag veröffentlicht). Die Tauri-App wird in der CI nicht gebaut (Begründung als Kommentar in `ci.yml`).

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

**project**: `name` (Pflicht, nicht leer, ≤ 500 Zeichen), `area_id` (Pflicht, existierende Area), `status` (`active`|`paused`|`completed`, Default `active`), `kind` (`standard`|`dev`, Default `standard`), `repo` (`owner/name` im GitHub-Format, nur bei `dev`, optional; bei `standard` abgelehnt), `description` (Markdown), `view` (`list`|`board`, Default: `board` bei dev, sonst `list`), `key` (optional, Schlüssel für Aufgabenkennungen: Großbuchstabe, dann Großbuchstaben oder Ziffern, höchstens 10 Zeichen, z. B. `ZUN`; eindeutig unter den lebenden Projekten), `deleted`.

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
| `ref` | Kennung `SCHLÜSSEL-n` (z. B. `ZUN-42`, `n` ≥ 1 ohne führende Null, höchstens 9 Stellen); **schreibt nur der Server** (Abschnitt 3.1) |
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
- Wird eine Entität abgelehnt, wird der Push ohne sie erneut geprüft (z. B. wird eine Aufgabe in einem abgelehnten neuen Projekt ebenfalls abgelehnt). Entitäten, deren Referenz (`parent_id`, `location` der Wurzelaufgabe, `area_id`) auf eine abgelehnte Entität zeigt, die ohne den Push nicht (lebend) existiert, werden direkt mit `invalid_reference` abgelehnt, auch über beliebig tiefe Ketten hinweg; die Meldung nennt die Ursache der abgelehnten Entität. Erneut geprüft wird danach nur noch die Nachbarschaft der abgelehnten Entitäten.
- Eine ID gehört genau einer Entitätsart.
- **Kennungen:** Operationen auf `ref` aus einem Client-Push (auch `null`) werden mit `invalid_value` abgelehnt; nur der Server prüft mit `validate_push_as(…, Author::Server)`. Ein `key`, den schon ein anderes lebendes Projekt trägt, wird mit `invalid_value` abgelehnt (geprüft, wenn `key` gesetzt oder das Projekt wiederhergestellt wird).
- Bereits bekannte `op_id`s gelten als angenommen und werden nicht erneut angewendet. Eine Wiederholung derselben `op_id` innerhalb des Pushs teilt das Ergebnis ihres ersten Vorkommens: angenommen (aber nur einmal gespeichert), wenn dieses angenommen wird, sonst mit demselben Code und derselben Meldung abgelehnt. Eine `op_id` steht nie zugleich in `accepted` und `rejected`.
- Die Validierung arbeitet auf einer Copy-on-write-Sicht des Zustands, die nur die berührten Entitäten kopiert; ihr Aufwand hängt von der Größe des Pushs ab, nicht von der Größe der Datenbank.

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

Validierungsvektoren (`"kind": "validate"`) prüfen die clientseitige Spiegelung der Validierung gegen den Kern: `state` (Operationen, die vorher gemergt werden), `push` (zu prüfende Operationen), `now_ms` (physische Zeit) und `expected_rejected` (`{"<entity_id>": "<code>"}`: je abgelehnter Entität der Code der Ursache, also der erste Code außer `entity_rejected`; leer = alles angenommen). Rust prüft zusätzlich, dass genau die Operationen dieser Entitäten abgelehnt werden; die Web-UI (`validateOps`) muss je Entität dieselbe Ursache melden. Fehlt das Verzeichnis, schlagen die TypeScript-Tests fehl (kein Überspringen).

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
| `GET /api/tasks/{id}/history` | Änderungsprotokoll: `[{"seq","op","source","received_at"}]` |
| `POST /api/tasks/{id}/flame/start` | Flamme starten (Timing-Brücke, Fallback-Zeitstempel), Details in 4.2 |
| `POST /api/tasks/{id}/flame/stop` | Flamme beenden (409, wenn keine läuft) |
| `GET /api/tokens` / `POST /api/tokens` / `DELETE /api/tokens/{id}` | API-Tokens verwalten (nur Session-Auth): Liste `[{"id","name","created_at","last_used_at"}]`; Anlegen mit `{"name"}` → `201` `{"id","name","created_at","last_used_at","token"}` (Token nur in dieser Antwort); Widerruf → `204` |
| `GET /api/oauth/clients` / `DELETE /api/oauth/clients/{id}` | per OAuth verbundene Apps (nur Session-Auth): Liste `[{"id","name","redirect_uris","created_at","last_used_at"}]`; Trennen → `204` (Details 4.3) |
| `GET` / `PUT` / `DELETE /api/settings/timing`, `POST /api/settings/timing/check` | Timing-Einstellungen (nur Session-Auth), Details in 4.2 |
| `GET /api/devices` / `DELETE /api/devices/{id}` | in der Zunder-App angemeldete Geräte (nur Session-Auth): Liste `[{"id","name","created_at","last_used_at"}]`; Abmelden → `204`, unbekannt → `404` (Details 4.3, „Geräte“) |
| `POST /mcp` (+ `GET`, `DELETE`) | MCP Streamable HTTP |
| `GET /.well-known/oauth-protected-resource` (auch `…/mcp`), `GET /.well-known/oauth-authorization-server` | OAuth-Metadaten für `/mcp`, ohne Auth (4.3) |
| `POST /oauth/register`, `GET`/`POST /oauth/authorize`, `POST /oauth/token`, `POST /oauth/revoke` | eingebauter OAuth-2.1-Authorization-Server für `/mcp` (4.3) |

- Idempotenz: Eine bereits bekannte `op_id` gilt als `accepted`, wird aber nicht erneut gespeichert.
- `seq` ist ein monotoner Server-Zähler; der Cursor ist das größte vergebene `seq`.
- Operationen, die der Server selbst erzeugt (MCP, Timing), und alle gepushten Operationen laufen durch dieselbe Validierung und dasselbe Log und tragen eine `source`: `web` (Browser-Sitzung), `api:<token-name>` (Sync-API mit API-Token, z. B. `api:import`, wenn der Import einen Token namens `import` nutzt; `api:bootstrap` für `ZUNDER_BOOTSTRAP_TOKEN`), `mcp:<token-name>` (`mcp:oauth:<client_name>` für per OAuth verbundene Apps, Abschnitt 4.3), `app:<gerätename>` (Sync-API mit Geräte-Token der Zunder-App, 4.3), `server`.

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
| `ZUNDER_TIMING_TOKEN`, `ZUNDER_TIMING_PROJECT` | Timing-Web-API (optional; Werte aus der Einstellungsseite haben Vorrang, 4.2) |
| `ZUNDER_BOOTSTRAP_TOKEN` | optionaler fester API-Token für Erstinbetriebnahme und Tests |
| `ZUNDER_DEV_AUTH` | `true` = Login ohne OIDC (nur lokal, Warnung im Log) |
| `ZUNDER_DEV_AUTH_ALLOW_REMOTE` | `true` = Dev-Auth auch mit nicht lokaler `ZUNDER_BIND` (lokaler Container) |
| `ZUNDER_BACKUP_HOUR` | Stunde (lokal) für das nächtliche Backup, Default 3 |

Leere Werte gelten als nicht gesetzt (Compose reicht optionale Variablen wie `ZUNDER_TIMING_TOKEN` als leeren String durch). „Lokal“ heißt: Zeitzone des Prozesses, im Container über `TZ` (z. B. `Europe/Berlin`).

**Persistenz:** SQLite im WAL-Modus. Tabellen mindestens `users`, `ops` (seq, op_id unique, entity, entity_id, field, value_json, hlc, source, received_at, owner_id), `entities` (materialisierter Zustand je Entität als JSON inkl. Feld-HLCs), `api_tokens`, `sessions`, ab Schema 2 `oauth_clients`, `oauth_codes`, `oauth_tokens` (4.3), ab Schema 3 `settings` (`key`, `value`, `updated_at`; Einstellungen aus der Web-UI, 4.2), ab Schema 4 `devices` (4.3). Der Server hält den materialisierten Zustand zusätzlich im Speicher. Nächtliches Backup per `VACUUM INTO` nach `data/backups/`, 14 Stück aufbewahren.

### 3.1 Präzisierungen (umgesetzt in `zunder-server`)

**Fehler.** Format `{"error":{"code","message"}}` mit `code` ∈ `unauthorized` (401, mit `WWW-Authenticate: Bearer`; bei `/mcp` mit `resource_metadata`, siehe 4.3), `forbidden` (403), `not_found` (404), `invalid_request` (400), `conflict` (409, z. B. Token-Name doppelt), `payload_too_large` (413), `unavailable` (503), `internal` (500). Unbekannte Pfade unter `/api/` liefern `not_found` als JSON.

**Push.** `client_id` ist optional (≤ 128 Zeichen) und wird je Operation gespeichert. Ist der Body kein gültiges JSON oder hat eine Operation nicht das Wire-Format (z. B. keine kanonische UUID, ungültiger HLC, fehlendes `value`), wird der ganze Push mit `400 invalid_request` abgelehnt; inhaltliche Fehler meldet der Kern je Operation in `rejected`. Ein Push ohne neue Operationen ändert den Cursor nicht und löst kein SSE-Ereignis aus. Body-Limit: 8 MiB für `POST /api/sync/push`, 64 KiB für alle anderen Anfragen.

**Pull.** `since` Default 0, nicht negativ; `limit` Default 1000, höchstens 5000 (größere Werte werden gekappt), mindestens 1. `cursor` ist das `seq` der letzten gelieferten Operation; ohne Operationen der aktuelle Server-Cursor. `has_more` heißt: direkt mit `since=cursor` weiterholen.

**SSE.** `GET /api/sync/events` sendet sofort `event: cursor` mit dem aktuellen Cursor und danach bei jeder Änderung; Keep-Alive-Kommentare alle 15 s. SSE-Antworten werden nie komprimiert. Beim Herunterfahren endet der Stream.

**Snapshot.** Enthält auch gelöschte Entitäten (Tombstones), damit Clients sie mergen können. Kopieren, Serialisieren und Komprimieren (Brotli vor gzip, je nach `Accept-Encoding`) laufen im Blocking-Pool, nicht auf den async-Workern; ebenso die vollständigen Durchläufe der MCP-Lesewerkzeuge (`filter_tasks`, `search`, `list_structure`).

**Verlauf.** `GET /api/tasks/{id}/history` liefert alle Operationen der Aufgabe in `seq`-Reihenfolge, `received_at` als RFC 3339 (UTC). Unbekannte Aufgabe → 404, keine kanonische ID → 400.

**Besitzer.** Eine Instanz hat genau einen Besitzer. Beim ersten Start legt der Server die Zeile in `users` an (nötig für `owner_id` an Operationen, auch vor dem ersten Login, z. B. für einen Import per Bootstrap-Token). Der erste erfolgreiche OIDC-Login hinterlegt dort Subject, E-Mail und Namen, spätere Logins aktualisieren E-Mail und Namen. Alle Zugangswege (Sitzung, Token, Bootstrap-Token, Dev-Auth) handeln als dieser Besitzer.

**Sitzungen.** Cookie `zunder_session` mit 32 Zufallsbytes (base64url), in der DB nur als HMAC-SHA256 mit `ZUNDER_SESSION_SECRET` (mindestens 32 Zeichen; Pflicht, sobald OIDC konfiguriert und Dev-Auth aus ist). `HttpOnly`, `SameSite=Lax`, `Path=/`, `Secure` außer bei `http://localhost`/`127.0.0.1`/`[::1]`. Gültigkeit 30 Tage, gleitend: Ist die letzte Verlängerung älter als ein Tag, verlängert der Server sie und sendet das Cookie neu. `POST /auth/logout` löscht die Sitzung (→ 204).

**CSRF.** Mit Cookie (oder Dev-Auth) authentifizierte Anfragen, die Zustand ändern (alles außer `GET`/`HEAD`/`OPTIONS`, auch `POST /auth/logout`), brauchen den Header **`X-Zunder-Client: web`**; sonst 403 `forbidden`. Die Web-UI sendet ihn bei jeder API-Anfrage. Bearer-Token-Anfragen brauchen ihn nicht. Der Server erlaubt kein CORS; einzige Ausnahme sind die öffentlichen OAuth-Metadaten unter `/.well-known/` (`Access-Control-Allow-Origin: *`, nur lesend).

**API-Tokens.** Format `zt_` + 32 Zufallsbytes base64url (43 Zeichen), gespeichert als SHA-256 (hex). Namen 1–64 Zeichen, ohne Steuerzeichen, eindeutig unter den aktiven Tokens, `bootstrap` ist reserviert. Widerrufen = `revoked_at` setzen (der Name bleibt im Verlauf nachvollziehbar). `last_used_at` wird höchstens einmal pro Minute geschrieben. Ein ungültiger Bearer-Token ist immer 401, auch wenn zusätzlich ein Cookie mitkommt. `ZUNDER_BOOTSTRAP_TOKEN` gilt zusätzlich als Token mit dem Namen `bootstrap`.

**OIDC.** `GET /auth/login?return_to=/pfad` (nur lokale Pfade bis 2048 Zeichen ohne Steuer- und Leerzeichen, sonst `/`) leitet mit `state`, `nonce` und PKCE (S256) zum Anbieter; Scope `openid email profile`. Der laufende Login (`state`, `nonce`, PKCE-Verifier, Rückweg, Ablauf nach 10 Minuten) liegt nicht auf dem Server, sondern verschlüsselt (ChaCha20-Poly1305, Schlüssel aus `ZUNDER_SESSION_SECRET` abgeleitet) im Cookie `zunder_login` (Pfad `/auth`), das ihn zugleich an den Browser bindet; anonyme Aufrufe von `/auth/login` können so keinen laufenden Login verdrängen. Der Callback verlangt, dass `state` zum Cookie passt (sonst 400), und merkt sich abgeschlossene Logins bis zu ihrem Ablauf (kein zweites Einlösen). Der Callback tauscht den Code (`client_secret_post`, ohne Secret als öffentlicher Client), prüft das ID-Token (Signatur per JWKS, keine HS-Algorithmen, `iss`, `aud`, bei mehreren Audiences `azp`, `exp`/`iat` mit 60 s Toleranz, `nonce`) und setzt die Sitzung. Unbekannte Schlüssel-IDs lösen ein erneutes Laden der JWKS aus. Erlaubt ist: das Subject aus `ZUNDER_OIDC_ALLOWED_SUBJECT`, falls gesetzt, sonst die E-Mail aus `ZUNDER_OIDC_ALLOWED_EMAIL` (Groß-/Kleinschreibung egal, `email_verified` darf nicht `false` sein). Fehler zeigt eine deutsche HTML-Seite. Ohne konfigurierten Anbieter antwortet `/auth/login` mit 503.

**Dev-Auth.** `ZUNDER_DEV_AUTH=true` ist nur mit lokaler `ZUNDER_PUBLIC_URL` erlaubt (sonst startet der Server nicht) und wird beim Start laut geloggt. `ZUNDER_BIND` ist dann standardmäßig `127.0.0.1:8080`; eine nicht lokale Bind-Adresse verweigert den Start, außer mit `ZUNDER_DEV_AUTH_ALLOW_REMOTE=true` (für einen Container, dessen Port nur lokal veröffentlicht wird). Dev-Auth greift nur für Anfragen, deren `Host` ein Loopback-Name ist (`localhost`, `127.0.0.0/8`, `[::1]`, beliebiger Port); sonst 401 bzw. 403 bei `/auth/login` (Schutz gegen DNS-Rebinding). Anfragen ohne Zugangsdaten gelten dann als der Besitzer (Name „Entwicklung“, E-Mail `dev@localhost`, solange kein OIDC-Login erfolgt ist); `/auth/login` legt direkt eine Sitzung an und leitet zurück. Die CSRF-Regel gilt auch hier.

**Weitere Variablen.** `ZUNDER_PUBLIC_URL` Default `http://localhost:<Port aus ZUNDER_BIND>`; OIDC-Redirect-URI ist `<ZUNDER_PUBLIC_URL>/auth/callback`. `ZUNDER_OIDC_CLIENT_SECRET` ist optional. `RUST_LOG` steuert das Log (Default `info`). Das Backup läuft zur lokalen Stunde `ZUNDER_BACKUP_HOUR` (Zeitzone über `TZ`), Dateiname `zunder-JJJJMMTT-HHMMSS.db`; die Kopie entsteht unter einem temporären Namen (`.zunder-….db.tmp`) und wird erst vollständig umbenannt, ein Fehlschlag hinterlässt keine Datei. Dabei werden abgelaufene Sitzungen entfernt, auch wenn das Backup fehlschlägt.

**Gesundheitsprüfung.** `zunder-server healthcheck` fragt `http://127.0.0.1:<Port aus ZUNDER_BIND>/healthz` ab (bzw. die gebundene IP, wenn nicht `0.0.0.0`/`::`) und endet mit Exit-Code 0 bei `{"status":"ok"}`, sonst 1. Gedacht für `HEALTHCHECK` im Dockerfile. `/healthz` prüft die Datenbank und antwortet bei Fehlern mit 503 `{"status":"error"}`.

**Statische Dateien.** `ZUNDER_WEB_DIR` wird unter `/` ausgeliefert. `/assets/*` (Vite-Ausgabe mit Hash im Namen) mit `Cache-Control: public, max-age=31536000, immutable`, alles andere mit `no-cache`. Unbekannte Pfade ohne Dateiendung liefern `index.html` (SPA-Fallback); fehlende Dateien unter `/assets/` oder mit Endung sind 404. Antworten werden mit gzip oder Brotli komprimiert, wenn der Client es anbietet.

**Kennungen.** Nach jedem gespeicherten Push und einmal beim Start vergibt der Server unter der Schreibsperre `ref` an jede lebende Aufgabe ohne Kennung, deren wirksame Stelle ein Projekt mit `key` ist (`zunder_core::numbering::pending_refs`): je Schlüssel in Erstellreihenfolge, fortlaufend ab der größten je unter diesem Schlüssel vergebenen Nummer (gelöschte Aufgaben zählen mit, Nummern werden nie wiederverwendet). Die Operationen laufen als eigener Push mit Quelle `server` durch Validierung, Log und SSE; Clients holen sie per `pull`. Eine Kennung ändert sich danach nie: Verschiebt man die Aufgabe in ein anderes Projekt oder ändert den Schlüssel, bleibt sie; neue Aufgaben zählen unter dem neuen Schlüssel. Offline erfasste Aufgaben bekommen ihre Kennung beim nächsten Abgleich.

**Schema.** Migrationen sind eingebettetes SQL, versioniert über `PRAGMA user_version`; ein Server verweigert eine Datenbank mit neuerer Version. Zeitstempel sind Unix-Millisekunden. `meta.materialized_seq:<owner_id>` hält fest, bis zu welchem `seq` die Tabelle `entities` aktuell ist; stimmt das (oder die Anzahl der Entitäten) beim Start nicht mit dem Log überein, baut der Server `entities` aus dem Log neu auf.

### 3.2 Container

**Container** (umgesetzt im `Dockerfile`, beschrieben in `docs/betrieb.md`):

- Image `ghcr.io/<owner>/zunder` für `linux/amd64` und `linux/arm64`. Binärdateien `/usr/local/bin/zunder-server` und `/usr/local/bin/zunder-import`, Web-UI unter `/app/web`, Daten unter `/data` (Volume), Nutzer `zunder` (UID/GID 10001), Port 8080. Im Image gesetzt: `ZUNDER_BIND=0.0.0.0:8080`, `ZUNDER_DATA_DIR=/data`, `ZUNDER_WEB_DIR=/app/web`, `TZ=Europe/Berlin`.
- `tini` ist PID 1. `zunder-server` beendet sich bei `SIGTERM` (und `SIGINT`) sauber: keine neuen Verbindungen, laufende Anfragen höchstens 8 s abschließen lassen (danach werden offene Verbindungen getrennt, unter Dockers Stopp-Frist von 10 s), SQLite schließen. Ein Request-Body, der 15 s lang nichts sendet, bricht ab.
- Healthcheck: `zunder-server healthcheck` (kein `curl` im Image; folgt `ZUNDER_BIND`), alle 30 s, in der 20-s-Startphase alle 2 s (`--start-interval`), damit Traefik den Container nach einem Neustart sofort wieder anspricht. `/healthz` antwortet erst mit 200, wenn die Datenbank geöffnet ist.
- `tzdata` ist installiert, `TZ=Europe/Berlin` ist im Image gesetzt (überschreibbar) und bestimmt die Stunde des nächtlichen Backups.
- Keine OpenSSL-/native-tls-Abhängigkeit (rustls verwenden), damit der Cross-Build ohne Ziel-`libssl` auskommt. SQLite kommt gebündelt über `rusqlite`.
- Logs gehen nach stdout/stderr.

## 4. MCP

Streamable HTTP unter `/mcp`, JSON-RPC 2.0, Protokollversion `2025-06-18` (akzeptiert auch `2025-03-26`). Auth per Bearer-Token: API-Token oder OAuth-Access-Token (4.3). Antworten als `application/json` (SSE-Antwort nicht nötig).

Werkzeuge (Namen und Schemas sind Vertrag, per Snapshot-Test eingefroren):

| Werkzeug | Zweck |
|---|---|
| `filter_tasks` | Filter: `location` (inbox/loose/area/project), `area_id`, `project_id`, `status[]`, `type[]`, `phoenix_role`, `phoenix_fire`, `due_before`, `due_after`, `closed_after`, `closed_before`, `include_closed`, `parent_id`, `claimed_by`, `flame_running`; `order_by`, `reverse`, `offset`, `limit` |
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
| `start_flame`, `stop_flame` | Timing-Brücke; `stop_flame` ohne Aufgabe beendet die laufende Flamme |
| `get_task_history` | Änderungsprotokoll |

Jedes Werkzeug, das eine Aufgabe adressiert, akzeptiert `task_id` (UUID oder Kennung wie `ZUN-42`, Groß-/Kleinschreibung egal) **oder** `clickup_id`. Statusnamen im Werkzeug sind die englischen Codes. Werkzeugergebnisse enthalten `structuredContent` und einen kompakten JSON-Text in `content`.

### 4.1 Präzisierungen (umgesetzt in `zunder-server`)

**Transport.** `POST /mcp` nimmt eine JSON-RPC-Nachricht oder einen Batch (Array) an. Anfragen → `200 application/json` (Batch → Array der Antworten); nur Benachrichtigungen bzw. Client-Antworten → `202` ohne Body. Kein serverseitiger Stream: `GET /mcp` und `DELETE /mcp` → `405`. Zustandslos: Der Server vergibt keine `Mcp-Session-Id`, eine mitgeschickte wird ignoriert. Ein vorhandener Header `MCP-Protocol-Version` muss `2025-06-18` oder `2025-03-26` sein (sonst `400`). `initialize` antwortet mit der angefragten Version, falls unterstützt, sonst mit `2025-06-18`; dazu `serverInfo {name:"zunder", title, version}`, `capabilities {tools:{listChanged:false}}` und deutsche `instructions` (Datenmodell, Status, Stellen). Methoden: `initialize`, `ping`, `tools/list` (ohne Paging), `tools/call`; Benachrichtigungen (`notifications/initialized` u. a.) werden angenommen. Body-Limit 1 MiB.

**Auth und Herkunft.** Nur Bearer-Token: API-Token, `ZUNDER_BOOTSTRAP_TOKEN` oder OAuth-Access-Token `zoa_…` (4.3). Session-Cookie und Dev-Auth gelten für `/mcp` nicht. Ohne oder mit ungültigem Token → `401` mit `WWW-Authenticate: Bearer resource_metadata="<ZUNDER_PUBLIC_URL>/.well-known/oauth-protected-resource"` (bei vorhandenem, aber abgelehntem Token zusätzlich `, error="invalid_token"`). Ein vorhandener `Origin`-Header muss dem Origin von `ZUNDER_PUBLIC_URL` entsprechen (→ sonst `403`). Operationen tragen die Quelle `mcp:<token-name>` bzw. `mcp:oauth:<client_name>`.

**Fehler.** Protokollfehler sind JSON-RPC-Fehler: `-32700` (kein JSON, HTTP 400), `-32600` (keine gültige Nachricht, leerer Batch, `id` weder Text noch Zahl), `-32601` (unbekannte Methode), `-32602` (`params` kein Objekt, `name` fehlt, `arguments` kein Objekt, unbekanntes Werkzeug). Alle anderen Fehler eines Werkzeugs sind Ergebnisse mit `isError: true`, deutschem Text in `content` und `structuredContent: {"error":{"code","message"}}`. `code` ist `invalid_arguments` (unbekannter/fehlender Parameter, falscher Typ, auch fehlende oder doppelte Aufgabenadressierung), `not_found`, `conflict` (fremde Reservierung, ID vergeben, keine laufende Flamme), ein Ablehnungscode des Kerns (Abschnitt 2.5, mit dessen deutscher Meldung) oder `internal`.

**Änderungen sind atomar.** Jeder Werkzeugaufruf erzeugt einen Push mit der Server-Uhr (Knoten `server`), der unter der Schreibsperre vorab validiert wird: Wird irgendeine Operation abgelehnt, wird nichts gespeichert (z. B. `decompose_task` legt alle oder keine Unteraufgaben an). Warnungen (`open_subtasks`) stehen im Ergebnis unter `warnings`. Werte, die der Kern prüft (Datum, Stelle, Links, Phönix …), werden unverändert durchgereicht, damit seine Meldungen ankommen. `update_*` schreibt nur Felder, deren Wert sich tatsächlich ändert.

**Darstellung einer Aufgabe** (`task` in Ergebnissen): `id`, `ref`, `title`, `description`, `type`, `status`, `location` (wirksam, mit aufgelösten Namen: `{"kind":"project","id","name","area_id","area_name"}`, `{"kind":"area","id","name"}`, `{"kind":"inbox"}`, `{"kind":"loose"}`), `parent_id`, `due`, `remind_at`, `links`, `vault_links`, `phoenix`, `claimed_by`, `claim_expires_at`, `claim_active`, `plan`, `branch`, `pr_url`, `timing_ref`, `flame_started_at`, `flame_ended_at`, `flame_running`, `clickup_id`, `created_at`, `updated_at`, `status_changed_at` (RFC 3339 UTC aus den HLCs), `progress {done,total}`; bei `get_task` und `decompose_task` zusätzlich `subtasks` (Baum, je Knoten wieder mit `subtasks`).

**Werkzeuge im Detail** (Schemas: `crates/zunder-server/tests/snapshots/mcp_tools.json`):

- Adressierung: genau eines von `task_id` (UUID oder Kennung, z. B. `zun-42`) oder `clickup_id`. Eine Kennung wird unter allen Aufgaben gesucht (unbekannt → `not_found`); ein Text, der weder UUID noch Kennung ist, ist `invalid_arguments`. Die `clickup_id` wird unter den lebenden Aufgaben gesucht, sonst über die deterministische Import-ID. Gelöschte Aufgaben gelten als nicht gefunden, außer bei `get_task_history`. Fehlt beides oder kommt beides, ist der Code `invalid_arguments`. Verweise auf eine andere Aufgabe (`parent_id` in `filter_tasks`, `create_task`, `move_task`) nehmen eine UUID, eine Kennung oder die ClickUp-ID einer importierten Aufgabe (jeder Text, der weder kanonische UUID noch Kennung ist); die ClickUp-ID wird genauso aufgelöst (unbekannt → `not_found`, bei `filter_tasks` einfach keine Treffer), eine UUID prüft weiter der Kern.
- `filter_tasks`: alle Filter mit UND; `area_id` = direkt im Bereich oder in einem seiner Projekte; `location` = Art der wirksamen Stelle; `due_before`/`due_after` inklusive, Vergleich nach Kalendertag, Aufgaben ohne `due` fallen dann heraus; `claimed_by` nur gültige Reservierungen; ohne `include_closed` gilt die 3-Tage-Regel; Unteraufgaben erscheinen wie andere Aufgaben (mit `parent_id`). `closed_after`/`closed_before` (inklusive) treffen nur erledigte/verworfene Aufgaben nach `status_changed_at`, ein Datum als Kalendertag in der Zeitzone des Servers (`TZ`), ein RFC-3339-Zeitpunkt exakt; sie schließen `include_closed` ein. `flame_running: true` findet Aufgaben mit laufender Flamme unabhängig von Status und Alter, `false` nur solche ohne. `order_by` ∈ `due` (Default: `due` aufsteigend, ohne zuletzt, dann `created_at`), `created`, `updated`, `status_changed` (jeweils aufsteigend), Gleichstand nach `id`; `reverse: true` kehrt die ganze Reihenfolge um. `offset` (Default 0, höchstens 1 000 000) überspringt so viele Treffer, `limit` Default 100, höchstens 500. Ergebnis `{tasks, count, total, offset, truncated, next_offset}`; `truncated` = es gibt Treffer hinter dieser Seite, `next_offset` ist dann der `offset` der nächsten Seite, sonst `null`.
- `create_task`: `title` Pflicht, dazu `description`, `type`, `status`, `location` (Default inbox), `parent_id`, `due`, `remind_at`, `links`, `vault_links`, `phoenix`, `plan`, `branch`, `pr_url`. `update_task`: dieselben Felder außer `location`/`parent_id`; `null` setzt zurück. Ergebnis `{task, warnings}`.
- `move_task`: nur `location` → an diese Stelle, von einer Elternaufgabe gelöst; `parent_id` (Text) → Unteraufgabe (eine zusätzlich angegebene `location` wird als eigene Stelle gespeichert); `parent_id: null` → eigenständig, behält ohne `location` die bisherige wirksame Stelle.
- `delete_task`: löscht samt Unteraufgaben (`OpBatch::delete_cascade`), Ergebnis `{deleted:[id], count}`.
- `search`: `query` Pflicht, `limit` Default 20 (höchstens 200); Treffer `{entity, id, title, score}`, bei Aufgaben zusätzlich `status`, `type`, `location`, `parent_id`, `due`, `clickup_id`, `ref`, bei Projekten `area_id`, `area_name`, `status`, `kind`, `key`, bei Bereichen `state`. Eine exakte ClickUp-ID oder Kennung (Groß-/Kleinschreibung egal) steht mit Punktzahl 1000 vorn.
- `claim_task`: `agent` Pflicht (≤ 200 Zeichen), `ttl_minutes` 1–10080, Default 120. Atomar unter der Schreibsperre: Fehler `conflict`, wenn ein anderer Agent eine nicht abgelaufene Reservierung hält; derselbe Agent verlängert. `release_task`: `agent` (Pflicht ohne `force`), `force`; fremde, gültige Reservierung nur mit `force`; Ergebnis mit `released` (false, wenn nichts reserviert war).
- `decompose_task`: `subtasks` 1–50 Einträge mit `title` (Pflicht), `description`, `type` (Default `aufgabe`), `status`, `due`, `remind_at`, `plan`; ein Push, alles oder nichts. Ergebnis `{task (mit Baum), created:[id], warnings}`.
- `list_structure`: `include_archived` (archivierte Bereiche, abgeschlossene Projekte); Ergebnis `{areas:[{…Bereich, open_tasks, projects:[{…Projekt, open_tasks}]}], inbox_open, loose_open}`, nach Namen sortiert.
- `create_area` (`name`, `state`, `vault_path`, optional `id` = vorhandene `zunder_id`; vergebene ID → `conflict`), `update_area` (`area_id`, `name`, `state`, `vault_path`), `create_project` (`name`, `area_id`, `status`, `kind`, `repo`, `description`, `view`, `key`, optional `id`), `update_project` (`project_id` + Felder; `repo`/`description`/`view`/`key` mit `null` zurücksetzbar). Projekte in Ergebnissen tragen `key`.
- `get_task_history`: `limit` Default 100 (höchstens 1000, die neuesten); Einträge `{seq, field, value, hlc, source, received_at}`.

### 4.2 Flammen und Timing-Brücke

- `POST /api/tasks/{id}/flame/start|stop` (Session mit `X-Zunder-Client` oder Bearer-Token) und die Werkzeuge `start_flame`/`stop_flame` nutzen dieselbe Funktion (`zunder_server::flame`). Antwort: `{task, running, stopped_task_ids, unchanged, timing:{configured, used, entry, error}, warnings}`.
- Start schreibt immer `flame_started_at` (jetzt) und setzt `flame_ended_at` zurück; Stopp schreibt `flame_ended_at`. Eine Flamme läuft, solange `flame_started_at` gesetzt und `flame_ended_at` leer oder älter ist. Es läuft höchstens eine Flamme: Start beendet alle anderen laufenden Flammen (`stopped_task_ids`). Start einer schon laufenden Flamme ändert nichts (`unchanged: true`). Stopp ohne laufende Flamme → `409 conflict`.
- Mit Timing-Token (Einstellungen oder `ZUNDER_TIMING_TOKEN`): Start ruft `POST https://web.timingapp.com/api/v1/time-entries/start` mit `{"title": <Aufgabentitel>, "project": <Timing-Projekt> (falls gesetzt; Self-Link wie /projects/1 oder Titel), "replace_existing": false}` und speichert `data.self` (z. B. `/time-entries/2`) als `timing_ref` (Timing beendet einen laufenden Timer beim Start selbst; `replace_existing: false`, damit nie fremde Einträge gekürzt oder gelöscht werden). Stopp fragt `GET /time-entries/running` und ruft `PUT /time-entries/stop` nur, wenn der laufende Timer der eigene ist (ein in Timing inzwischen gestarteter fremder Timer bleibt unangetastet). `timing_ref` bleibt nach dem Stopp als Verweis erhalten.
- Scheitert Timing (nicht erreichbar, Fehlerstatus), läuft die Flamme trotzdem mit Zeitstempeln: `timing.used: false`, `timing.error` mit deutscher Meldung, `timing_ref` leer. Ohne Token: `timing.configured: false`.
- Flammen-Operationen tragen die Quelle `server` (Timing-Brücke), unabhängig davon, ob HTTP oder MCP sie ausgelöst hat.
- **Einstellungen:** Token und Projekt lassen sich in der Web-UI (`#/einstellungen`) setzen. Sie liegen in der Tabelle `settings` (`timing.token`, `timing.project`) und überschreiben die Umgebungsvariablen Feld für Feld; ein leeres gespeichertes Projekt heißt „ohne Projekt“, auch wenn `ZUNDER_TIMING_PROJECT` eines nennt. Änderungen gelten ab der nächsten Flamme, ohne Neustart. Der Token liegt im Klartext in der Datenbank (und ihren Backups), weil Timing ihn so braucht; die API gibt ihn nie zurück. Alle Endpunkte nur mit Browser-Sitzung (sonst `403`), schreibende mit `X-Zunder-Client`:
  - `GET /api/settings/timing` → `{"configured", "token_source", "token_hint", "project", "project_source", "overridden", "env_token", "env_project"}`; `*_source` ist `"settings"`, `"env"` oder `null`, `token_hint` die letzten 4 Zeichen mit `…` davor (bei Tokens unter 12 Zeichen nur `…`), `overridden` sagt, ob gespeicherte Werte gelten, `env_token`/`env_project` zeigen, was nach dem Zurücksetzen gilt.
  - `PUT /api/settings/timing` mit `{"token"?, "project"?}` (unbekannte Felder → `400`): fehlender oder leerer `token` behält den gespeicherten, fehlendes `project` behält das Projekt, `""` entfernt es. Werte werden getrimmt; Token höchstens 2048 Zeichen ohne Leer- und Steuerzeichen, Projekt höchstens 500 Zeichen ohne Steuerzeichen (sonst `400`). Antwort wie `GET`.
  - `DELETE /api/settings/timing` entfernt die gespeicherten Werte (danach gelten wieder die Umgebungsvariablen); Antwort wie `GET`.
  - `POST /api/settings/timing/check` fragt `GET /time-entries/running` mit dem wirksamen Token → `{"ok": true, "error": null}` oder `{"ok": false, "error": <deutsche Meldung>}`; ohne Token `409 conflict`.

### 4.3 OAuth für claude.ai (MCP-Authorization)

Benutzerdefinierte Connectors in claude.ai können keinen festen Header senden, sondern nur OAuth nach der MCP-Authorization-Spezifikation (Stand `2025-06-18`, kompatibel zu `2025-11-25`). Zunder ist dafür selbst der Authorization-Server; die Anmeldung der Person läuft über die bestehende Browser-Sitzung (Pocket-ID-Login, lokal Dev-Auth). API-Tokens funktionieren unverändert weiter (Claude Code, eigene Agenten). Umgesetzt in `zunder_server::oauth`.

- **Bezeichner:** Issuer = `ZUNDER_PUBLIC_URL` ohne Schrägstrich am Ende (z. B. `https://zunder.rubeen.dev`), geschützte Ressource = `<Issuer>/mcp`.
- **Protected Resource Metadata (RFC 9728):** `GET /.well-known/oauth-protected-resource` und `GET /.well-known/oauth-protected-resource/mcp` → `{"resource":"<Issuer>/mcp","authorization_servers":["<Issuer>"],"bearer_methods_supported":["header"],"resource_name":"Zunder"}`.
- **Authorization Server Metadata (RFC 8414):** `GET /.well-known/oauth-authorization-server` → `issuer`, `authorization_endpoint` (`/oauth/authorize`), `token_endpoint` (`/oauth/token`), `registration_endpoint` (`/oauth/register`), `revocation_endpoint` (`/oauth/revoke`), `response_types_supported: ["code"]`, `response_modes_supported: ["query"]`, `grant_types_supported: ["authorization_code","refresh_token"]`, `code_challenge_methods_supported: ["S256"]`, `token_endpoint_auth_methods_supported: ["none"]`, `revocation_endpoint_auth_methods_supported: ["none"]`, `authorization_response_iss_parameter_supported: true`. Keine `scopes_supported` (es gibt nur einen Zugriffsumfang: alle MCP-Werkzeuge). Kein Client-ID-Metadata-Document: claude.ai fällt dann auf die dynamische Registrierung zurück. Beide Metadaten-Dokumente mit `Access-Control-Allow-Origin: *` und `Cache-Control: max-age=300`.
- **Registrierung (RFC 7591):** `POST /oauth/register` mit JSON (Body ≤ 16 KiB, wie alle `/oauth/*`-Anfragen). `redirect_uris` Pflicht, 1–10 Einträge, je ≤ 2000 Zeichen, ohne Fragment und Zugangsdaten, `https` mit Host, `http` auf `localhost`/`127.0.0.1`/`[::1]` oder ein App-Schema in umgekehrter Domain-Schreibweise mit Punkt (RFC 8252, 7.1; etwa `dev.rubeen.zunder:/oauth` für die Zunder-App). `client_name` optional (getrimmt, ≤ 100 Zeichen, keine Steuerzeichen; fehlt er: „Unbenannter Client“). `grant_types` (falls angegeben) muss `authorization_code` enthalten und darf sonst nur `refresh_token` nennen, `response_types` nur `code`. Jeder Client wird als öffentlicher Client registriert, auch wenn er eine andere `token_endpoint_auth_method` wünscht; kein `client_secret`. Antwort `201` `{"client_id","client_id_issued_at","client_name","redirect_uris","grant_types","response_types","token_endpoint_auth_method":"none"}`. Fehler im OAuth-Format `{"error","error_description"}`: `invalid_redirect_uri`, `invalid_client_metadata`. Grenzen: höchstens 10 erfolgreiche Registrierungen pro Stunde je Quelladresse (IPv6: je /64) und 500 Clients insgesamt (sonst `429` `temporarily_unavailable`). Quelladresse ist die TCP-Gegenstelle; ist sie eine Loopback- oder private Adresse (Traefik im Docker-Netz), zählt der letzte Eintrag von `X-Forwarded-For`. Nie genutzte Clients (kein Code, kein Token) werden nach 30 Minuten entfernt, Clients ohne lebende Freigabe nach 24 Stunden (bei jeder Registrierung und beim nächtlichen Backup). So kann niemand mit Wegwerf-Registrierungen das Verbinden von claude.ai blockieren.
- **Autorisierung:** `GET /oauth/authorize` mit `response_type=code`, `client_id`, `redirect_uri`, `code_challenge`, `code_challenge_method=S256` (Pflicht, `plain` wird abgelehnt), `state`, `resource`, `scope` (optional, wird nur gespeichert). Unbekannter oder widerrufener Client und nicht registrierte Redirect-URI → deutsche Fehlerseite (`400`), nie eine Weiterleitung. Redirect-URIs müssen exakt übereinstimmen; bei `http`-Loopback-URIs zählt der Port nicht (RFC 8252, gilt für `localhost` und IP-Literale). Fehlt `redirect_uri` und hat der Client genau eine, gilt diese. Alle anderen Fehler gehen per `303` an die Redirect-URI zurück (`error`, `error_description`, `state`, `iss`): `unsupported_response_type`, `invalid_request` (PKCE fehlt/ungültig), `invalid_target` (`resource` ist weder `<Issuer>/mcp` noch `<Issuer>/api`, oder `<Issuer>/api` für einen Client, dessen Redirect-URI weder App-Schema noch Loopback ist; Vergleich nach Normalisierung von Groß-/Kleinschreibung bei Schema und Host, Standardport und abschließendem `/`; fehlt `resource`, gilt `<Issuer>/mcp`), `invalid_scope` (`scope` > 1000 Zeichen); `state` > 2048 Zeichen → Fehlerseite. Ohne Browser-Sitzung (und ohne Dev-Auth) → `303` nach `/auth/login?return_to=/oauth/authorize?<ursprüngliche Query>`, nach dem Login zurück.
- **Freigabe:** Mit Sitzung zeigt Zunder eine deutsche Seite mit Client-Name, Host der Redirect-URI und angemeldeter Person (zusätzliche Warnung, wenn der Client nur Loopback-Redirect-URIs hat), Knöpfe „Erlauben“/„Ablehnen“. Die Seite hat `Cache-Control: no-store`, `Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'` und kein Skript. Das Formular sendet `POST /oauth/authorize` (`application/x-www-form-urlencoded`: `request_id`, `decision` = `allow`|`deny`). `request_id` sind 32 Zufallsbytes, einmal verwendbar, 10 Minuten gültig, im Speicher an die Sitzung gebunden (höchstens 64 offene Freigaben); das ersetzt einen CSRF-Token. Ein vorhandener `Origin` muss der von `ZUNDER_PUBLIC_URL` sein. Fremde oder fehlende Sitzung, fremder Origin → `403`-Seite; unbekannte oder abgelaufene `request_id` → `400`-Seite. `deny` → `access_denied` an den Client. `allow` → `303` an die Redirect-URI mit `code`, `state` und `iss`.
- **Autorisierungscode:** `zoc_` + 32 Zufallsbytes, als SHA-256 gespeichert, 5 Minuten gültig, gebunden an Client, Redirect-URI, PKCE-Challenge, Ressource und Besitzer. Jeder Einlöseversuch verbraucht den Code; ein erneuter Versuch mit einem verbrauchten Code widerruft alle daraus ausgestellten Tokens.
- **Token-Endpunkt:** `POST /oauth/token`, nur `application/x-www-form-urlencoded`, Antworten mit `Cache-Control: no-store` und `Pragma: no-cache`.
  - `grant_type=authorization_code`: `code`, `redirect_uri` (identisch mit der Autorisierung), `client_id`, `code_verifier` (43–128 Zeichen, S256 gegen die Challenge), optional `resource` (muss zur Autorisierung passen, sonst `invalid_target`).
  - `grant_type=refresh_token`: `refresh_token`, optional `client_id` (muss passen), optional `resource`. Rotation: Jeder Refresh-Token gilt genau einmal und liefert ein neues Paar. Wird ein schon rotierter Refresh-Token erneut vorgelegt, widerruft Zunder die ganze Familie (alle Access- und Refresh-Tokens dieser Freigabe) → `invalid_grant`.
  - Antwort `{"access_token":"zoa_…","token_type":"Bearer","expires_in":3600,"refresh_token":"zor_…"}` (plus `scope`, falls bei der Autorisierung angegeben). Access-Token 1 Stunde, Refresh-Token 30 Tage ab Ausstellung (jede Rotation beginnt neu). Beide opak (Präfix + 32 Zufallsbytes), nur als SHA-256 gespeichert.
  - Fehler: `invalid_request` (Parameter fehlen, falscher Content-Type), `invalid_client` (`401`, unbekannter oder widerrufener Client), `invalid_grant` (Code/Refresh-Token unbekannt, abgelaufen, verbraucht, widerrufen, fremder Client, falsche Redirect-URI, PKCE falsch), `invalid_target`, `unsupported_grant_type`.
- **Widerruf (RFC 7009):** `POST /oauth/revoke` mit `token` (optional `client_id`): Refresh-Token → ganze Familie, Access-Token → nur dieser, Geräte-Token → dieses Gerät. Antwortet immer `200`, auch für unbekannte Tokens.
- **Nutzung an `/mcp`:** OAuth-Access-Tokens gelten nur für `/mcp`, nicht für `/api/*` (dort `401`). Geprüft wird: bekannt, nicht widerrufen, nicht abgelaufen, Client nicht widerrufen und `resource` = eigene Ressource (Audience; ändert sich `ZUNDER_PUBLIC_URL`, werden alte Tokens ungültig). Quelle der Operationen `mcp:oauth:<client_name>`. `last_used_at` des Clients wird höchstens einmal pro Minute geschrieben.
- **Verwaltung:** `GET /api/oauth/clients` (nur Browser-Sitzung) listet Clients mit mindestens einem lebenden Token. `DELETE /api/oauth/clients/{id}` widerruft den Client und alle seine Tokens (`204`, unbekannt → `404`); ein widerrufener Client kann sich nicht erneut autorisieren, claude.ai registriert beim nächsten Verbinden einen neuen. Die Web-UI zeigt die Liste auf der Token-Seite unter „Verbundene Apps“ mit „Trennen“.
- **Geräte (Zunder-App, Schema 4):** Die App meldet sich mit derselben Autorisierung an (System-Browser, Rücksprung über ihr App-Schema), aber für die Ressource `<Issuer>/api`. Die Freigabeseite sagt dann, dass sich ein Gerät mit vollem Sync-Zugriff ohne Ablauf verbindet, und warnt wie bei Loopback, dass die Anmeldung an eine App auf dem Gerät geht (Ziel: das Schema). Der Token-Endpunkt liefert statt Access-/Refresh-Paar einen Geräte-Token `{"access_token":"zd_…","token_type":"Bearer"}` ohne `expires_in` und `refresh_token`: 32 Zufallsbytes, als SHA-256 gespeichert, läuft nicht ab (die App ist offline-first), gilt nur für `/api/*` (an `/mcp` → `401`). Das Gerät heißt wie der Client (`client_name`), seine Operationen tragen die Quelle `app:<name>`. Widerruf: unter „Geräte“ (`DELETE /api/devices/{id}`), über `/oauth/revoke` mit dem Geräte-Token (die App tut das beim erneuten Anmelden für den alten Token) und, wenn der Code ein zweites Mal eingelöst wird, zusammen mit der Familie. `devices` (id, user_id, name, token_hash, family_id, created_at, last_used_at, revoked_at) hat keinen Fremdschlüssel auf `oauth_clients`: Registrierte Clients werden nach einem Tag entfernt, das Gerät bleibt.
- **Speicher (Schema 2):** `oauth_clients` (id, name, redirect_uris_json, created_at, last_used_at, revoked_at), `oauth_codes` (code_hash, client_id, user_id, redirect_uri, code_challenge, resource, scope, family_id, created_at, expires_at, used_at), `oauth_tokens` (id, token_hash, kind `access`|`refresh`, client_id, user_id, family_id, resource, scope, created_at, expires_at, used_at, revoked_at). Abgelaufene Codes und Tokens räumt das nächtliche Backup ab.

## 5. Web-UI

SolidJS + Vite + TypeScript (strict). Kein UI-Framework mit großem Laufzeitanteil; eigenes, schlankes CSS mit Variablen, hell/dunkel.

- **Sync-Client** spricht dasselbe Protokoll: Snapshot beim Start, dann `pull` (ausgelöst über SSE, Fallback Polling 15 s). Änderungen werden lokal sofort angewendet (optimistisch), in eine Warteschlange gelegt und gepusht. Abgelehnte Operationen werden lokal zurückgerollt und als Meldung angezeigt. Merge-Logik in TypeScript identisch zum Kern; gemeinsame Testvektoren in `crates/zunder-core/tests/vectors/*.json` werden von Rust- **und** TS-Tests geprüft.
- **Sichten:** Heute (Start), Inbox, Lose, Bereiche → Projekte (Liste/Board), Dev, Phönix, Suche (⌘K), plus Login- und Token-Seite.
- **Links auf eine Aufgabe:** `<ZUNDER_PUBLIC_URL>/#/aufgabe/<id>` (und `#/clickup/<clickup_id>`) öffnet die Sicht, in der die Aufgabe steht (Inbox, Lose, Bereich oder Projekt), samt Detail; unbekannt oder gelöscht → Meldung und Heute. Das Detail zeigt den Link („Link“), z. B. zum Einfügen in den Vault.
- **Schnellerfassung:** Taste `n` bzw. großes ＋ mobil; Kurzschreibweise `Zahnarzt anrufen morgen 9:00 #gesundheit` (Datum: heute, morgen, übermorgen, Wochentage, `12.10.`, `12.10.2026`; Uhrzeit `9:00`/`9 Uhr` → `remind_at` und `due`; `#slug` → Bereich oder Projekt, Projekt gewinnt bei Gleichstand).
- **Aufgabendetail:** Seitenpanel, alles inline editierbar, Unteraufgaben-Baum, Änderungsprotokoll. Ein Textfeld schreibt nur, wenn darin getippt wurde: Fokus und Verlassen ohne Tippen überschreibt keine Änderung, die inzwischen per Pull ankam (das Feld zeigt sie, solange nicht getippt wurde); nach dem Verlassen zeigt es den gespeicherten Wert (abgelehnt → der alte). Ein Wechsel zu einer anderen Aufgabe baut den Inhalt neu auf, unfertige Eingaben (Unteraufgabe, Link) bleiben nicht stehen. Links (`links`) mit anderen Schemata als `http`, `https` und `mailto` zeigt die UI als Text, nicht als Link. Das Panel bekommt beim Öffnen den Fokus und gibt ihn beim Schließen an die ausgewählte Zeile zurück; mobil (volle Breite) ist der verdeckte Rest `inert`.
- **Tastatur:** `j/k` bewegen, `Enter` öffnen, `n` erfassen, `x` erledigt, `⌘K` Palette, in der Inbox `a` zuordnen, `l` lose, `d` verwerfen, `Esc` schließen, `g h/i/l/b/d/p` Sichtwechsel, `y` kopiert die Kennung (`ZUN-42`) der offenen oder ausgewählten Aufgabe, ohne Kennung ihre UUID. `j/k/Enter/x` wirken nur auf die Liste der sichtbaren Sicht; eine Sicht ohne Liste (Bereiche ohne Auswahl, Token-Seite, Dev ohne Dev-Projekte) hat keine. `Enter` auf einem fokussierten Knopf oder Link löst dessen eigene Aktion aus; `j/k` nehmen den Fokus von einem solchen Element auf die ausgewählte Zeile. Dialoge halten den Fokus (Tab/Shift+Tab kreisen im Dialog).
- **Mobil:** Tabs unten, Wisch-Gesten in der Inbox (rechts: lose, links: verwerfen); „Zuordnen“ bleibt als Knopf in jeder Inbox-Zeile sichtbar (die übrigen Zeilenaktionen sind mobil ausgeblendet).
- Erledigte/verworfene Aufgaben verschwinden **3 Tage** nach `status_changed_at` aus den Standardansichten, bleiben in der Suche.
- Playwright-Abläufe: erfassen, Inbox abarbeiten, Board verschieben, Flamme starten; Tempo-Budget (Interaktion < 100 ms, Kaltstart < 1 s) wird gemessen.
- Mit `ZUNDER_E2E_BACKEND=real` startet die Playwright-Konfiguration (`webServer`) den echten Server: Binärdatei aus `ZUNDER_SERVER_BIN` (Default `../target/debug/zunder-server`, relativ zu `web/`), mit `ZUNDER_DEV_AUTH=true`, einem frischen temporären `ZUNDER_DATA_DIR`, `ZUNDER_WEB_DIR=dist` (vorher `npm run build`), `ZUNDER_BIND=127.0.0.1:18080` und passender `ZUNDER_PUBLIC_URL`. Die CI baut den Server im Release-Profil und setzt `ZUNDER_SERVER_BIN` entsprechend.

### 5.1 Präzisierungen aus der Web-UI

Lücken, die beim Bau von `zunder-web` geschlossen wurden. Der Server darf mehr liefern, muss aber mindestens dies erfüllen.

- **Snapshot:** Ein Feld darf zusätzlich `op_id` tragen (`{"value","hlc","op_id"}`). Fehlt es, behandelt der Client die `op_id` als leer; bei identischem HLC gewinnt dann jede echte Operation. `created_hlc` übernimmt der Client als Erstellzeit (Standardrang, Sortierung); ohne das Feld gilt der kleinste Feld-HLC. Sind HLC und `op_id` gleich, entscheidet wie im Kern die feste Ordnung auf dem JSON-Wert.
- **Knoten-ID der Web-UI:** `web-<12 hex>-<8 hex>`: der erste Teil je Browser in `localStorage` gespeichert, der zweite je geöffneter Seite (Tab) neu gewürfelt, damit zwei Tabs nie denselben HLC erzeugen; `client_id` im Push ist dieselbe Kennung. Die Push-Warteschlange liegt ebenfalls in `localStorage`, je Seite unter einem eigenen Schlüssel (`zunder.pending.v1.<seite>`), damit sich zwei Tabs nicht gegenseitig überschreiben. Ob eine Seite noch lebt, zeigt ein Web Lock, den sie bis zu ihrem Ende hält (ohne Web Locks: ein Herzschlag in `localStorage`, nach 5 Minuten Stille gilt sie als beendet). Warteschlangen beendeter Seiten (und der frühere gemeinsame Schlüssel `zunder.pending.v1`) übernimmt die nächste Seite beim Start und danach bei jedem Poll-Intervall und `retryNow`. Ein Reload oder ein geschlossener Tab verliert so nichts (doppelt gepushte Operationen sind über `op_id` idempotent).
- **Push-Pakete:** ganze Gruppen (eine Gruppe = eine Benutzeraktion) von vorn aus der Warteschlange, höchstens 1000 Operationen und etwa 4 MiB JSON. Ein Paket endet vor der ersten Gruppe, die eine Entität einer früheren Gruppe im Paket berührt: Der Kern lehnt sonst wegen einer ungültigen späteren Gruppe auch frühere, gültige Änderungen derselben Entität ab (2.5). Lehnt der Server ein Paket mit mehreren Gruppen als Ganzes ab (z. B. 400, 413), wird nichts verworfen; die Warteschlange wird dann Gruppe für Gruppe gepusht, bis sie leer ist, und nur eine Gruppe, die allein abgelehnt wird, wird zurückgerollt. Einzelne UTF-16-Surrogate in Texten werden vor dem Einreihen durch U+FFFD ersetzt.
- **Tokens:** `GET /api/tokens` → `[{"id","name","created_at","last_used_at"}]`; `POST /api/tokens` mit `{"name"}` → dasselbe Objekt plus `"token"` (nur in dieser Antwort); `DELETE /api/tokens/{id}` → 204. Der Client akzeptiert auch `{"tokens":[…]}`. Darunter „Geräte“: `GET /api/devices`, Spalten Name, angemeldet seit, zuletzt abgeglichen; „Abmelden“ ruft `DELETE /api/devices/{id}`. Dann „Verbundene Apps“: `GET /api/oauth/clients` (auch `{"clients":[…]}` wird akzeptiert), Spalten Name, Host der Redirect-URIs, verbunden seit, zuletzt benutzt; „Trennen“ ruft `DELETE /api/oauth/clients/{id}`.
- **Einstellungen:** `#/einstellungen` (Seitenleiste, Topbar, Befehlspalette „Einstellungen“) zeigt den Abschnitt „Timing“: Status (Token-Hinweis, Projekt und jeweils die Herkunft), Felder „Timing-Token“ (Passwortfeld, leer = behalten) und „Timing-Projekt“ (zeigt das wirksame Projekt), „Speichern“ (nur bei Änderungen; das Projekt wird nur gesendet, wenn es geändert wurde, damit ein Projekt aus der Umgebung nicht unbemerkt zur Einstellung wird), „Verbindung prüfen“ und „Zurücksetzen“ (mit Rückfrage). In der App ersetzt ein Hinweis auf die Web-Oberfläche die Seite.
- **Änderungsprotokoll:** `GET /api/tasks/{id}/history` → Array `[{"seq","op","source","received_at"}]` (auch `{"entries":[…]}` wird akzeptiert). Die UI zeigt die Quelle lesbar: `web` → „Web“, `api:import` → „Import“, `api:<name>` → „API · name“, `mcp:<name>` → „Agent · name“, `mcp:oauth:<client>` → „Agent · client (OAuth)“, `app:<gerät>` → „App · gerät“, `server` → „Server“.
- **Flamme:** Der Server schreibt die Änderungen als Operationen (`flame_started_at`, `flame_ended_at`, ggf. `timing_ref`); der Client holt sie per `pull`. Aus dem Antwortkörper liest die UI nur `timing` und `unchanged`: Bei `timing.configured && !timing.used` (und nicht `unchanged`) zeigt sie statt „Flamme gestartet.“/„Flamme beendet.“ eine Warnung mit `timing.error` („Flamme läuft, aber Timing: …“), denn Timing ist die einzige Zeiterfassung. Fehlt der Körper oder ist er unerwartet, bleibt es bei der Erfolgsmeldung. In der App reicht `flame_start`/`flame_stop` die Antwort unverändert durch.
- **Warnungen:** `open_subtasks` zeigt die UI schon beim Erledigen selbst (auch offline); dieselbe Warnung aus der Push-Antwort wird deshalb nicht noch einmal angezeigt.
- **Abmelden:** `POST /auth/logout`, danach lädt die UI neu; `GET /api/me` mit 401 zeigt die Login-Seite (Knopf auf `/auth/login`).
- **Rang:** Aufgaben ohne `rank` sortieren nach einem Standardrang aus der Erstellzeit: `wall_ms` des kleinsten HLC in Basis 36, auf 9 Stellen mit `0` aufgefüllt, plus Suffix `i`. Erzeugte Ränge nutzen `[0-9a-z]` und enden nie auf `0`. Beim Einsortieren zählt nur die Stringordnung der Nachbarn, deshalb funktioniert das auch zwischen fremden Rängen wie den Dezimalrängen des Imports (`"0000000000001.500000"`); diese sortieren vor allen Standardrängen neuer Aufgaben. Nur unterhalb eines Rangs, der ab der Abweichung nur aus `0` und `.` besteht, darf ein erzeugter Rang auf `0` enden. Erledigte/verworfene Aufgaben stehen in Listen unten, neueste Statusänderung zuerst.
- **Kurzschreibweise:** Ein Wochentag allein meint das nächste Vorkommen (1–7 Tage voraus), `nächsten <Wochentag>` den Tag in der folgenden Kalenderwoche (Montag als Wochenbeginn). `12.10.` in der Vergangenheit meint das nächste Jahr. Eine Uhrzeit ohne Datum, die heute schon vorbei ist, meint morgen. `am` vor einem Datum und `um` vor einer Uhrzeit werden mit entfernt. Unbekannte `#tags` bleiben im Titel stehen und werden in der Vorschau markiert.
- **Kennungen:** Zeilen und Karten zeigen `ref` als erstes Merkmal; das Detail zeigt bei Aufgaben mit Kennung oder in Dev-Projekten oben „Kennung“ bzw. „ID“ als Knopf zum Kopieren, in der Fußzeile „UUID kopieren“. Der Projektdialog setzt `key` (Eingabe wird großgeschrieben). Die Palette findet Aufgaben über ihre Kennung (exakter Treffer zuerst); `#/aufgabe/<Kennung>` öffnet die Aufgabe wie `#/aufgabe/<id>`, der Link im Detail nutzt die Kennung, sobald es eine gibt.
- **Inbox:** Erledigte oder verworfene Aufgaben verlassen die Inbox sofort (Ziel: null); die übrigen Sichten folgen der 3-Tage-Regel.
- **Validierung im Client:** `web/src/sync/validate.ts` spiegelt `zunder_core::validate` mit denselben Codes (Abschnitt 2.5) und Regeln (Wertformen, Pflichtfelder nicht `null`, `repo` nur bei `dev`, Dev-Regeln inkl. Verursacher-Zuordnung, Referenzen, `has_dependents`, Wiederherstellen per `deleted: false`/`null`). Eine Änderung wird bei einem Befund ganz verworfen, die Meldung zeigt die erste Ursache. Löschen in der UI nimmt die lebenden Abhängigen mit (`deleteCascade`, wie `OpBatch::delete_cascade`).
- **Mock-Server:** `web/e2e/mock-server.ts` bildet für die UI relevante Serverregeln nach (Fehlerformat, CSRF-Header, Snapshot mit `created_hlc`/`op_id`, Verlauf, Flammen inkl. 409, Token-Regeln, Timing-Einstellungen (ohne Umgebungsvariablen), Kennungen (Vergabe nach jedem Push, `ref` nur vom Server, eindeutige Schlüssel), verbundene OAuth-Apps (Testendpunkt `POST /__test/oauth-client`), eine Teilmenge der Kernvalidierung mit Kerncodes). Ablehnungen in E2E-Tests entstehen in beiden Modi echt (z. B. veralteter Client-Zustand, Verweis auf eine inzwischen gelöschte Entität).

### 5.1 App-Befehle (Tauri, E4)

In der Tauri-App spricht die Web-UI nicht direkt mit dem Server (die CSP erlaubt nur `ipc:`, der Server kein CORS), sondern über Befehle der App, die einen lokalen SQLite-Speicher mit Operations-Log (`pending` + `confirmed`), materialisiertem Zustand und Cursor führt und selbst synchronisiert. Der `TauriTransport` der Web-UI (`web/src/platform/tauri.ts`) ruft sie per `invoke` auf. Fehler: abgelehntes Promise mit `{"code","message"}`; Fehler von Tauri selbst (unbekannter Befehl, unpassendes Argument) als String.

| Befehl | Argumente | Ergebnis |
|---|---|---|
| `local_snapshot` | – | `{"cursor","entities":[SnapshotEntity],"pending":N}` (Format von `GET /api/snapshot`, lokale Sicht = bestätigt + ausstehend, inkl. Tombstones) |
| `local_apply` | `{"ops":[Op]}` | `{"accepted":[op_id],"rejected":[Rejection],"warnings":[Warning],"pending":N}`; lokal per `validate_push` geprüft, angenommene neue Operationen sofort sichtbar und zum Push vorgemerkt |
| `sync_now` | – | `{"pushed","rejected":[Rejection],"warnings":[Warning],"pulled","cursor","pending","changed","resynced"}` |
| `get_config` | – | `{"server_url"\|null,"has_api_token","node_id","cursor","pending","last_sync":{"at_ms","ok","error"?}\|null}` (Token wird nie zurückgegeben) |
| `set_config` | `{"update":{"server_url"?,"api_token"?,"discard_pending"?}}` | wie `get_config`; fehlender Schlüssel = unverändert, `null` = löschen; `http://` nur für lokale Adressen (Loopback, private/Link-local-Adressen inkl. 100.64.0.0/10, Namen ohne Punkt, `*.localhost`/`.local`/`.home.arpa`/`.internal`/`.lan`), sonst `https://` (Fehler `invalid_config`) |
| `login_start` | `{"server_url","device_name","discard_pending"}` | – ; liest die OAuth-Metadaten des Servers, registriert die App als Client `device_name` (1–100 Zeichen) mit der Redirect-URI `dev.rubeen.zunder:/oauth`, speichert die laufende Anmeldung (`client_id`, PKCE-Verifier, `state`, Token-Endpunkt, Ressource `<Issuer>/api`, 15 Minuten gültig) in `meta` und öffnet die Autorisierung im System-Browser. Ein Serverwechsel, der ausstehende Operationen verwerfen würde, scheitert sofort mit `pending_ops` (außer `discard_pending`). Das Ergebnis kommt als `zunder://login` |
| `task_history` | `{"task_id"}` | `GET /api/tasks/{id}/history` mit dem gespeicherten Token, Antwort unverändert |
| `flame_start`, `flame_stop` | `{"task_id"}` | `POST /api/tasks/{id}/flame/start\|stop`, Antwort unverändert; vorher Push, falls Operationen ausstehen, danach ein Sync (Flammenfelder kommen per `zunder://changed`). 409 → Fehler `server` |

- Sync-Ablauf: ausstehende Operationen in Paketen zu 500 pushen (`client_id` = `node_id`), bei Cursor 0 den Snapshot übernehmen, dann `pull` bis `has_more = false`. Abgelehnte Operationen werden lokal zurückgenommen (Sicht = bestätigter Zustand + übrige ausstehende) und gemeldet. Jeder Schritt ist für sich festgeschrieben; scheitert ein Durchgang mittendrin, meldet er trotzdem, was er bis dahin getan hat (Ablehnungen, `changed`).
- Server zurückgesetzt (Backup eingespielt, `seq` werden neu vergeben): Jeder Pull mit Cursor > 0 beginnt eine Operation früher (`since = cursor - 1`; die überlappende zählt nicht als gepullt) und prüft, dass die Operation am eigenen Cursor noch unter dieser `seq` steht, und zwar mit derselben `op_id`, sobald die App sie kennt (nach einem Snapshot bringt der erste Pull sie mit). Fehlt sie oder steht dort eine andere, verwirft die App den bestätigten Zustand samt Log und Cursor, behält die ausstehenden Operationen und lädt den Snapshot neu (`resynced: true`, `changed: true`).
- Auslöser: Start, alle 30 s, 0,5 s nach `local_apply`, Rückkehr in den Vordergrund, `online`-Ereignis des Webviews, `sync_now`; nach Netzwerkfehlern 5/10/20 s, dann wieder 30 s.
- Die Web-UI nimmt `node_id` (Form `app-<12 hex>`) als Knoten ihrer HLC.
- Serverwechsel löscht die lokalen Daten; mit ausstehenden Operationen nur mit `discard_pending: true` (sonst Fehler `pending_ops`). Verglichen wird mit dem Server, von dem die lokalen Daten stammen (auch wenn die Adresse zwischendurch mit `null` gelöscht wurde). Ein Durchgang, während dessen der Server gewechselt wurde, verwirft seine Ergebnisse und endet mit `reconfigured`.
- Fehlercodes: `not_configured`, `invalid_config`, `pending_ops`, `network`, `unauthorized`, `server`, `protocol`, `db`, `corrupt`, `schema_too_new`, `notification`, `reconfigured`.
- Anmeldung im Browser: Der Server leitet nach der Freigabe auf `dev.rubeen.zunder:/oauth?code=…&state=…&iss=…` (manche Browser normalisieren zu `dev.rubeen.zunder://oauth/?…`, beides gilt). Das Betriebssystem übergibt die Adresse der App (Deep-Link, auch wenn sie dafür neu gestartet wird); sie prüft `state`, verbraucht die gespeicherte Anmeldung (einmalig), tauscht den Code gegen den Geräte-Token und speichert Server-Adresse und Token wie `set_config` (danach Sync). Ersetzt der neue Token einen alten Geräte-Token, widerruft die App den alten (`/oauth/revoke`, ohne Fehler zu melden). Scheitert das Speichern, widerruft sie den neuen. Ereignis `zunder://login` `{"ok":true}` bzw. `{"ok":false,"error":{"code","message"}}` (Code `login` für abgelehnte, abgelaufene oder fremde Antworten). Einschränkung: macOS liefert Deep-Links nur an eine installierte App (nicht an `tauri dev`); Linux und Windows bräuchten zusätzlich Single-Instance. Der Token bleibt als Weg daneben.
- Ereignisse: `zunder://changed` `{"cursor","pending"}` (Sicht durch Sync geändert, auch durch einen später gescheiterten Durchgang → `local_snapshot` neu laden), `zunder://sync` `{"ok":true,"report"}` bzw. `{"ok":false,"error":{"code","message"},"report"}` (`report` = was der Durchgang vor dem Fehler festgeschrieben hat; für `reconfigured` kein Ereignis).
- **Web-UI in der App:** Erkennung über `window.__TAURI_INTERNALS__`; die Brücke und `@tauri-apps/api` werden nur dann per dynamischem Import geladen. Keine OIDC-Anmeldung in der Web-UI: ohne `server_url` oder Token erscheint eine Einrichtung mit Server-Adresse und Gerätename (Vorschlag aus dem User-Agent: „iPhone“, „Mac“ …) und „Im Browser anmelden“ (`login_start`, danach Hinweis bis `zunder://login`; kehrt das Fenster zurück, liest die UI `get_config` neu, falls das Ereignis verloren ging). Daneben weiterhin Adresse + API-Token → `set_config`, dann `sync_now` als Probe (die Einstellungen bleiben auch bei einem Fehlschlag gespeichert). `pending_ops` bietet in beiden Fällen „Änderungen verwerfen und wechseln“ an. Die Sync-Engine der Web-UI nutzt `local_snapshot` als Snapshot und `local_apply` als Push, pullt nicht und lädt nach `zunder://changed` den Snapshot neu (Ausstehendes bleibt obenauf; ein Snapshot, der während eines `local_apply` angefordert wurde, wird verworfen und neu geholt). Die Sync-Anzeige folgt `zunder://sync` und dem `pending` der App, Ablehnungen aus `report.rejected` werden gemeldet (auch bei `ok: false`). API-Tokens lassen sich in der App nicht verwalten (nur Browser-Sitzung); an ihre Stelle tritt die Ansicht „Verbindung“ (`#/verbindung`).
- Erinnerungen: für offene, lebende Aufgaben zu `remind_at` und zu `due` mit Uhrzeit (gleicher Moment → nur eine). Mobil plant das Betriebssystem die 64 nächsten (iOS-Grenze) über `tauri-plugin-notification`, jede für sich (eine, die das System ablehnt, hält die übrigen nicht auf und wird beim nächsten Mal erneut geplant); Desktop zeigt sie per Timer, solange die App läuft.
- **Kein Web-Push:** Erinnerungen stellt nur die App zu. Der in der Spec (Abschnitte „Erinnerungen“ und Risiken) genannte Web-Push-Fallback für den Fall, dass E4 sich verzögert, wird nicht gebaut, weil E4 gebaut ist; die Web-UI im Browser zeigt `remind_at` nur in „Heute“ an. Verlässlich sind Erinnerungen damit erst mit laufender Desktop-App oder einem auf dem Gerät geprüften Mobile-Build (siehe Abschnitt 7). Wird ein Mobile-Build dauerhaft nicht fertig, ist Web-Push (Service Worker, VAPID, Abo-Tabelle, Planer im Server) der Weg.

Einzelheiten: [`apps/zunder-app/README.md`](../../../apps/zunder-app/README.md).

## 6. Import

`zunder-import` (CLI, clap): `plan` (Probelauf → `mapping.toml`), `report` (Mengen, unbekannte Status, Aufgaben ohne Ziel), `run --mapping mapping.toml` (schreibt über `POST /api/sync/push` mit API-Token, Knoten `import`), `backup --out <dir>` (Roh-JSON inkl. Zeiteinträgen, Standard-Ziel außerhalb des Repos). Idempotent über `clickup_id` (erneuter Lauf aktualisiert statt dupliziert; IDs werden deterministisch aus der `clickup_id` abgeleitet: UUIDv5 im Zunder-Namespace oder Lookup).

Präzisierungen (umgesetzt in `crates/zunder-import`, Bedienung im README der Crate):

- **Umgebung:** `CLICKUP_TOKEN` (persönlicher Token, Header `Authorization: <token>`; `--token` bei `plan`/`report`/`backup`, `--clickup-token` bei `run`), `ZUNDER_URL` und `ZUNDER_TOKEN` (bzw. `run --server`, `--token`). Pushes tragen `client_id: "import"` und den Header `X-Zunder-Client: import`; Standard-Paketgröße 500 Operationen.
- **Deterministische IDs** (`clickup_entity_id(kind, clickup_id)`): Aufgabe ← ClickUp-Task-ID; Bereich ← Space-ID; Projekt ← Listen-ID bzw. `folder-<Ordner-ID>`, wenn ein ClickUp-Ordner das Projekt bildet. Bereiche/Projekte ohne `clickup_id` in der Zuordnungsdatei: `deterministic_id("import:<entity>:<schlüssel>")`. Mit `existing_id` wird auf einen bestehenden Eintrag abgebildet und nichts dafür geschrieben; bei Projekten muss `kind` der Art des Zunder-Projekts entsprechen (die Dev-Regeln der Umwandlung hängen daran), sonst bricht `run` nach dem Snapshot vor dem ersten Push ab.
- **Zuordnungsdatei** (`version = 1`, TOML, unbekannte Schlüssel werden abgelehnt): `team_id`, `[settings]` (`timezone`, Default `Europe/Berlin`; `import_archived_tasks`, Default `false`), `[type_tags]` (ClickUp-Tag klein → `bug`/`feature`), `[areas.<schlüssel>]` (`name`, `clickup_id`, `existing_id`, `state`), `[projects.<schlüssel>]` (`name`, `area`, `kind`, `repo`, `status`, `clickup_id`, `existing_id`), `[lists."<Listen-ID>"]` (`path` nur zur Info, `target` = `area:<k>` | `project:<k>` | `inbox` | `loose` | `skip`, leer = offen; `[lists."<id>".statuses]` ClickUp-Statusname klein → Zunder-Status, leer = offen). Offene Status und Listen ohne Ziel blockieren `run`.
- **Felder einer importierten Aufgabe:** `title` (getrimmt; leer → „(ohne Titel)“, über 500 Zeichen gekürzt), `description` (`markdown_description`, sonst `text_content`, sonst `description`), `type` (aus `type_tags`), `status` (Zuordnung je Liste), `location` (Ziel der eigenen Liste, auch bei Unteraufgaben), `parent_id`, `due`, `links` (`[{"url": <ClickUp-URL>, "title": "ClickUp"}]`), `clickup_id`, `rank`. `bug`/`feature` bzw. `review` außerhalb eines Dev-Projekts (maßgeblich ist die Wurzel-Aufgabe) werden zu `aufgabe` bzw. `in_progress` und im Bericht gelistet.
- **Fälligkeit:** `due_date` (Unix-ms) in der Zeitzone der Zuordnung; `due_date_time = true` → RFC 3339 mit Offset (Sekunden), `false` → `YYYY-MM-DD`; fehlt das Flag, gilt 00:00 oder 04:00 Ortszeit als reines Datum.
- **Rang:** `orderindex` als Dezimalzahl fester Breite, 13 Vor- und 6 Nachkommastellen (`1.5` → `"0000000000001.500000"`, negativ → 0). Andere Clients müssen beim Einsortieren nur die lexikografische Ordnung erhalten.
- **Unteraufgaben:** beliebige Tiefe; Eltern außerhalb der geladenen Listen (archiviert, andere Liste) werden per `GET /task/{id}` nachgeladen und mit importiert. Fehlt die Eltern-Aufgabe oder wird sie nicht importiert (`skip`), wird die Unteraufgabe eigenständige Aufgabe ihrer Liste. Archivierte Aufgaben sonst nur mit `import_archived_tasks`.
- **Erneuter Lauf:** `run` liest `GET /api/snapshot` und schreibt nur abweichende Felder. Der Import besitzt die Aufgabenfelder `title`, `description`, `type`, `status`, `location`, `parent_id`, `due`, `links`, `clickup_id`, `rank` (ClickUp gewinnt; in ClickUp leer gewordene werden auf `null` gesetzt), bei Bereichen `name`, `state` und bei Projekten `name`, `area_id`, `status`, `kind`, `repo`. Andere Felder bleiben unberührt; in Zunder gelöschte Entitäten werden übersprungen. Die Import-Uhr wird vorher per `receive` über alle HLCs des Snapshots gehoben. Ausnahme: Operationen einer neu angelegten Aufgabe werden mit einer eigenen Uhr auf die ClickUp-Zeitpunkte datiert (alle Felder auf `date_created`, der Status geschlossener Aufgaben auf `date_closed`, sonst `date_done`, sonst `date_updated`; höchstens der Importzeitpunkt). `created_at` entspricht damit der ClickUp-Erstellung, und die 3-Tage-Regel zählt ab dem Schließen in ClickUp statt ab dem Import.
- **Vorprüfung:** Vor dem ersten Push prüft `run` jedes Paket lokal mit `validate_push`, so wie der Server es sieht: Paket für Paket auf einer Kopie des Snapshots, auf die die angenommenen Operationen der vorigen Pakete angewandt sind. Bei Ablehnungen wird ohne `--keep-going` nichts geschrieben; serverseitige Ablehnungen brechen nach dem jeweiligen Paket ab. Pakete teilen nie die Operationen einer neuen Entität (bei vorhandenen Aufgaben können die vorgezogenen Typ-/Statusänderungen in einem früheren Paket liegen), haben höchstens `--batch-size` Operationen und 7 MiB serialisierte Operationen (Server-Grenze 8 MiB je Anfrage) und halten die Reihenfolge: zuerst `type`/`status`-Änderungen vorhandener Aufgaben auf Werte, die überall erlaubt sind (sie machen den Wechsel eines Projekts zu `standard` oder das Verschieben aus einem Dev-Projekt erst zulässig), dann Bereiche → Projekte → Aufgaben (Eltern vor Kindern). Eine Entität, die allein größer als 7 MiB ist, wird nie gesendet; `run` bricht dann ohne `--keep-going` vor dem ersten Push ab und nennt sie.
- **Backup:** Basis Standard `~/zunder-import-backup`, darin `<JJJJ-MM-TT_hh-mm-ss>/` (Rechte 0700) mit `manifest.json`, `teams.json`, `teams/<id>/spaces.json`, `teams/<id>/time_entries.json`, `spaces/<id>/folders.json`, `spaces/<id>/lists.json`, `folders/<id>/lists.json`, `lists/<id>/list.json`, `lists/<id>/tasks.json` (aktiv und archiviert), `lists/<id>/comments.json` (Listen-Kommentare, nur wenn vorhanden), `tasks/parents.json`, `tasks/<id>/task.json` (`GET /task/{id}` jeder Aufgabe aus den Listen, mit Anhang-Metadaten), `tasks/<id>/comments.json` (`GET /task/{id}/comment`, Seiten zu 25 über `start`/`start_id` des ältesten Kommentars, Antworten aus `GET /comment/{id}/reply` unter `"replies"`; nur wenn vorhanden). `manifest.json` zählt zusätzlich `task_details`, `attachments`, `tasks_with_comments`, `lists_with_comments`, `comments` und `comment_replies`. Kommentare werden nicht importiert, nur gesichert. Zeiteinträge ab `--since` (Default 2017-01-01) in Fenstern von `--window-days` (Default 30), dedupliziert nach ID. Ziele in einem Git-Repository (Suche nach `.git` aufwärts) werden ohne `--force` abgelehnt.
- **Rate-Limit:** HTTP 429 → warten bis `X-RateLimit-Reset` (Unix-Sekunden, höchstens 120 s) bzw. `Retry-After`, dann erneut; bei `X-RateLimit-Remaining: 0` wird vor der nächsten Anfrage bis zum Reset gewartet; 5xx und Netzwerkfehler mit Backoff, insgesamt höchstens 8 Versuche.

## 7. Etappen im Repo

E1–E2 werden vollständig gebaut. E3 ist überwiegend Betrieb (Runbook in `docs/umstieg.md`). E4 (Tauri) wird als lauffähiges Gerüst angelegt, das `zunder-core` für lokale SQLite und Sync nutzt; Mobile-Builds werden in dieser Umgebung nicht verifiziert. Speicher, Sync, die Server-Aufrufe für Verlauf und Flammen und die Erinnerungsplanung liegen in der webview-freien Crate `apps/zunder-app/src-tauri/zunder-local` (zweites Mitglied des eigenen Workspaces von `src-tauri`), die Tauri-Hülle verdrahtet sie nur (Befehle: Abschnitt 5.1). Die Web-UI ist an die App angebunden (Abschnitt 5.1, „Web-UI in der App“).
