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
| `GET /api/tasks/{id}/history` | Änderungsprotokoll: `[{"seq","op","source","received_at"}]` |
| `POST /api/tasks/{id}/flame/start` | Flamme starten (Timing-Brücke, Fallback-Zeitstempel), Details in 4.2 |
| `POST /api/tasks/{id}/flame/stop` | Flamme beenden (409, wenn keine läuft) |
| `GET /api/tokens` / `POST /api/tokens` / `DELETE /api/tokens/{id}` | API-Tokens verwalten (nur Session-Auth): Liste `[{"id","name","created_at","last_used_at"}]`; Anlegen mit `{"name"}` → `201` `{"id","name","created_at","last_used_at","token"}` (Token nur in dieser Antwort); Widerruf → `204` |
| `POST /mcp` (+ `GET`, `DELETE`) | MCP Streamable HTTP |

- Idempotenz: Eine bereits bekannte `op_id` gilt als `accepted`, wird aber nicht erneut gespeichert.
- `seq` ist ein monotoner Server-Zähler; der Cursor ist das größte vergebene `seq`.
- Operationen, die der Server selbst erzeugt (MCP, Timing), und alle gepushten Operationen laufen durch dieselbe Validierung und dasselbe Log und tragen eine `source`: `web` (Browser-Sitzung), `api:<token-name>` (Sync-API mit API-Token, z. B. `api:import`, wenn der Import einen Token namens `import` nutzt; `api:bootstrap` für `ZUNDER_BOOTSTRAP_TOKEN`), `mcp:<token-name>`, `server`.

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

### 3.1 Präzisierungen (umgesetzt in `zunder-server`)

**Fehler.** Format `{"error":{"code","message"}}` mit `code` ∈ `unauthorized` (401, mit `WWW-Authenticate: Bearer`), `forbidden` (403), `not_found` (404), `invalid_request` (400), `conflict` (409, z. B. Token-Name doppelt), `payload_too_large` (413), `unavailable` (503), `internal` (500). Unbekannte Pfade unter `/api/` liefern `not_found` als JSON.

**Push.** `client_id` ist optional (≤ 128 Zeichen) und wird je Operation gespeichert. Ist der Body kein gültiges JSON oder hat eine Operation nicht das Wire-Format (z. B. keine kanonische UUID, ungültiger HLC, fehlendes `value`), wird der ganze Push mit `400 invalid_request` abgelehnt; inhaltliche Fehler meldet der Kern je Operation in `rejected`. Ein Push ohne neue Operationen ändert den Cursor nicht und löst kein SSE-Ereignis aus. Body-Limit: 8 MiB für `POST /api/sync/push`, 64 KiB für alle anderen Anfragen.

**Pull.** `since` Default 0, nicht negativ; `limit` Default 1000, höchstens 5000 (größere Werte werden gekappt), mindestens 1. `cursor` ist das `seq` der letzten gelieferten Operation; ohne Operationen der aktuelle Server-Cursor. `has_more` heißt: direkt mit `since=cursor` weiterholen.

**SSE.** `GET /api/sync/events` sendet sofort `event: cursor` mit dem aktuellen Cursor und danach bei jeder Änderung; Keep-Alive-Kommentare alle 15 s. SSE-Antworten werden nie komprimiert. Beim Herunterfahren endet der Stream.

**Snapshot.** Enthält auch gelöschte Entitäten (Tombstones), damit Clients sie mergen können.

**Verlauf.** `GET /api/tasks/{id}/history` liefert alle Operationen der Aufgabe in `seq`-Reihenfolge, `received_at` als RFC 3339 (UTC). Unbekannte Aufgabe → 404, keine kanonische ID → 400.

**Besitzer.** Eine Instanz hat genau einen Besitzer. Beim ersten Start legt der Server die Zeile in `users` an (nötig für `owner_id` an Operationen, auch vor dem ersten Login, z. B. für einen Import per Bootstrap-Token). Der erste erfolgreiche OIDC-Login hinterlegt dort Subject, E-Mail und Namen, spätere Logins aktualisieren E-Mail und Namen. Alle Zugangswege (Sitzung, Token, Bootstrap-Token, Dev-Auth) handeln als dieser Besitzer.

**Sitzungen.** Cookie `zunder_session` mit 32 Zufallsbytes (base64url), in der DB nur als HMAC-SHA256 mit `ZUNDER_SESSION_SECRET` (mindestens 32 Zeichen; Pflicht, sobald OIDC konfiguriert und Dev-Auth aus ist). `HttpOnly`, `SameSite=Lax`, `Path=/`, `Secure` außer bei `http://localhost`/`127.0.0.1`/`[::1]`. Gültigkeit 30 Tage, gleitend: Ist die letzte Verlängerung älter als ein Tag, verlängert der Server sie und sendet das Cookie neu. `POST /auth/logout` löscht die Sitzung (→ 204).

**CSRF.** Mit Cookie (oder Dev-Auth) authentifizierte Anfragen, die Zustand ändern (alles außer `GET`/`HEAD`/`OPTIONS`, auch `POST /auth/logout`), brauchen den Header **`X-Zunder-Client: web`**; sonst 403 `forbidden`. Die Web-UI sendet ihn bei jeder API-Anfrage. Bearer-Token-Anfragen brauchen ihn nicht. Der Server erlaubt kein CORS.

**API-Tokens.** Format `zt_` + 32 Zufallsbytes base64url (43 Zeichen), gespeichert als SHA-256 (hex). Namen 1–64 Zeichen, ohne Steuerzeichen, eindeutig unter den aktiven Tokens, `bootstrap` ist reserviert. Widerrufen = `revoked_at` setzen (der Name bleibt im Verlauf nachvollziehbar). `last_used_at` wird höchstens einmal pro Minute geschrieben. Ein ungültiger Bearer-Token ist immer 401, auch wenn zusätzlich ein Cookie mitkommt. `ZUNDER_BOOTSTRAP_TOKEN` gilt zusätzlich als Token mit dem Namen `bootstrap`.

**OIDC.** `GET /auth/login?return_to=/pfad` (nur lokale Pfade) leitet mit `state`, `nonce` und PKCE (S256) zum Anbieter; Scope `openid email profile`. Der Server merkt sich laufende Logins 10 Minuten im Speicher und bindet sie per Cookie `zunder_login` (Pfad `/auth`) an den Browser. Der Callback tauscht den Code (`client_secret_post`, ohne Secret als öffentlicher Client), prüft das ID-Token (Signatur per JWKS, keine HS-Algorithmen, `iss`, `aud`, bei mehreren Audiences `azp`, `exp`/`iat` mit 60 s Toleranz, `nonce`) und setzt die Sitzung. Unbekannte Schlüssel-IDs lösen ein erneutes Laden der JWKS aus. Erlaubt ist: das Subject aus `ZUNDER_OIDC_ALLOWED_SUBJECT`, falls gesetzt, sonst die E-Mail aus `ZUNDER_OIDC_ALLOWED_EMAIL` (Groß-/Kleinschreibung egal, `email_verified` darf nicht `false` sein). Fehler zeigt eine deutsche HTML-Seite. Ohne konfigurierten Anbieter antwortet `/auth/login` mit 503.

**Dev-Auth.** `ZUNDER_DEV_AUTH=true` ist nur mit lokaler `ZUNDER_PUBLIC_URL` erlaubt (sonst startet der Server nicht) und wird beim Start laut geloggt. Anfragen ohne Zugangsdaten gelten dann als der Besitzer (Name „Entwicklung“, E-Mail `dev@localhost`, solange kein OIDC-Login erfolgt ist); `/auth/login` legt direkt eine Sitzung an und leitet zurück. Die CSRF-Regel gilt auch hier.

**Weitere Variablen.** `ZUNDER_PUBLIC_URL` Default `http://localhost:<Port aus ZUNDER_BIND>`; OIDC-Redirect-URI ist `<ZUNDER_PUBLIC_URL>/auth/callback`. `ZUNDER_OIDC_CLIENT_SECRET` ist optional. `RUST_LOG` steuert das Log (Default `info`). Das Backup läuft zur lokalen Stunde `ZUNDER_BACKUP_HOUR` (Zeitzone über `TZ`), Dateiname `zunder-JJJJMMTT-HHMMSS.db`; dabei werden abgelaufene Sitzungen entfernt.

**Gesundheitsprüfung.** `zunder-server healthcheck` fragt `http://127.0.0.1:<Port aus ZUNDER_BIND>/healthz` ab (bzw. die gebundene IP, wenn nicht `0.0.0.0`/`::`) und endet mit Exit-Code 0 bei `{"status":"ok"}`, sonst 1. Gedacht für `HEALTHCHECK` im Dockerfile. `/healthz` prüft die Datenbank und antwortet bei Fehlern mit 503 `{"status":"error"}`.

**Statische Dateien.** `ZUNDER_WEB_DIR` wird unter `/` ausgeliefert. `/assets/*` (Vite-Ausgabe mit Hash im Namen) mit `Cache-Control: public, max-age=31536000, immutable`, alles andere mit `no-cache`. Unbekannte Pfade ohne Dateiendung liefern `index.html` (SPA-Fallback); fehlende Dateien unter `/assets/` oder mit Endung sind 404. Antworten werden mit gzip oder Brotli komprimiert, wenn der Client es anbietet.

**Schema.** Migrationen sind eingebettetes SQL, versioniert über `PRAGMA user_version`; ein Server verweigert eine Datenbank mit neuerer Version. Zeitstempel sind Unix-Millisekunden. `meta.materialized_seq:<owner_id>` hält fest, bis zu welchem `seq` die Tabelle `entities` aktuell ist; stimmt das (oder die Anzahl der Entitäten) beim Start nicht mit dem Log überein, baut der Server `entities` aus dem Log neu auf.

### 3.2 Container

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

### 4.1 Präzisierungen (umgesetzt in `zunder-server`)

**Transport.** `POST /mcp` nimmt eine JSON-RPC-Nachricht oder einen Batch (Array) an. Anfragen → `200 application/json` (Batch → Array der Antworten); nur Benachrichtigungen bzw. Client-Antworten → `202` ohne Body. Kein serverseitiger Stream: `GET /mcp` und `DELETE /mcp` → `405`. Zustandslos: Der Server vergibt keine `Mcp-Session-Id`, eine mitgeschickte wird ignoriert. Ein vorhandener Header `MCP-Protocol-Version` muss `2025-06-18` oder `2025-03-26` sein (sonst `400`). `initialize` antwortet mit der angefragten Version, falls unterstützt, sonst mit `2025-06-18`; dazu `serverInfo {name:"zunder", title, version}`, `capabilities {tools:{listChanged:false}}` und deutsche `instructions` (Datenmodell, Status, Stellen). Methoden: `initialize`, `ping`, `tools/list` (ohne Paging), `tools/call`; Benachrichtigungen (`notifications/initialized` u. a.) werden angenommen. Body-Limit 1 MiB.

**Auth und Herkunft.** Nur Bearer-Token (API-Token oder `ZUNDER_BOOTSTRAP_TOKEN`); Session-Cookie und Dev-Auth gelten für `/mcp` nicht (→ `401` mit `WWW-Authenticate: Bearer`). Ein vorhandener `Origin`-Header muss dem Origin von `ZUNDER_PUBLIC_URL` entsprechen (→ sonst `403`). Operationen tragen die Quelle `mcp:<token-name>`.

**Fehler.** Protokollfehler sind JSON-RPC-Fehler: `-32700` (kein JSON, HTTP 400), `-32600` (keine gültige Nachricht, leerer Batch, `id` weder Text noch Zahl), `-32601` (unbekannte Methode), `-32602` (`params` kein Objekt, `name` fehlt, `arguments` kein Objekt, unbekanntes Werkzeug). Alle anderen Fehler eines Werkzeugs sind Ergebnisse mit `isError: true`, deutschem Text in `content` und `structuredContent: {"error":{"code","message"}}`. `code` ist `invalid_arguments` (unbekannter/fehlender Parameter, falscher Typ), `not_found`, `conflict` (fremde Reservierung, ID vergeben, keine laufende Flamme), ein Ablehnungscode des Kerns (Abschnitt 2.5, mit dessen deutscher Meldung) oder `internal`.

**Änderungen sind atomar.** Jeder Werkzeugaufruf erzeugt einen Push mit der Server-Uhr (Knoten `server`), der unter der Schreibsperre vorab validiert wird: Wird irgendeine Operation abgelehnt, wird nichts gespeichert (z. B. `decompose_task` legt alle oder keine Unteraufgaben an). Warnungen (`open_subtasks`) stehen im Ergebnis unter `warnings`. Werte, die der Kern prüft (Datum, Stelle, Links, Phönix …), werden unverändert durchgereicht, damit seine Meldungen ankommen. `update_*` schreibt nur Felder, deren Wert sich tatsächlich ändert.

**Darstellung einer Aufgabe** (`task` in Ergebnissen): `id`, `title`, `description`, `type`, `status`, `location` (wirksam, mit aufgelösten Namen: `{"kind":"project","id","name","area_id","area_name"}`, `{"kind":"area","id","name"}`, `{"kind":"inbox"}`, `{"kind":"loose"}`), `parent_id`, `due`, `remind_at`, `links`, `vault_links`, `phoenix`, `claimed_by`, `claim_expires_at`, `claim_active`, `plan`, `branch`, `pr_url`, `timing_ref`, `flame_started_at`, `flame_ended_at`, `flame_running`, `clickup_id`, `created_at`, `updated_at`, `status_changed_at` (RFC 3339 UTC aus den HLCs), `progress {done,total}`; bei `get_task` und `decompose_task` zusätzlich `subtasks` (Baum, je Knoten wieder mit `subtasks`).

**Werkzeuge im Detail** (Schemas: `crates/zunder-server/tests/snapshots/mcp_tools.json`):

- Adressierung: genau eines von `task_id` (UUID) oder `clickup_id`. Die `clickup_id` wird unter den lebenden Aufgaben gesucht, sonst über die deterministische Import-ID. Gelöschte Aufgaben gelten als nicht gefunden, außer bei `get_task_history`.
- `filter_tasks`: alle Filter mit UND; `area_id` = direkt im Bereich oder in einem seiner Projekte; `location` = Art der wirksamen Stelle; `due_before`/`due_after` inklusive, Vergleich nach Kalendertag, Aufgaben ohne `due` fallen dann heraus; `claimed_by` nur gültige Reservierungen; ohne `include_closed` gilt die 3-Tage-Regel; Unteraufgaben erscheinen wie andere Aufgaben (mit `parent_id`). `limit` Default 100, höchstens 500. Sortierung: `due` aufsteigend (ohne zuletzt), dann `created_at`. Ergebnis `{tasks, count, total, truncated}`.
- `create_task`: `title` Pflicht, dazu `description`, `type`, `status`, `location` (Default inbox), `parent_id`, `due`, `remind_at`, `links`, `vault_links`, `phoenix`, `plan`, `branch`, `pr_url`. `update_task`: dieselben Felder außer `location`/`parent_id`; `null` setzt zurück. Ergebnis `{task, warnings}`.
- `move_task`: nur `location` → an diese Stelle, von einer Elternaufgabe gelöst; `parent_id` (Text) → Unteraufgabe (eine zusätzlich angegebene `location` wird als eigene Stelle gespeichert); `parent_id: null` → eigenständig, behält ohne `location` die bisherige wirksame Stelle.
- `delete_task`: löscht samt Unteraufgaben (`OpBatch::delete_cascade`), Ergebnis `{deleted:[id], count}`.
- `search`: `query` Pflicht, `limit` Default 20 (höchstens 200); Treffer `{entity, id, title, score}`, bei Aufgaben zusätzlich `status`, `type`, `location`, `parent_id`, `due`, `clickup_id`, bei Projekten `area_id`, `area_name`, `status`, `kind`, bei Bereichen `state`.
- `claim_task`: `agent` Pflicht (≤ 200 Zeichen), `ttl_minutes` 1–10080, Default 120. Atomar unter der Schreibsperre: Fehler `conflict`, wenn ein anderer Agent eine nicht abgelaufene Reservierung hält; derselbe Agent verlängert. `release_task`: `agent` (Pflicht ohne `force`), `force`; fremde, gültige Reservierung nur mit `force`; Ergebnis mit `released` (false, wenn nichts reserviert war).
- `decompose_task`: `subtasks` 1–50 Einträge mit `title` (Pflicht), `description`, `type` (Default `aufgabe`), `status`, `due`, `remind_at`, `plan`; ein Push, alles oder nichts. Ergebnis `{task (mit Baum), created:[id], warnings}`.
- `list_structure`: `include_archived` (archivierte Bereiche, abgeschlossene Projekte); Ergebnis `{areas:[{…Bereich, open_tasks, projects:[{…Projekt, open_tasks}]}], inbox_open, loose_open}`, nach Namen sortiert.
- `create_area` (`name`, `state`, `vault_path`, optional `id` = vorhandene `zunder_id`; vergebene ID → `conflict`), `update_area` (`area_id`, `name`, `state`, `vault_path`), `create_project` (`name`, `area_id`, `status`, `kind`, `repo`, `description`, `view`, optional `id`), `update_project` (`project_id` + Felder; `repo`/`description`/`view` mit `null` zurücksetzbar).
- `get_task_history`: `limit` Default 100 (höchstens 1000, die neuesten); Einträge `{seq, field, value, hlc, source, received_at}`.

### 4.2 Flammen und Timing-Brücke

- `POST /api/tasks/{id}/flame/start|stop` (Session mit `X-Zunder-Client` oder Bearer-Token) und die Werkzeuge `start_flame`/`stop_flame` nutzen dieselbe Funktion (`zunder_server::flame`). Antwort: `{task, running, stopped_task_ids, unchanged, timing:{configured, used, entry, error}, warnings}`.
- Start schreibt immer `flame_started_at` (jetzt) und setzt `flame_ended_at` zurück; Stopp schreibt `flame_ended_at`. Eine Flamme läuft, solange `flame_started_at` gesetzt und `flame_ended_at` leer oder älter ist. Es läuft höchstens eine Flamme: Start beendet alle anderen laufenden Flammen (`stopped_task_ids`). Start einer schon laufenden Flamme ändert nichts (`unchanged: true`). Stopp ohne laufende Flamme → `409 conflict`.
- Mit `ZUNDER_TIMING_TOKEN`: Start ruft `POST https://web.timingapp.com/api/v1/time-entries/start` mit `{"title": <Aufgabentitel>, "project": ZUNDER_TIMING_PROJECT (falls gesetzt; Self-Link wie /projects/1 oder Titel), "replace_existing": false}` und speichert `data.self` (z. B. `/time-entries/2`) als `timing_ref` (Timing beendet einen laufenden Timer beim Start selbst; `replace_existing: false`, damit nie fremde Einträge gekürzt oder gelöscht werden). Stopp fragt `GET /time-entries/running` und ruft `PUT /time-entries/stop` nur, wenn der laufende Timer der eigene ist (ein in Timing inzwischen gestarteter fremder Timer bleibt unangetastet). `timing_ref` bleibt nach dem Stopp als Verweis erhalten.
- Scheitert Timing (nicht erreichbar, Fehlerstatus), läuft die Flamme trotzdem mit Zeitstempeln: `timing.used: false`, `timing.error` mit deutscher Meldung, `timing_ref` leer. Ohne Token: `timing.configured: false`.
- Flammen-Operationen tragen die Quelle `server` (Timing-Brücke), unabhängig davon, ob HTTP oder MCP sie ausgelöst hat.

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

### 5.1 Präzisierungen aus der Web-UI

Lücken, die beim Bau von `zunder-web` geschlossen wurden. Der Server darf mehr liefern, muss aber mindestens dies erfüllen.

- **Snapshot:** Ein Feld darf zusätzlich `op_id` tragen (`{"value","hlc","op_id"}`). Fehlt es, behandelt der Client die `op_id` als leer; bei identischem HLC gewinnt dann jede echte Operation.
- **Knoten-ID der Web-UI:** `web-<12 hex>`, je Browser in `localStorage` gespeichert; `client_id` im Push ist dieselbe Kennung. Die Push-Warteschlange liegt ebenfalls in `localStorage`, ein Reload verliert nichts (Idempotenz über `op_id`).
- **Tokens:** `GET /api/tokens` → `[{"id","name","created_at","last_used_at"}]`; `POST /api/tokens` mit `{"name"}` → dasselbe Objekt plus `"token"` (nur in dieser Antwort); `DELETE /api/tokens/{id}` → 204. Der Client akzeptiert auch `{"tokens":[…]}`.
- **Änderungsprotokoll:** `GET /api/tasks/{id}/history` → Array `[{"seq","op","source"}]` (auch `{"entries":[…]}` wird akzeptiert).
- **Flamme:** Der Antwortkörper von `flame/start|stop` wird ignoriert. Der Server schreibt die Änderungen als Operationen (`flame_started_at`, `flame_ended_at`, ggf. `timing_ref`); der Client holt sie per `pull`.
- **Abmelden:** `POST /auth/logout`, danach lädt die UI neu; `GET /api/me` mit 401 zeigt die Login-Seite (Knopf auf `/auth/login`).
- **Rang:** Aufgaben ohne `rank` sortieren nach einem Standardrang aus der Erstellzeit: `wall_ms` des kleinsten HLC in Basis 36, auf 9 Stellen mit `0` aufgefüllt, plus Suffix `i`. Erzeugte Ränge nutzen `[0-9a-z]` und enden nie auf `0`. Erledigte/verworfene Aufgaben stehen in Listen unten, neueste Statusänderung zuerst.
- **Kurzschreibweise:** Ein Wochentag allein meint das nächste Vorkommen (1–7 Tage voraus), `nächsten <Wochentag>` den Tag in der folgenden Kalenderwoche (Montag als Wochenbeginn). `12.10.` in der Vergangenheit meint das nächste Jahr. Eine Uhrzeit ohne Datum, die heute schon vorbei ist, meint morgen. `am` vor einem Datum und `um` vor einer Uhrzeit werden mit entfernt. Unbekannte `#tags` bleiben im Titel stehen und werden in der Vorschau markiert.
- **Inbox:** Erledigte oder verworfene Aufgaben verlassen die Inbox sofort (Ziel: null); die übrigen Sichten folgen der 3-Tage-Regel.

### 5.1 App-Befehle (Tauri, E4)

In der Tauri-App spricht die Web-UI nicht direkt mit dem Server (die CSP erlaubt nur `ipc:`), sondern über Befehle der App, die einen lokalen SQLite-Speicher mit Operations-Log (`pending` + `confirmed`), materialisiertem Zustand und Cursor führt und selbst synchronisiert. Ein künftiger `TauriTransport` der Web-UI ruft sie per `invoke` auf. Fehler: abgelehntes Promise mit `{"code","message"}`.

| Befehl | Argumente | Ergebnis |
|---|---|---|
| `local_snapshot` | – | `{"cursor","entities":[SnapshotEntity],"pending":N}` (Format von `GET /api/snapshot`, lokale Sicht = bestätigt + ausstehend, inkl. Tombstones) |
| `local_apply` | `{"ops":[Op]}` | `{"accepted":[op_id],"rejected":[Rejection],"warnings":[Warning],"pending":N}`; lokal per `validate_push` geprüft, angenommene neue Operationen sofort sichtbar und zum Push vorgemerkt |
| `sync_now` | – | `{"pushed","rejected":[Rejection],"warnings":[Warning],"pulled","cursor","pending","changed"}` |
| `get_config` | – | `{"server_url"\|null,"has_api_token","node_id","cursor","pending","last_sync":{"at_ms","ok","error"?}\|null}` (Token wird nie zurückgegeben) |
| `set_config` | `{"update":{"server_url"?,"api_token"?,"discard_pending"?}}` | wie `get_config`; fehlender Schlüssel = unverändert, `null` = löschen |

- Sync-Ablauf: ausstehende Operationen in Paketen zu 500 pushen (`client_id` = `node_id`), bei Cursor 0 den Snapshot übernehmen, dann `pull` bis `has_more = false`. Abgelehnte Operationen werden lokal zurückgenommen (Sicht = bestätigter Zustand + übrige ausstehende) und gemeldet.
- Auslöser: Start, alle 30 s, 0,5 s nach `local_apply`, Rückkehr in den Vordergrund, `online`-Ereignis des Webviews, `sync_now`; nach Netzwerkfehlern 5/10/20 s, dann wieder 30 s.
- Die Web-UI nimmt `node_id` (Form `app-<12 hex>`) als Knoten ihrer HLC.
- Serverwechsel löscht die lokalen Daten; mit ausstehenden Operationen nur mit `discard_pending: true` (sonst Fehler `pending_ops`).
- Fehlercodes: `not_configured`, `invalid_config`, `pending_ops`, `network`, `unauthorized`, `server`, `protocol`, `db`, `corrupt`, `schema_too_new`, `notification`.
- Ereignisse: `zunder://changed` `{"cursor","pending"}` (Sicht durch Sync geändert → `local_snapshot` neu laden), `zunder://sync` `{"ok":true,"report"}` bzw. `{"ok":false,"error":{"code","message"}}`.
- Erinnerungen: für offene, lebende Aufgaben zu `remind_at` und zu `due` mit Uhrzeit (gleicher Moment → nur eine). Mobil plant das Betriebssystem die 64 nächsten (iOS-Grenze) über `tauri-plugin-notification`; Desktop zeigt sie per Timer, solange die App läuft.

Einzelheiten: [`apps/zunder-app/README.md`](../../../apps/zunder-app/README.md).

## 6. Import

`zunder-import` (CLI, clap): `plan` (Probelauf → `mapping.toml`), `report` (Mengen, unbekannte Status, Aufgaben ohne Ziel), `run --mapping mapping.toml` (schreibt über `POST /api/sync/push` mit API-Token, Knoten `import`), `backup --out <dir>` (Roh-JSON inkl. Zeiteinträgen, Standard-Ziel außerhalb des Repos). Idempotent über `clickup_id` (erneuter Lauf aktualisiert statt dupliziert; IDs werden deterministisch aus der `clickup_id` abgeleitet: UUIDv5 im Zunder-Namespace oder Lookup).

Präzisierungen (umgesetzt in `crates/zunder-import`, Bedienung im README der Crate):

- **Umgebung:** `CLICKUP_TOKEN` (persönlicher Token, Header `Authorization: <token>`; `--token` bei `plan`/`report`/`backup`, `--clickup-token` bei `run`), `ZUNDER_URL` und `ZUNDER_TOKEN` (bzw. `run --server`, `--token`). Pushes tragen `client_id: "import"` und den Header `X-Zunder-Client: import`; Standard-Paketgröße 500 Operationen.
- **Deterministische IDs** (`clickup_entity_id(kind, clickup_id)`): Aufgabe ← ClickUp-Task-ID; Bereich ← Space-ID; Projekt ← Listen-ID bzw. `folder-<Ordner-ID>`, wenn ein ClickUp-Ordner das Projekt bildet. Bereiche/Projekte ohne `clickup_id` in der Zuordnungsdatei: `deterministic_id("import:<entity>:<schlüssel>")`. Mit `existing_id` wird auf einen bestehenden Eintrag abgebildet und nichts dafür geschrieben.
- **Zuordnungsdatei** (`version = 1`, TOML, unbekannte Schlüssel werden abgelehnt): `team_id`, `[settings]` (`timezone`, Default `Europe/Berlin`; `import_archived_tasks`, Default `false`), `[type_tags]` (ClickUp-Tag klein → `bug`/`feature`), `[areas.<schlüssel>]` (`name`, `clickup_id`, `existing_id`, `state`), `[projects.<schlüssel>]` (`name`, `area`, `kind`, `repo`, `status`, `clickup_id`, `existing_id`), `[lists."<Listen-ID>"]` (`path` nur zur Info, `target` = `area:<k>` | `project:<k>` | `inbox` | `loose` | `skip`, leer = offen; `[lists."<id>".statuses]` ClickUp-Statusname klein → Zunder-Status, leer = offen). Offene Status und Listen ohne Ziel blockieren `run`.
- **Felder einer importierten Aufgabe:** `title` (getrimmt; leer → „(ohne Titel)“, über 500 Zeichen gekürzt), `description` (`markdown_description`, sonst `text_content`, sonst `description`), `type` (aus `type_tags`), `status` (Zuordnung je Liste), `location` (Ziel der eigenen Liste, auch bei Unteraufgaben), `parent_id`, `due`, `links` (`[{"url": <ClickUp-URL>, "title": "ClickUp"}]`), `clickup_id`, `rank`. `bug`/`feature` bzw. `review` außerhalb eines Dev-Projekts (maßgeblich ist die Wurzel-Aufgabe) werden zu `aufgabe` bzw. `in_progress` und im Bericht gelistet.
- **Fälligkeit:** `due_date` (Unix-ms) in der Zeitzone der Zuordnung; `due_date_time = true` → RFC 3339 mit Offset (Sekunden), `false` → `YYYY-MM-DD`; fehlt das Flag, gilt 00:00 oder 04:00 Ortszeit als reines Datum.
- **Rang:** `orderindex` als Dezimalzahl fester Breite, 13 Vor- und 6 Nachkommastellen (`1.5` → `"0000000000001.500000"`, negativ → 0). Andere Clients müssen beim Einsortieren nur die lexikografische Ordnung erhalten.
- **Unteraufgaben:** beliebige Tiefe; Eltern außerhalb der geladenen Listen (archiviert, andere Liste) werden per `GET /task/{id}` nachgeladen und mit importiert. Fehlt die Eltern-Aufgabe oder wird sie nicht importiert (`skip`), wird die Unteraufgabe eigenständige Aufgabe ihrer Liste. Archivierte Aufgaben sonst nur mit `import_archived_tasks`.
- **Erneuter Lauf:** `run` liest `GET /api/snapshot` und schreibt nur abweichende Felder. Der Import besitzt die Aufgabenfelder `title`, `description`, `type`, `status`, `location`, `parent_id`, `due`, `links`, `clickup_id`, `rank` (ClickUp gewinnt; in ClickUp leer gewordene werden auf `null` gesetzt), bei Bereichen `name`, `state` und bei Projekten `name`, `area_id`, `status`, `kind`, `repo`. Andere Felder bleiben unberührt; in Zunder gelöschte Entitäten werden übersprungen. Die Import-Uhr wird vorher per `receive` über alle HLCs des Snapshots gehoben.
- **Vorprüfung:** Vor dem ersten Push prüft `run` alle Operationen lokal mit `validate_push` gegen den Snapshot. Bei Ablehnungen wird ohne `--keep-going` nichts geschrieben; serverseitige Ablehnungen brechen nach dem jeweiligen Paket ab. Pakete teilen nie die Operationen einer Entität und halten die Reihenfolge Bereiche → Projekte → Aufgaben (Eltern vor Kindern).
- **Backup:** Basis Standard `~/zunder-import-backup`, darin `<JJJJ-MM-TT_hh-mm-ss>/` (Rechte 0700) mit `manifest.json`, `teams.json`, `teams/<id>/spaces.json`, `teams/<id>/time_entries.json`, `spaces/<id>/folders.json`, `spaces/<id>/lists.json`, `folders/<id>/lists.json`, `lists/<id>/list.json`, `lists/<id>/tasks.json` (aktiv und archiviert), `tasks/parents.json`. Zeiteinträge ab `--since` (Default 2017-01-01) in Fenstern von `--window-days` (Default 30), dedupliziert nach ID. Ziele in einem Git-Repository (Suche nach `.git` aufwärts) werden ohne `--force` abgelehnt.
- **Rate-Limit:** HTTP 429 → warten bis `X-RateLimit-Reset` (Unix-Sekunden, höchstens 120 s) bzw. `Retry-After`, dann erneut; bei `X-RateLimit-Remaining: 0` wird vor der nächsten Anfrage bis zum Reset gewartet; 5xx und Netzwerkfehler mit Backoff, insgesamt höchstens 8 Versuche.

## 7. Etappen im Repo

E1–E2 werden vollständig gebaut. E3 ist überwiegend Betrieb (Runbook in `docs/umstieg.md`). E4 (Tauri) wird als lauffähiges Gerüst angelegt, das `zunder-core` für lokale SQLite und Sync nutzt; Mobile-Builds werden in dieser Umgebung nicht verifiziert. Speicher, Sync und Erinnerungsplanung liegen in der webview-freien Crate `apps/zunder-app/src-tauri/zunder-local` (zweites Mitglied des eigenen Workspaces von `src-tauri`), die Tauri-Hülle verdrahtet sie nur (Befehle: Abschnitt 5.1).
