# zunder-server

Der Server von Zunder: Sync-API, Login (OIDC und API-Tokens), SQLite-Persistenz, nächtliches Backup und Auslieferung der Web-UI. Verbindlich ist der [Implementierungsvertrag](../../docs/superpowers/plans/2026-10-11-zunder-implementation.md), Abschnitt 3.

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
| `db` | SQLite (WAL, Fremdschlüssel, `busy_timeout`), eingebettete Migrationen über `user_version`, alle Abfragen |
| `app` | Anwendungsdienst `App`: `push`, `mutate` (serverseitige Operationen), `snapshot`, `pull`, `task_history`, Tokens, Sitzungen, Backup |
| `auth` | Principal, Token- und Sitzungs-Hashes, CSRF-Regel, Middleware `require_auth`, OIDC (`auth::oidc`) |
| `http` | Router: `/healthz`, `/auth/*`, `/api/*`, statische Dateien mit SPA-Fallback |
| `backup` | nächtliches `VACUUM INTO` nach `data/backups/`, 14 Stück |

Alles, was Daten liest oder ändert, läuft über `App`. Schreibzugriffe sind serialisiert: eine Schreibverbindung hinter einem Mutex, ausgeführt in `spawn_blocking`. Validierung (`zunder_core::validate_push`), Speichern von Operationen und materialisierten Entitäten in einer Transaktion und das Anwenden auf den Zustand im Speicher passieren unter dieser Sperre. Danach geht der neue Cursor per Broadcast an die SSE-Verbindungen.

## Erweitern (MCP, Timing)

- Neue Routen hängen in `http::router` ein. Geschützte Routen nutzen `auth::middleware::require_auth` (oder `bearer_principal` für reine Token-Auth) und den Extraktor `auth::Principal`.
- Serverseitige Änderungen laufen über `App::mutate(source, |state, batch| …)`: Der Closure sieht den aktuellen Zustand und füllt einen `OpBatch` mit der Server-Uhr (Knoten `server`). Das Ergebnis durchläuft dieselbe Validierung und dasselbe Log; Ablehnungen stehen im `PushResult`.
- Lesend: `App::read(|state| …)`, `App::task_history(id)`.
- Die Quelle einer Änderung ist `Source::Web`, `Source::Api(name)`, `Source::Mcp(name)` oder `Source::Server`; für MCP liefert `Principal::mcp_source()` die passende Quelle.

## Tests

```sh
cargo test -p zunder-server
```

- `tests/api.rs`: HTTP-API über `tower::ServiceExt::oneshot` mit temporärem Datenverzeichnis (Push/Pull/Snapshot, Idempotenz, Ablehnungen, Warnungen, Paging, SSE, Tokens, Sitzungen, CSRF, Dev-Auth, Verlauf, Neustart und Wiederaufbau, Backup, Body-Limits, Kompression, statische Dateien).
- `tests/oidc.rs`: kompletter Login gegen einen lokalen Mock-Anbieter (Discovery, JWKS, Token-Endpunkt) mit nur für Tests erzeugten RSA-Schlüsseln; Signatur, Issuer, Audience, Ablauf, Nonce, PKCE, Schlüsselrotation, erlaubte Person.
- `tests/process.rs`: das echte Binary mit `healthcheck` und sauberem Herunterfahren bei SIGTERM trotz offener SSE-Verbindung.
