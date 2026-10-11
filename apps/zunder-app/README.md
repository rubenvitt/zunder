# zunder-app

Zunder als Tauri-2-App für Desktop (Linux, macOS, Windows), iOS und Android (Etappe E4 der [Spec](../../docs/superpowers/specs/2026-10-11-zunder-design.md)). Die App zeigt die Web-UI aus `web/` und bringt einen lokalen SQLite-Speicher mit: Änderungen landen sofort lokal, der Sync mit dem Server läuft im Hintergrund, Erinnerungen plant das Gerät selbst. Die Web-UI erkennt die App und spricht dann nur noch über die Befehle unten mit ihr (siehe [Web-UI in der App](#web-ui-in-der-app)).

Verbindlich ist der [Implementierungsvertrag](../../docs/superpowers/plans/2026-10-11-zunder-implementation.md), insbesondere Abschnitt 2 (Operationen), 3 (Sync-API) und 5.1 (App-Befehle).

## Aufbau

```
apps/zunder-app/
  package.json            @tauri-apps/cli (npm run dev / build / icons)
  icon.svg                Quelle der App-Icons (npm run icons)
  src-tauri/              eigener Cargo-Workspace, nicht im Root-Workspace
    Cargo.toml            Workspace [".", "zunder-local"] + Paket zunder-app
    tauri.conf.json       Identifier dev.rubeen.zunder, Frontend = web/dist
    capabilities/         Rechte: core:default, notification:default
    icons/                generierte Icons (Desktop, iOS, Android)
    src/lib.rs            Einstieg (auch mobil: tauri::mobile_entry_point)
    src/commands.rs       Befehle für die Web-UI
    src/background.rs     Sync-Schleife und Ereignisse
    src/notify.rs         Erinnerungen (mobil: OS-Planer, Desktop: Timer)
    zunder-local/         Speicher, Sync, Erinnerungsplanung – ohne Webview testbar
```

`zunder-local` hängt nur an `zunder-core`, `rusqlite` (bundled) und `reqwest` (rustls). So laufen Speicher-, Sync- und Erinnerungstests auch ohne webkit2gtk, etwa in der CI.

## Voraussetzungen

- Rust (stabil), Node 22 mit npm
- Linux: `libwebkit2gtk-4.1-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev librsvg2-dev libayatana-appindicator3-dev build-essential`
- macOS/iOS: Xcode; Android: Android Studio mit SDK und NDK (`ANDROID_HOME`, `NDK_HOME`)

## Entwickeln und bauen

```sh
cd apps/zunder-app
npm install
npm run dev      # startet Vite in web/ (beforeDevCommand) und die App gegen http://localhost:5173
npm run build    # baut web/dist (beforeBuildCommand) und das Desktop-Bundle
```

Nur den Rust-Teil prüfen:

```sh
cd apps/zunder-app/src-tauri
cargo test -p zunder-local                       # ohne webkit
cargo clippy --workspace --all-targets -- -D warnings
```

Ende-zu-Ende-Test gegen den echten Server (ignoriert, solange kein Binary angegeben ist):

```sh
cargo build -p zunder-server                     # im Repo-Root
ZUNDER_SERVER_BIN=$PWD/target/debug/zunder-server \
  cargo test --manifest-path apps/zunder-app/src-tauri/Cargo.toml \
  -p zunder-local --test server_e2e -- --ignored
```

## Mobil (iOS, Android)

Die nativen Projekte werden nicht eingecheckt, sondern einmalig erzeugt (landen in `src-tauri/gen/`):

```sh
npx tauri android init
npx tauri ios init          # nur auf macOS
npx tauri android dev       # bzw. ios dev; braucht ein Gerät oder einen Emulator
npx tauri android build     # bzw. ios build
```

Hinweise:

- **Android:** Das Benachrichtigungs-Plugin bringt `POST_NOTIFICATIONS`, `RECEIVE_BOOT_COMPLETED` und `WAKE_LOCK` mit; geplante Benachrichtigungen werden nach einem Neustart wiederhergestellt. Für minutengenaue Zustellung im Doze-Modus braucht es ab Android 12 zusätzlich `SCHEDULE_EXACT_ALARM` bzw. `USE_EXACT_ALARM` im erzeugten Manifest; ohne diese Rechte stellt Android ungefähr pünktlich zu.
- **iOS:** Höchstens 64 ausstehende lokale Benachrichtigungen pro App. Zunder plant deshalb immer nur die 64 nächsten; weiter entfernte rücken nach, sobald die App wieder läuft (Start, Rückkehr in den Vordergrund, Sync).
- Die App fragt beim Start nach der Benachrichtigungs-Berechtigung.
- Mobile Builds sind in der Entwicklungsumgebung dieses Repos nicht verifiziert.

## Lokaler Speicher

SQLite-Datei `zunder.db` im App-Datenverzeichnis (Linux: `~/.local/share/dev.rubeen.zunder/`, macOS: `~/Library/Application Support/dev.rubeen.zunder/`, mobil in der App-Sandbox), WAL-Modus, Schema über `PRAGMA user_version`.

| Tabelle | Inhalt |
|---|---|
| `ops` | Operations-Log: `pending` (lokal angewendet, noch nicht gepusht, in lokaler Reihenfolge) und `confirmed` (vom Server angenommen; `server_seq`, sobald gepullt) |
| `entities` | materialisierter **bestätigter** Zustand, je Entität ein `SnapshotEntity` als JSON |
| `meta` | `cursor`, `node_id`, `server_url`, `api_token`, `last_sync` |
| `reminders` | an das Betriebssystem übergebene Benachrichtigungen (nur mobil genutzt) |

Im Speicher hält die App zwei Zustände: den bestätigten und die **Sicht** = bestätigt + ausstehende Operationen. Die UI sieht immer die Sicht. Lehnt der Server eine Operation ab, wird die Sicht aus dem bestätigten Zustand und den übrigen ausstehenden Operationen neu aufgebaut (Rücknahme) und die Ablehnung gemeldet.

Der API-Token liegt im Klartext in `meta` (App-Sandbox bzw. Benutzerverzeichnis). Eine Ablage im Schlüsselbund des Betriebssystems ist noch offen.

## Sync

1. Ausstehende Operationen in Paketen zu 500 pushen (`POST /api/sync/push`, `client_id` = Knoten-ID des Geräts). Angenommene werden bestätigt, abgelehnte zurückgenommen.
2. Ist der Speicher noch leer (Cursor 0): `GET /api/snapshot` übernehmen. Das lokale Log beginnt dann beim Snapshot.
3. `GET /api/sync/pull?since=<cursor>` seitenweise, bis `has_more` falsch ist.

Ausgelöst wird der Sync beim Start, alle 30 s, 0,5 s nach lokalen Änderungen (`local_apply`), bei Rückkehr in den Vordergrund, wenn der Webview das Ereignis `online` meldet, und per `sync_now`. Nach Netzwerkfehlern versucht es die App nach 5, 10 und 20 s erneut, danach wieder alle 30 s. Ein verlorener Push ist harmlos: Operationen sind über `op_id` idempotent.

HTTP läuft über `reqwest` mit rustls und den eingebauten Mozilla-Wurzelzertifikaten (gleich auf allen Plattformen, ohne System-TLS). Ein Server mit selbst signiertem Zertifikat wird deshalb nicht akzeptiert; lokal geht `http://`.

## Befehle für die Web-UI

Die Web-UI ruft sie mit `invoke` aus `@tauri-apps/api/core` auf (`TauriTransport` in `web/src/platform/tauri.ts`). Argumentnamen sind die hier gezeigten Schlüssel (auch `task_id` in snake_case). Fehler kommen als abgelehntes Promise mit `{"code": "...", "message": "..."}` (Meldung deutsch); Fehler von Tauri selbst (unbekannter Befehl, Argument passt nicht, z. B. keine UUID) als einfacher String.

| Befehl | Argumente | Ergebnis |
|---|---|---|
| `local_snapshot` | – | `{"cursor": N, "entities": [SnapshotEntity], "pending": N}` – Format wie `GET /api/snapshot`, aber mit der lokalen Sicht (inkl. Tombstones und ausstehender Änderungen) |
| `local_apply` | `{"ops": [Op]}` | `{"accepted": [op_id], "rejected": [{"op_id","code","message"}], "warnings": [{"entity_id","code","message"}], "pending": N}` – lokal mit `zunder_core::validate_push` geprüft; angenommene neue Operationen sind sofort in der Sicht und warten auf den Push |
| `sync_now` | – | `SyncReport`: `{"pushed": N, "rejected": [Rejection], "warnings": [Warning], "pulled": N, "cursor": N, "pending": N, "changed": bool}` |
| `get_config` | – | `{"server_url": String\|null, "has_api_token": bool, "node_id": String, "cursor": N, "pending": N, "last_sync": {"at_ms": N, "ok": bool, "error"?: String}\|null}` – der Token selbst wird nie zurückgegeben |
| `set_config` | `{"update": {"server_url"?: String\|null, "api_token"?: String\|null, "discard_pending"?: bool}}` | wie `get_config`. Fehlender Schlüssel = unverändert, `null` = löschen |
| `task_history` | `{"task_id": UUID}` | `[{"seq","op","source","received_at"}]` – `GET /api/tasks/{id}/history` mit dem gespeicherten Token; kennt nur, was der Server schon erhalten hat |
| `flame_start`, `flame_stop` | `{"task_id": UUID}` | Antwort des Servers unverändert (`{task, running, stopped_task_ids, unchanged, timing, warnings}`). Vorher wird gepusht, falls Operationen ausstehen (der Server kennt sonst die Aufgabe nicht), danach synchronisiert, damit `flame_started_at`/`flame_ended_at` sofort per `zunder://changed` ankommen. Stopp ohne laufende Flamme: Fehler `server` („Server-Fehler (409): …“) |

Regeln:

- Operationen erzeugt die Web-UI selbst (wie im Browser). Als Knoten-ID ihrer HLC nimmt sie `node_id` aus `get_config` (Form `app-<12 hex>`), damit HLCs verschiedener Geräte nie kollidieren.
- `server_url`: `http(s)://host[:port][/pfad]`, ohne Query, Fragment und Zugangsdaten; ein abschließender `/` wird entfernt. `api_token`: nicht leer, ohne Leerzeichen.
- Ein **Wechsel auf einen anderen Server** löscht die lokalen Daten (Log, Zustand, Cursor). Gibt es ausstehende Operationen, schlägt er mit `pending_ops` fehl, außer `discard_pending` ist `true`.
- Fehlercodes: `not_configured`, `invalid_config`, `pending_ops`, `network`, `unauthorized`, `server`, `protocol`, `db`, `corrupt`, `schema_too_new`, `notification`.

Ereignisse (`listen` aus `@tauri-apps/api/event`):

| Ereignis | Nutzlast | Bedeutung |
|---|---|---|
| `zunder://changed` | `{"cursor": N, "pending": N}` | Die Sicht hat sich durch einen Sync geändert (neue Daten oder Rücknahme) – `local_snapshot` neu laden |
| `zunder://sync` | `{"ok": true, "report": SyncReport}` oder `{"ok": false, "error": {"code","message"}}` | Ein Sync-Versuch ist fertig; Ablehnungen stehen in `report.rejected` und sollten angezeigt werden |

`local_apply` sendet kein Ereignis; die aufrufende UI kennt das Ergebnis.

Die Content-Security-Policy erlaubt nur `ipc:`-Verbindungen (und der Server erlaubt kein CORS). Die Web-UI spricht in der App also nie direkt mit dem Server, sondern ausschließlich über diese Befehle. API-Tokens verwalten (`/api/tokens`) geht nur mit Browser-Sitzung und ist deshalb in der App nicht verfügbar.

## Web-UI in der App

Die Web-UI erkennt die App an `window.__TAURI_INTERNALS__` und lädt erst dann per dynamischem Import die Brücke (`web/src/platform/tauri.ts`, `web/src/state/desktop.ts`, `web/src/views/Desktop.tsx`) samt `@tauri-apps/api`. Im Browser werden diese Teile nie geladen.

- **Start:** `get_config`. Fehlen Server-Adresse oder Token, erscheint statt der OIDC-Anmeldung die Einrichtung (Server-Adresse + API-Token → `set_config`, danach `sync_now` als Probe). Ein abgelehnter Token oder ein nicht erreichbarer Server wird angezeigt; die Einstellungen bleiben gespeichert („Trotzdem öffnen“). Ist alles eingerichtet, startet die UI sofort mit den lokalen Daten, auch offline.
- **Sync-Engine der Web-UI:** Snapshot = `local_snapshot`, Push = `local_apply`, Pull gibt es nicht. Nach `zunder://changed` lädt die Engine den Snapshot neu (Ausstehendes bleibt obenauf). Die HLC-Knoten-ID ist `node_id`. Eine Ausstehend-Warteschlange führt die Web-UI nur für einen laufenden `local_apply`, gespeichert wird in SQLite.
- **Status:** Die Sync-Anzeige folgt `zunder://sync` (Synchron, Synchronisiert …, Offline, Token abgelehnt, Sync-Fehler) und der Zahl ausstehender Operationen der App; Ablehnungen aus `report.rejected` erscheinen als Meldung. Ein Klick führt zur Ansicht **Verbindung** (`#/verbindung`: Adresse, Token ersetzen, letzter Abgleich, „Jetzt abgleichen“). Sie ersetzt in der App die Token-Seite und „Abmelden“.
- **Serverwechsel** mit ausstehenden Änderungen: Die UI meldet `pending_ops` und bietet „Änderungen verwerfen und wechseln“ an (`discard_pending: true`).
- Verlauf und Flammen laufen über `task_history`, `flame_start` und `flame_stop`.

Geprüft wird das ohne Webview: Vitest mit nachgebautem `invoke`/`listen` (`web/test/tauri.test.ts`, `web/test/desktop.test.ts`) und ein Playwright-Test, der im Browser eine App-Attrappe als `__TAURI_INTERNALS__` einsetzt (`web/e2e/tauri.spec.ts`; prüft auch, dass keine einzige `/api`-Anfrage gestellt wird). Die HTTP-Seite der neuen Befehle testet `zunder-local` (`client.rs` gegen einen Testserver, `tests/server_e2e.rs` gegen den echten Server).

## Erinnerungen

Nach jeder Zustandsänderung (lokal, Sync, Start, Rückkehr in den Vordergrund) berechnet die App die anstehenden Erinnerungen aller offenen Aufgaben (nicht `done`/`dropped`, nicht gelöscht):

- zu `remind_at`,
- zu `due`, wenn `due` eine Uhrzeit hat (reine Daten haben keinen Zeitpunkt); fällt `due` auf denselben Moment wie `remind_at`, gibt es nur eine Erinnerung.

Titel ist der Aufgabentitel, der Text „Erinnerung“ bzw. „Jetzt fällig“, ergänzt um Bereich oder Projekt der Wurzel-Aufgabe.

- **Mobil** plant das Betriebssystem (64 nächste, siehe oben). Unveränderte Erinnerungen bleiben, geänderte werden abgesagt und unter derselben ID neu geplant, entfallene abgesagt. Bereits zugestellte bleiben sichtbar. Schlägt das Planen fehl, wird nichts gespeichert und beim nächsten Mal erneut geplant.
- **Desktop:** Das Plugin kann dort nicht planen. Ein Timer in der App zeigt fällige Erinnerungen an, solange die App läuft. Was fällig wird, während die App geschlossen ist, wird nicht nachgeholt.

## Offen

- Die fertige App (Webview + Rust) ist hier nicht von Hand durchgeklickt; geprüft sind Rust-Befehle und Web-UI je für sich (siehe oben).
- Der Verlauf einer Aufgabe zeigt erst nach dem Push etwas; noch ausstehende Änderungen fehlen darin.
- Kein SSE in der App: Änderungen anderer Geräte kommen mit dem 30-s-Takt (oder per `sync_now`).
- API-Token im Schlüsselbund statt in SQLite.
- Mobile Builds und Signierung sind nicht eingerichtet und hier nicht getestet.
