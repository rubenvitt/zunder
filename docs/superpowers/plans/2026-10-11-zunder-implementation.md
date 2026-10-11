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
deploy/                    compose.yaml, stack.env, generate-secrets.sh (Portainer/Traefik)
Dockerfile                 Multi-Stage: Web bauen, Server bauen, schlankes Runtime-Image
.github/workflows/ci.yml   fmt, clippy -D warnings, Tests, Web, Playwright, GHCR
docs/                      Spec, dieser Vertrag, Betrieb, Umstieg
```

Code-Bezeichner sind englisch, Doku und UI deutsch.

## 2. Wire-Format

### 2.1 IDs

UUIDv7 als kleingeschriebener Bindestrich-String. Clients erzeugen IDs selbst.

### 2.2 Hybrid Logical Clock

String `"<wall_ms>:<counter>:<node>"`:

- `wall_ms`: Unix-Millisekunden, **13 Stellen**, mit Nullen aufgefüllt
- `counter`: **5 Stellen**, dezimal, mit Nullen aufgefüllt (0–99999)
- `node`: Knoten-ID, `[a-z0-9-]{1,36}` (z. B. `server`, `web-3f9a…`, `import`)

Ordnung = **lexikografischer Vergleich des ganzen Strings** (durch feste Breiten identisch mit Tupelvergleich). Gleichstand ist bei verschiedenen Knoten unmöglich; bei identischem HLC entscheidet `op_id` lexikografisch (größer gewinnt).

Regeln: `now()` liefert `max(physisch, letzter)`; gleicher `wall_ms` → `counter + 1`. `receive(remote)` hebt die lokale Uhr auf `max(lokal, remote)` an. Der Server lehnt Operationen ab, deren `wall_ms` mehr als 24 h in der Zukunft liegt.

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
- Löschen = Feld `deleted` mit Wert `true` (Tombstone). Gelöschte Entitäten bleiben im Zustand, werden aber nirgends angezeigt.
- `value: null` setzt ein optionales Feld zurück.
- Merge: **Last-Writer-Wins pro (Entität, Feld)** nach HLC, Gleichstand nach `op_id`. Merge ist kommutativ, assoziativ und idempotent.

### 2.4 Felder

**area**: `name` (String, Pflicht), `state` (`active`|`archived`, Default `active`), `vault_path` (optional, nur Info), `deleted`.

Die Area-`id` **ist** die `zunder_id` im Vault-Frontmatter. Gleiches gilt für Projekte.

**project**: `name` (Pflicht), `area_id` (Pflicht, existierende Area), `status` (`active`|`paused`|`completed`, Default `active`), `kind` (`standard`|`dev`, Default `standard`), `repo` (`owner/name`, nur bei `dev`, optional), `description` (Markdown), `view` (`list`|`board`, Default: `board` bei dev, sonst `list`), `deleted`.

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
| `vault_links` | Array von Strings (Vault-Pfade oder `obsidian://`-URIs) |
| `links` | Array von `{"url": String, "title"?: String}` |
| `phoenix` | `null` oder `{"role":"feuer_schritt"\|"flamme","fire":"2026-W45"}` |
| `claimed_by`, `claim_expires_at` | String, RFC 3339 |
| `plan` | Markdown-String |
| `branch` | String |
| `pr_url` | URL-String |
| `timing_ref` | String (ID des Timing-Eintrags) |
| `flame_started_at`, `flame_ended_at` | RFC 3339 (Fallback, Abschnitt 6 der Spec) |
| `clickup_id` | String |
| `rank` | String, lexikografisch sortierbar (Reihenfolge in Liste/Spalte) |
| `deleted` | `true` |

Unteraufgaben: Die wirksame Stelle einer Unteraufgabe ist die Stelle ihrer Wurzel-Aufgabe; das eigene `location` einer Unteraufgabe wird ignoriert.

Abgeleitet (nicht als Feld gespeichert): `created_at` (kleinster HLC der Entität), `updated_at` (größter HLC), `status_changed_at` (HLC der gewinnenden `status`-Operation), Fortschritt der Unteraufgaben (`done`/`gesamt`, `dropped` zählt nicht mit).

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

`expected` enthält je Entität die gewinnenden Werte aller gesetzten Felder (`null`-Werte erscheinen als `null`). Eine Datei mit `"kind": "hlc_order"` enthält statt `ops`/`expected` das Feld `"sorted": [hlc, …]` in aufsteigender Ordnung. Rust- und TypeScript-Tests wenden die Operationen in mehreren Permutationen an und vergleichen mit `expected`.

## 3. HTTP-API (Server)

Basis: `ZUNDER_PUBLIC_URL`. JSON, UTF-8. Fehler: `{"error":{"code":…,"message":…}}`.

| Methode, Pfad | Zweck |
|---|---|
| `GET /healthz` | `{"status":"ok","version":…}`, ohne Auth |
| `GET /auth/login` | OIDC-Login (Authorization Code + PKCE) starten |
| `GET /auth/callback` | OIDC-Callback, setzt Session-Cookie `zunder_session` (HttpOnly, Secure, SameSite=Lax) |
| `POST /auth/logout` | Session beenden |
| `GET /api/me` | `{"user_id","name","email"}` |
| `GET /api/snapshot` | `{"cursor":N,"entities":[{"entity","entity_id","fields":{f:{"value","hlc"}}}]}` |
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

**Persistenz:** SQLite im WAL-Modus. Tabellen mindestens `users`, `ops` (seq, op_id unique, entity, entity_id, field, value_json, hlc, source, received_at, owner_id), `entities` (materialisierter Zustand je Entität als JSON inkl. Feld-HLCs), `api_tokens`, `sessions`. Der Server hält den materialisierten Zustand zusätzlich im Speicher. Nächtliches Backup per `VACUUM INTO` nach `data/backups/`, 14 Stück aufbewahren.

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

## 6. Import

`zunder-import` (CLI, clap): `plan` (Probelauf → `mapping.toml`), `report` (Mengen, unbekannte Status, Aufgaben ohne Ziel), `run --mapping mapping.toml` (schreibt über `POST /api/sync/push` mit API-Token, Knoten `import`), `backup --out <dir>` (Roh-JSON inkl. Zeiteinträgen, Standard-Ziel außerhalb des Repos). Idempotent über `clickup_id` (erneuter Lauf aktualisiert statt dupliziert; IDs werden deterministisch aus der `clickup_id` abgeleitet: UUIDv5 im Zunder-Namespace oder Lookup).

## 7. Etappen im Repo

E1–E2 werden vollständig gebaut. E3 ist überwiegend Betrieb (Runbook in `docs/umstieg.md`). E4 (Tauri) wird als lauffähiges Gerüst angelegt, das `zunder-core` für lokale SQLite und Sync nutzt; Mobile-Builds werden in dieser Umgebung nicht verifiziert.
