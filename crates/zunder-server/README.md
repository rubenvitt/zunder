# zunder-server

Der Server von Zunder: Sync-API, Login (OIDC und API-Tokens), SQLite-Persistenz, MCP-Endpunkt für Agenten mit eingebautem OAuth-Authorization-Server (für claude.ai), Timing-Brücke für Flammen, nächtliches Backup und Auslieferung der Web-UI. Verbindlich ist der [Implementierungsvertrag](../../docs/superpowers/plans/2026-10-11-zunder-implementation.md), Abschnitte 3 und 4 (OAuth: 4.3).

## Starten

```sh
# lokal ohne Anmeldedienst (jede Anfrage gilt als angemeldet, nur auf localhost erlaubt)
ZUNDER_DEV_AUTH=true cargo run -p zunder-server

# Gesundheitsprüfung (für Docker HEALTHCHECK), Exit-Code 0 = gesund
cargo run -p zunder-server -- healthcheck
```

Alle Einstellungen kommen aus `ZUNDER_*`-Umgebungsvariablen (Tabelle im Vertrag). Das Log steuert `RUST_LOG` (Standard `info`). Für das Backup zur lokalen Stunde braucht der Container eine Zeitzone (`TZ=Europe/Berlin` und `tzdata`).

## Aufbau

| Modul | Inhalt |
|---|---|
| `config` | Umgebungsvariablen lesen und prüfen |
| `settings` | Einstellungen aus der Web-UI (Timing-Token und -Projekt), die die Umgebungsvariablen Feld für Feld überschreiben |
| `db` | SQLite (WAL, Fremdschlüssel, `busy_timeout`), eingebettete Migrationen über `user_version`, alle Abfragen |
| `app` | Anwendungsdienst `App`: `push`, `mutate` (serverseitige Operationen), `snapshot`, `pull`, `task_history`, Tokens, Sitzungen, Backup |
| `auth` | Principal, Token- und Sitzungs-Hashes, CSRF-Regel, Middleware `require_auth`, OIDC (`auth::oidc`) |
| `http` | Router: `/healthz`, `/auth/*`, `/api/*` (inkl. `/api/tasks/{id}/flame/start\|stop`), `/mcp`, statische Dateien mit SPA-Fallback |
| `mcp` | MCP über Streamable HTTP: JSON-RPC 2.0 (`mcp`), Werkzeug-Schemas (`mcp::schema`), Werkzeuge (`mcp::tools`), Argumentprüfung (`mcp::args`) |
| `oauth` | OAuth-2.1-Authorization-Server für `/mcp`: Registrierung, Codes, Tokens, Rotation, Widerruf (`oauth`); Metadaten, Freigabeseite und Endpunkte (`oauth::routes`) |
| `views` | Darstellung von Aufgaben (wirksame Stelle mit Namen, abgeleitete Zeitpunkte, Fortschritt, Unteraufgaben-Baum), Bereichen und Projekten; Auflösen von `task_id`/`clickup_id` |
| `flame` | Flamme starten/beenden (gemeinsam für HTTP und MCP), immer mit Fallback-Zeitstempeln |
| `timing` | Client der Timing-Web-API hinter dem Trait `TimingApi` |
| `backup` | nächtliches `VACUUM INTO` nach `data/backups/`, 14 Stück |

Alles, was Daten liest oder ändert, läuft über `App`. Schreibzugriffe sind serialisiert: eine Schreibverbindung hinter einem Mutex, ausgeführt in `spawn_blocking`. Validierung (`zunder_core::validate_push`), Speichern von Operationen und materialisierten Entitäten in einer Transaktion und das Anwenden auf den Zustand im Speicher passieren unter dieser Sperre. Danach geht der neue Cursor per Broadcast an die SSE-Verbindungen.

## MCP

Der MCP-Server läuft unter `/mcp` (Streamable HTTP, JSON-RPC 2.0, Protokoll `2025-06-18`, akzeptiert auch `2025-03-26`). Er ist zustandslos, antwortet immer mit `application/json` und braucht einen Bearer-Token: einen API-Token (`zt_…`) oder einen OAuth-Access-Token (`zoa_…`, siehe unten); Browser-Sitzungen gelten hier nicht. Änderungen erscheinen im Verlauf mit der Quelle `mcp:<token-name>` bzw. `mcp:oauth:<client_name>`. Ein `401` nennt in `WWW-Authenticate` die Protected Resource Metadata, damit OAuth-fähige Clients den Anmeldeweg finden.

Token anlegen (Web-UI unter „Tokens“ oder per API mit Browser-Sitzung), dann in Claude Code verbinden:

```sh
claude mcp add --transport http zunder https://zunder.example.org/mcp \
  --header "Authorization: Bearer zt_…"
```

Für andere Clients gilt dasselbe: URL `<ZUNDER_PUBLIC_URL>/mcp`, Header `Authorization: Bearer zt_…`. Schickt ein Client einen `Origin`-Header, muss er zu `ZUNDER_PUBLIC_URL` passen.

Werkzeuge: `filter_tasks`, `get_task`, `create_task`, `update_task`, `move_task`, `delete_task`, `search`, `claim_task`, `release_task`, `decompose_task`, `list_structure`, `create_area`, `update_area`, `create_project`, `update_project`, `start_flame`, `stop_flame`, `get_task_history`. Jedes Werkzeug, das eine Aufgabe adressiert, nimmt `task_id` oder `clickup_id`. Ergebnisse enthalten `structuredContent` und denselben Inhalt als kompakten JSON-Text; Fehler kommen als Ergebnis mit `isError: true` und deutscher Meldung (Ablehnungen des Kerns mit dessen Code und Text). Jeder Aufruf ist atomar: Wird etwas abgelehnt, wird nichts gespeichert. Details und Parameter: Vertrag Abschnitt 4.1, Schemas in `tests/snapshots/mcp_tools.json`.

Schnelltest mit curl:

```sh
curl -s https://zunder.example.org/mcp \
  -H "Authorization: Bearer zt_…" -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_structure","arguments":{}}}'
```

## OAuth für claude.ai

claude.ai kann bei benutzerdefinierten Connectors keinen festen Header schicken, sondern verbindet sich per OAuth nach der MCP-Authorization-Spezifikation. Zunder ist dafür selbst der Authorization-Server (Modul `oauth`, Vertrag Abschnitt 4.3):

1. claude.ai ruft `/mcp` ohne Token auf und bekommt `401` mit `WWW-Authenticate: Bearer resource_metadata="<ZUNDER_PUBLIC_URL>/.well-known/oauth-protected-resource"`.
2. Discovery: `/.well-known/oauth-protected-resource` (Ressource `<ZUNDER_PUBLIC_URL>/mcp`, Authorization-Server `<ZUNDER_PUBLIC_URL>`), dann `/.well-known/oauth-authorization-server`.
3. Dynamische Registrierung `POST /oauth/register` als öffentlicher Client (Redirect-URI `https://claude.ai/api/mcp/auth_callback`; Loopback-URIs für lokale Werkzeuge passen mit jedem Port).
4. `GET /oauth/authorize` mit PKCE (S256) und `resource`: ohne Sitzung geht es über den Pocket-ID-Login (bzw. Dev-Auth) und zurück; dann zeigt Zunder eine Freigabeseite mit Client-Name und Ziel-Host.
5. Nach „Erlauben“ geht ein 5 Minuten gültiger Einmal-Code an claude.ai, `POST /oauth/token` tauscht ihn gegen Access-Token (1 Stunde) und Refresh-Token (30 Tage, rotierend; Wiederverwendung widerruft die ganze Familie).

Codes und Tokens liegen nur als SHA-256 in SQLite (`oauth_clients`, `oauth_codes`, `oauth_tokens`, Schema 2). Access-Tokens gelten nur für `/mcp` und nur für die eigene Ressource. Verbundene Apps listet `GET /api/oauth/clients`, `DELETE /api/oauth/clients/{id}` trennt sie (Web-UI: Token-Seite, „Verbundene Apps“). `POST /oauth/revoke` widerruft einzelne Tokens (RFC 7009).

## Flammen und Timing

`POST /api/tasks/{id}/flame/start|stop` und die Werkzeuge `start_flame`/`stop_flame` teilen sich `flame::start`/`flame::stop`. Eine Flamme speichert immer `flame_started_at`/`flame_ended_at`; es läuft höchstens eine. Mit einem Timing-Token (API-Key aus der Timing-Web-App, in der Web-UI unter „Einstellungen“ oder per `ZUNDER_TIMING_TOKEN`) startet zusätzlich ein Timer in Timing (`POST /time-entries/start`, Titel = Aufgabentitel, Projekt aus den Einstellungen oder `ZUNDER_TIMING_PROJECT`, z. B. `/projects/1` oder ein Projekttitel); dessen Self-Link landet in `timing_ref`. Beim Beenden stoppt Zunder den Timer nur, wenn er noch der laufende ist. Ist Timing nicht erreichbar, läuft die Flamme mit Zeitstempeln weiter; die Antwort sagt unter `timing`, ob Timing genutzt wurde und warum nicht. Flammen-Operationen tragen die Quelle `server`.

Einstellungen: `GET`/`PUT`/`DELETE /api/settings/timing` und `POST /api/settings/timing/check` sowie `GET /api/settings/timing/projects` für die Projektauswahl (nur Browser-Sitzung). Gespeicherte Werte liegen in der Tabelle `settings` (Schema 3) und überschreiben die Umgebungsvariablen Feld für Feld; `App` hält die daraus gebaute Brücke und tauscht sie beim Speichern aus, die nächste Flamme nutzt sie ohne Neustart. Details im Vertrag, Abschnitt 4.2.

## Erweitern

- Neue Routen hängen in `http::router` ein. Geschützte Routen nutzen `auth::middleware::require_auth` (oder `bearer_principal` für reine Token-Auth) und den Extraktor `auth::Principal`.
- Serverseitige Änderungen laufen über `App::mutate(source, |state, batch| …)`: Der Closure sieht den aktuellen Zustand und füllt einen `OpBatch` mit der Server-Uhr (Knoten `server`). Das Ergebnis durchläuft dieselbe Validierung und dasselbe Log; Ablehnungen stehen im `PushResult`.
- `App::mutate_atomic(source, …)` ist die Alles-oder-nichts-Variante (genutzt von MCP und Flammen): Der Batch wird vorab gegen denselben Zustand validiert; jede Ablehnung bricht ihn als Fehler mit Code und Meldung des Kerns ab.
- Lesend: `App::read(|state| …)`, `App::task_history(id)`.
- Die Quelle einer Änderung ist `Source::Web`, `Source::Api(name)`, `Source::Mcp(name)` oder `Source::Server`; für MCP liefert `Principal::mcp_source()` die passende Quelle.

## Tests

```sh
cargo test -p zunder-server

# nach einer absichtlichen Änderung an Werkzeugen oder Schemas (Vertrag mit anpassen!)
UPDATE_SNAPSHOTS=1 cargo test -p zunder-server --test mcp tool_schemas_match_snapshot
```

- `tests/api.rs`: HTTP-API über `tower::ServiceExt::oneshot` mit temporärem Datenverzeichnis (Push/Pull/Snapshot, Idempotenz, Ablehnungen, Warnungen, Paging, SSE, Tokens, Sitzungen, CSRF, Dev-Auth, Verlauf, Neustart und Wiederaufbau, Backup, Body-Limits, Kompression, statische Dateien).
- `tests/oidc.rs`: kompletter Login gegen einen lokalen Mock-Anbieter (Discovery, JWKS, Token-Endpunkt) mit nur für Tests erzeugten RSA-Schlüsseln; Signatur, Issuer, Audience, Ablauf, Nonce, PKCE, Schlüsselrotation, erlaubte Person.
- `tests/oauth.rs`: OAuth-Ablauf komplett (Registrierung → Freigabe mit Sitzung → Code → Token mit PKCE → `tools/list` → Rotation → Wiederverwendung → Widerruf) und die Gegenfälle (falscher Verifier, falsche Redirect-URI, abgelaufener/wiederverwendeter Code, Freigabe ohne bzw. mit fremder Sitzung oder fremdem Origin, fremde Ressource, Audience nach Wechsel der öffentlichen URL, Registrierungsregeln und -grenze, Loopback-Ports, HTML-Escaping, Dev-Auth). Der Umweg über den echten OIDC-Login steht in `tests/oidc.rs`.
- `tests/mcp.rs`: MCP über HTTP mit Bearer-Token: `initialize` (Versionsaushandlung), Benachrichtigungen, `ping`, Batches, JSON-RPC-Fehler, Auth (kein Token, falscher/widerrufener Token, Cookie, Origin, Protokollversion, `GET`/`DELETE`), Snapshot der Werkzeug-Schemas, jedes Werkzeug inkl. `clickup_id`, Reservierungskonflikten, atomarem `decompose_task`, Suche nach geschlossenen Aufgaben, Filtern, Verschieben, Löschen und Kernmeldungen.
- `tests/flame.rs`: Flammen per MCP und HTTP gegen einen lokalen Mock der Timing-API (Anfrageform, Token, Redirect bei `running`, fremder Timer bleibt laufen, Fehlerstatus, nicht erreichbar) und ohne Timing (Fallback-Zeitstempel, CSRF, 409); Timing-Einstellungen aus der Web-UI (nur Sitzung, Validierung, sofort wirksam, Neustart, Vorrang vor und Rückfall auf die Umgebung).
- `tests/process.rs`: das echte Binary mit `healthcheck` und sauberem Herunterfahren bei SIGTERM trotz offener SSE-Verbindung.
