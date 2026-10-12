# zunder-app

Zunder als Tauri-2-App für Desktop (Linux, macOS, Windows), iOS und Android (Etappe E4 der [Spec](../../docs/superpowers/specs/2026-10-11-zunder-design.md)). Die App zeigt die Web-UI aus `web/` und bringt einen lokalen SQLite-Speicher mit: Änderungen landen sofort lokal, der Sync mit dem Server läuft im Hintergrund, Erinnerungen plant das Gerät selbst. Die Web-UI erkennt die App und spricht dann nur noch über die Befehle unten mit ihr (siehe [Web-UI in der App](#web-ui-in-der-app)).

Verbindlich ist der [Implementierungsvertrag](../../docs/superpowers/plans/2026-10-11-zunder-implementation.md), insbesondere Abschnitt 2 (Operationen), 3 (Sync-API) und 5.1 (App-Befehle).

## Aufbau

```
apps/zunder-app/
  package.json            @tauri-apps/cli (npm run dev / build / icons)
  icon.svg                Quelle der App-Icons (npm run icons)
  icon-ios.svg            Quelle der iOS-Icons (randlos, ohne Transparenz)
  src-tauri/              eigener Cargo-Workspace, nicht im Root-Workspace
    Cargo.toml            Workspace [".", "zunder-local"] + Paket zunder-app
    tauri.conf.json       Identifier dev.rubeen.zunder, Frontend = web/dist
    capabilities/         Rechte des Webviews: nur core:default
    icons/                generierte Icons (Desktop, iOS, Android)
    src/lib.rs            Einstieg (auch mobil: tauri::mobile_entry_point)
    src/commands.rs       Befehle für die Web-UI
    src/background.rs     Sync-Schleife und Ereignisse
    src/login.rs          Anmeldung im Browser: Deep-Link zurück, Geräte-Token speichern
    src/notify.rs         Erinnerungen (mobil: OS-Planer, Desktop: Timer)
    src/update.rs         Selbstaktualisierung aus GitHub-Releases (nur Desktop)
    zunder-local/         Speicher, Sync, Anmeldung (OAuth), Erinnerungsplanung – ohne Webview testbar
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

## Updates und Releases

Die Desktop-App aktualisiert sich selbst aus den GitHub-Releases (`tauri-plugin-updater`, nur Desktop, nur Release-Builds; `src/update.rs`). Sie prüft beim Start und dann alle 6 Stunden `https://github.com/rubenvitt/zunder/releases/latest/download/latest.json`, lädt eine neuere Version im Hintergrund herunter, installiert sie und meldet das per Benachrichtigung. Aktiv wird sie beim nächsten Start; die App startet nicht von selbst neu. Updates müssen mit dem Schlüssel signiert sein, dessen öffentlicher Teil in `tauri.conf.json` (`plugins.updater.pubkey`) steht.

Releases entstehen automatisch aus [Conventional Commits](https://www.conventionalcommits.org/de/v1.0.0/): Nach jedem grünen CI-Lauf eines Pushs auf `main` bestimmt semantic-release (`.github/workflows/release.yml`, Konfiguration in `.github/release/release.mjs`) die nächste Version aus den Commits seit dem letzten Tag `v*`, setzt den Tag und ruft `.github/workflows/app.yml` auf.

| Commits seit dem letzten Tag | Version |
|---|---|
| `typ!:` im Betreff oder `BREAKING CHANGE:` im Text | Major (`v1.4.2` → `v2.0.0`) |
| `feat:` | Minor (`v1.4.2` → `v1.5.0`) |
| `fix:`, `perf:` | Patch (`v1.4.2` → `v1.4.3`) |
| nur `docs:`, `chore:`, `ci:`, `test:`, `refactor:`, `build:`, `style:` | kein Release |

Das erste Release ist `v1.0.0`. Ein von Hand gepushter Tag `v*` startet `app.yml` und `ci.yml` wie bisher direkt; semantic-release zählt danach von diesem Tag weiter.

`app.yml` baut dann auf macOS (Apple Silicon), signiert mit der Developer ID, notarisiert über die App Store Connect API und legt das Release als Entwurf mit `.dmg`, `.app.tar.gz`, Signatur, `latest.json` und den Release-Notes von semantic-release (Neu, Behoben, Schneller, Breaking Changes) an. Erst wenn alles hochgeladen ist, wird es veröffentlicht; `releases/latest` zeigt also nie auf ein Release ohne `latest.json`. Schlägt der Build fehl, bleibt der Tag ohne Release stehen (den Job neu starten oder einfach das nächste Release abwarten). Die Version kommt aus dem Tag (`v0.2.0` → `0.2.0`), nicht aus `tauri.conf.json`; der Updater installiert nur höhere Versionen. Bei Pull Requests baut derselbe Workflow nur (unsigniert, ohne Release).

Das Server-Image bekommt beim Release zusätzlich die SemVer-Tags (etwa `0.2.0`, `0.2`; ab `1.0` auch `1`) in GHCR, ohne neu zu bauen: `release.yml` hängt sie an das Image, das die CI schon unter dem Commit-SHA veröffentlicht hat. App und Server teilen sich die Versionsnummer.

Secrets im Repo, Originale im 1Password-Tresor „Dev“:

| Secret | 1Password-Eintrag |
|---|---|
| `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Zunder App Updater-Signierschlüssel |
| `APPLE_CERTIFICATE` (p12, base64), `APPLE_CERTIFICATE_PASSWORD` | Zunder – Developer ID Application (.p12) |
| `APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_KEY_P8` | Zunder – Apple Notarisierung (App Store Connect API, Rolle Admin; auch für iOS-Signatur und TestFlight) |

Derselbe Workflow lädt bei jedem Release auch die iOS-App zu TestFlight hoch (Job `ios`): `tauri ios init` erzeugt das Xcode-Projekt, die Icons aus `src-tauri/icons/ios` ersetzen die Platzhalter, `tauri ios build --export-method app-store-connect` baut und signiert, `xcrun altool --upload-app` lädt hoch. Signiert wird automatisch über den App-Store-Connect-API-Schlüssel (Xcode legt Distributionszertifikat und Profil selbst an); dafür braucht der Schlüssel die Rolle **Admin**. Version und Build-Nummer kommen aus dem Tag. `src-tauri/Info.ios.plist` erklärt die Verschlüsselung als ausgenommen (nur HTTPS), damit TestFlight nicht nach der Exportkonformität fragt. Nach der Verarbeitung durch Apple (meist 10–30 Minuten) steht der Build den internen TestFlight-Testern zur Verfügung. Einmalig von Hand: App-Eintrag für `dev.rubeen.zunder` in App Store Connect anlegen und eine interne Testgruppe einrichten. Wird ein Release-Lauf wiederholt, lehnt Apple den Upload ab, weil die Build-Nummer schon existiert.

Geht der Updater-Schlüssel verloren, können installierte Apps keine Updates mehr annehmen; dann hilft nur eine Neuinstallation von Hand.

Lokal baut `npm run build` ohne Updater-Artefakte und ohne Signierschlüssel. Mit Artefakten (zum Prüfen):

```sh
TAURI_SIGNING_PRIVATE_KEY=… TAURI_SIGNING_PRIVATE_KEY_PASSWORD=… \
  npm run build -- --config '{"bundle":{"createUpdaterArtifacts":true}}'
```

## Mobil (iOS, Android)

Die nativen Projekte werden nicht eingecheckt, sondern einmalig erzeugt (landen in `src-tauri/gen/`):

```sh
npx tauri android init
npx tauri ios init          # nur auf macOS
npx tauri android dev       # bzw. ios dev; braucht ein Gerät oder einen Emulator
npx tauri android build     # bzw. ios build
```

Auf einem echten iPhone lädt der Webview die Web-UI über das Netz vom Entwicklungsrechner: `npx tauri ios dev --host` (bzw. `--force-ip-prompt`, um die Adresse zu wählen); Telefon und Rechner müssen im selben Netz sein. Vite lauscht dann auf der Adresse aus `TAURI_DEV_HOST` (HMR auf Port 5174, siehe `web/vite.config.ts`), ohne die Variable wie gewohnt nur auf `localhost:5173`.

Hinweise:

- **Android:** Das Benachrichtigungs-Plugin bringt `POST_NOTIFICATIONS`, `RECEIVE_BOOT_COMPLETED` und `WAKE_LOCK` mit; geplante Benachrichtigungen werden nach einem Neustart wiederhergestellt. Für minutengenaue Zustellung im Doze-Modus braucht es ab Android 12 zusätzlich `SCHEDULE_EXACT_ALARM` bzw. `USE_EXACT_ALARM` im erzeugten Manifest; ohne diese Rechte stellt Android ungefähr pünktlich zu.
- **iOS:** Höchstens 64 ausstehende lokale Benachrichtigungen pro App. Zunder plant deshalb immer nur die 64 nächsten; weiter entfernte rücken nach, sobald die App wieder läuft (Start, Rückkehr in den Vordergrund, Sync).
- **Zeitpunkte ans Plugin** (`zunder_local::schedule::plugin_time`): `tauri-plugin-notification` 2.x sendet die Zeit als `…:ss.<9 Ziffern>Z`. iOS liest sie als lokale Uhrzeit (ohne Zeitzone), Android liest alle neun Nachkommastellen als Millisekunden. Die App übergibt deshalb volle Sekunden (aufgerundet) und auf iOS die lokale Uhrzeit des Zeitpunkts als UTC beschriftet (Versatz zum Zeitpunkt selbst, also über Sommer-/Winterzeit hinweg richtig). Bei einem Update des Plugins prüfen, ob das noch nötig ist.
- Die App fragt beim Start nach der Benachrichtigungs-Berechtigung.
- **iOS-Icons** dürfen keinen Alphakanal haben, sonst lehnt App Store Connect den Upload ab. `npm run icons` erzeugt sie aus `icon-ios.svg` und entfernt den Alphakanal danach mit ImageMagick (`magick`). Die CI kopiert `src-tauri/icons/ios` nach `tauri ios init` ins Xcode-Projekt (init bringt sonst Tauri-Platzhalter mit).
- **iOS-Build** (`npx tauri ios build --export-method debugging`) ist lokal mit Xcode 27 gebaut und auf einem iPhone installiert (`xcrun devicectl device install app --device <ID> src-tauri/gen/apple/build/arm64/Zunder.ipa`). Signiert wird automatisch mit dem Team aus `bundle.iOS.developmentTeam`. Android ist nicht eingerichtet (kein SDK/NDK).

## Anmeldung

Statt einen API-Token aus der Web-Oberfläche abzuschreiben, meldet sich die App über den OAuth-Server von Zunder an (Authorization Code mit PKCE für native Apps, RFC 8252; Server-Seite: Implementierungsvertrag 4.3, „Geräte“):

1. `login_start` liest `/.well-known/oauth-authorization-server`, registriert die App als Client mit dem Gerätenamen und der Redirect-URI `dev.rubeen.zunder:/oauth`, speichert die laufende Anmeldung (PKCE-Verifier, `state`, 15 Minuten gültig) in `meta.pending_login` und öffnet die Autorisierung für die Ressource `<Issuer>/api` im System-Browser.
2. Im Browser: Pocket-ID-Anmeldung (falls nötig) und Freigabeseite („… möchte sich als Gerät mit Zunder verbinden“).
3. Der Server leitet auf `dev.rubeen.zunder:/oauth?code=…&state=…`. Das Betriebssystem öffnet damit die App (`tauri-plugin-deep-link`, Schema in `tauri.conf.json`; auch ein Kaltstart, falls iOS die App inzwischen beendet hat).
4. Die App prüft `state`, verbraucht die laufende Anmeldung, tauscht den Code gegen einen Geräte-Token (`zd_…`, läuft nicht ab) und speichert Adresse und Token wie `set_config`. Einen ersetzten Geräte-Token widerruft sie beim Server.

Das Gerät erscheint in der Web-Oberfläche auf der Token-Seite unter „Geräte“ und lässt sich dort abmelden; seine Änderungen tragen die Quelle `app:<Gerätename>`.

- **Adresse:** Die Endpunkte aus den Metadaten müssen auf demselben Origin liegen wie die eingegebene Server-Adresse (die App öffnet sonst womöglich fremde Adressen). Für die Anmeldung also die öffentliche Adresse (`ZUNDER_PUBLIC_URL`) eingeben, nicht etwa eine IP im VPN.
- **macOS:** Deep-Links erreichen nur eine installierte App (Release-Bundle), nicht `npm run dev`. Dort bleibt der Token-Weg.
- **Linux/Windows:** bräuchten zusätzlich `tauri-plugin-single-instance`, damit der Link die laufende App erreicht; nicht eingerichtet, die CI baut dafür auch nichts.

## Lokaler Speicher

SQLite-Datei `zunder.db` im App-Datenverzeichnis (Linux: `~/.local/share/dev.rubeen.zunder/`, macOS: `~/Library/Application Support/dev.rubeen.zunder/`, mobil in der App-Sandbox), WAL-Modus, Schema über `PRAGMA user_version`.

| Tabelle | Inhalt |
|---|---|
| `ops` | Operations-Log: `pending` (lokal angewendet, noch nicht gepusht, in lokaler Reihenfolge) und `confirmed` (vom Server angenommen; `server_seq`, sobald gepullt) |
| `entities` | materialisierter **bestätigter** Zustand, je Entität ein `SnapshotEntity` als JSON |
| `meta` | `cursor`, `node_id`, `server_url`, `data_server_url` (Server, zu dem die lokalen Daten gehören; bleibt beim Löschen der Adresse), `api_token` (API- oder Geräte-Token), `last_sync`, `pending_login` (laufende Anmeldung im Browser) |
| `reminders` | an das Betriebssystem übergebene Benachrichtigungen (nur mobil genutzt) |

Im Speicher hält die App zwei Zustände: den bestätigten und die **Sicht** = bestätigt + ausstehende Operationen. Die UI sieht immer die Sicht. Lehnt der Server eine Operation ab, wird die Sicht aus dem bestätigten Zustand und den übrigen ausstehenden Operationen neu aufgebaut (Rücknahme) und die Ablehnung gemeldet.

Scheitert ein Schreibvorgang (Speicher voll, E/A-Fehler), lädt der Speicher seinen Zustand im Arbeitsspeicher aus SQLite neu; ein erneuter Versuch schreibt die Änderung dann vollständig.

Der API-Token liegt im Klartext in `meta` (App-Sandbox bzw. Benutzerverzeichnis). Auf Linux und macOS sind das Datenverzeichnis (`0700`) und die Datenbankdateien samt `-wal`/`-shm` (`0600`) nur für den eigenen Benutzer lesbar; bestehende Installationen werden beim Start nachgezogen. War die Datei vorher für andere lesbar (gemeinsam genutzter Rechner), den Token ersetzen. Eine Ablage im Schlüsselbund des Betriebssystems ist noch offen.

## Sync

1. Ausstehende Operationen in Paketen zu 500 pushen (`POST /api/sync/push`, `client_id` = Knoten-ID des Geräts). Angenommene werden bestätigt, abgelehnte zurückgenommen.
2. Ist der Speicher noch leer (Cursor 0): `GET /api/snapshot` übernehmen. Das lokale Log beginnt dann beim Snapshot.
3. `GET /api/sync/pull?since=<cursor>` seitenweise, bis `has_more` falsch ist.

Jeder Schritt ist für sich festgeschrieben. Scheitert ein Durchgang mittendrin (etwa der Pull nach einem Push mit Ablehnungen), meldet er trotzdem, was er bis dahin getan hat: `zunder://sync` mit `ok: false` trägt den bisherigen `report` (Ablehnungen, `pending`), und hat sich die Sicht geändert, folgt `zunder://changed`.

**Zurückgesetzter Server** (Backup eingespielt, siehe `docs/betrieb.md`): Danach vergibt der Server `seq`-Nummern neu, die die App schon kennt. Jeder Pull fragt deshalb ab `cursor - 1` und prüft, dass die Operation am eigenen Cursor noch unter dieser `seq` steht, mit derselben `op_id` (nach einem Snapshot bringt der erste Pull sie mit; die überlappende Operation zählt nicht als gepullt). Fehlt sie oder steht dort eine andere, verwirft sie den bestätigten Zustand (Log, Entitäten, Cursor), behält die ausstehenden Operationen und lädt den Snapshot neu (`report.resynced`). Bestätigte Änderungen, die nach dem Backup-Zeitpunkt auf dem Server lagen, sind dann auch in der App weg.

**Serverwechsel während eines Durchgangs:** Der laufende Durchgang wendet danach nichts mehr an (sonst landeten Daten und Cursor des alten Servers im frisch geleerten Speicher) und endet mit `reconfigured`; dafür gibt es kein `zunder://sync`, der nächste Durchgang mit dem neuen Server folgt sofort.

Ausgelöst wird der Sync beim Start, alle 30 s, 0,5 s nach lokalen Änderungen (`local_apply`), bei Rückkehr in den Vordergrund, wenn der Webview das Ereignis `online` meldet, und per `sync_now`. Nach Netzwerkfehlern versucht es die App nach 5, 10 und 20 s erneut, danach wieder alle 30 s. Ein verlorener Push ist harmlos: Operationen sind über `op_id` idempotent.

HTTP läuft über `reqwest` mit rustls und den eingebauten Mozilla-Wurzelzertifikaten (gleich auf allen Plattformen, ohne System-TLS). Ein Server mit selbst signiertem Zertifikat wird deshalb nicht akzeptiert. Unverschlüsseltes `http://` nimmt die App nur für lokale Adressen an (jede Anfrage trägt den Token): Loopback, private und Link-local-Adressen (auch `100.64.0.0/10`, etwa Tailscale), Namen ohne Punkt und Namen unter `localhost`, `local`, `home.arpa`, `internal` oder `lan`. Mit `https://` folgt sie keiner Umleitung auf `http://`.

## Befehle für die Web-UI

Die Web-UI ruft sie mit `invoke` aus `@tauri-apps/api/core` auf (`TauriTransport` in `web/src/platform/tauri.ts`). Argumentnamen sind die hier gezeigten Schlüssel (auch `task_id` in snake_case). Fehler kommen als abgelehntes Promise mit `{"code": "...", "message": "..."}` (Meldung deutsch); Fehler von Tauri selbst (unbekannter Befehl, Argument passt nicht, z. B. keine UUID) als einfacher String.

| Befehl | Argumente | Ergebnis |
|---|---|---|
| `local_snapshot` | – | `{"cursor": N, "entities": [SnapshotEntity], "pending": N}` – Format wie `GET /api/snapshot`, aber mit der lokalen Sicht (inkl. Tombstones und ausstehender Änderungen) |
| `local_apply` | `{"ops": [Op]}` | `{"accepted": [op_id], "rejected": [{"op_id","code","message"}], "warnings": [{"entity_id","code","message"}], "pending": N}` – lokal mit `zunder_core::validate_push` geprüft; angenommene neue Operationen sind sofort in der Sicht und warten auf den Push |
| `sync_now` | – | `SyncReport`: `{"pushed": N, "rejected": [Rejection], "warnings": [Warning], "pulled": N, "cursor": N, "pending": N, "changed": bool, "resynced": bool}` (`resynced`: Server zurückgesetzt, bestätigte Daten neu geladen) |
| `get_config` | – | `{"server_url": String\|null, "has_api_token": bool, "node_id": String, "cursor": N, "pending": N, "last_sync": {"at_ms": N, "ok": bool, "error"?: String}\|null}` – der Token selbst wird nie zurückgegeben |
| `set_config` | `{"update": {"server_url"?: String\|null, "api_token"?: String\|null, "discard_pending"?: bool}}` | wie `get_config`. Fehlender Schlüssel = unverändert, `null` = löschen |
| `login_start` | `{"server_url": String, "device_name": String, "discard_pending": bool}` | `null`; öffnet die Anmeldung im System-Browser (siehe [Anmeldung](#anmeldung)), das Ergebnis kommt als `zunder://login`. Würde der Serverwechsel ausstehende Änderungen verwerfen: sofort `pending_ops` |
| `task_history` | `{"task_id": UUID}` | `[{"seq","op","source","received_at"}]` – `GET /api/tasks/{id}/history` mit dem gespeicherten Token; kennt nur, was der Server schon erhalten hat |
| `flame_start`, `flame_stop` | `{"task_id": UUID}` | Antwort des Servers unverändert (`{task, running, stopped_task_ids, unchanged, timing, warnings}`). Vorher wird gepusht, falls Operationen ausstehen (der Server kennt sonst die Aufgabe nicht), danach synchronisiert, damit `flame_started_at`/`flame_ended_at` sofort per `zunder://changed` ankommen. Stopp ohne laufende Flamme: Fehler `server` („Server-Fehler (409): …“) |

Regeln:

- Operationen erzeugt die Web-UI selbst (wie im Browser). Als Knoten-ID ihrer HLC nimmt sie `node_id` aus `get_config` (Form `app-<12 hex>`), damit HLCs verschiedener Geräte nie kollidieren.
- `server_url`: `http(s)://host[:port][/pfad]`, ohne Query, Fragment und Zugangsdaten; ein abschließender `/` wird entfernt; `http://` nur für lokale Adressen (siehe [Sync](#sync)). `api_token`: nicht leer, ohne Leerzeichen.
- Ein **Wechsel auf einen anderen Server** löscht die lokalen Daten (Log, Zustand, Cursor). Gibt es ausstehende Operationen, schlägt er mit `pending_ops` fehl, außer `discard_pending` ist `true`. Maßgeblich ist der Server, von dem die lokalen Daten stammen: Wer die Adresse mit `null` löscht und dann eine andere setzt, wechselt ebenfalls.
- Fehlercodes: `not_configured`, `invalid_config`, `pending_ops`, `network`, `unauthorized`, `server`, `protocol`, `db`, `corrupt`, `schema_too_new`, `notification`, `reconfigured` (Server während des Durchgangs gewechselt).

Ereignisse (`listen` aus `@tauri-apps/api/event`):

| Ereignis | Nutzlast | Bedeutung |
|---|---|---|
| `zunder://changed` | `{"cursor": N, "pending": N}` | Die Sicht hat sich durch einen Sync geändert (neue Daten oder Rücknahme, auch in einem später gescheiterten Durchgang) – `local_snapshot` neu laden |
| `zunder://login` | `{"ok": true}` oder `{"ok": false, "error": {"code","message"}}` | Die Anmeldung im Browser ist abgeschlossen: Server-Adresse und Geräte-Token sind gespeichert, ein Sync läuft. Fehlercode `login` für abgelehnte, abgelaufene oder fremde Antworten |
| `zunder://sync` | `{"ok": true, "report": SyncReport}` oder `{"ok": false, "error": {"code","message"}, "report": SyncReport}` | Ein Sync-Versuch ist fertig; Ablehnungen stehen in `report.rejected` (auch bei `ok: false`: was der Durchgang vor dem Fehler getan hat) und sollten angezeigt werden. Nicht für `reconfigured` |

`local_apply` sendet kein Ereignis; die aufrufende UI kennt das Ergebnis.

Die Content-Security-Policy erlaubt nur `ipc:`-Verbindungen (und der Server erlaubt kein CORS). Die Web-UI spricht in der App also nie direkt mit dem Server, sondern ausschließlich über diese Befehle. API-Tokens verwalten (`/api/tokens`) geht nur mit Browser-Sitzung und ist deshalb in der App nicht verfügbar.

## Web-UI in der App

Die Web-UI erkennt die App an `window.__TAURI_INTERNALS__` und lädt erst dann per dynamischem Import die Brücke (`web/src/platform/tauri.ts`, `web/src/state/desktop.ts`, `web/src/views/Desktop.tsx`) samt `@tauri-apps/api`. Im Browser werden diese Teile nie geladen.

- **Start:** `get_config`. Fehlen Server-Adresse oder Token, erscheint statt der OIDC-Anmeldung die Einrichtung: Server-Adresse und Gerätename (Vorschlag aus dem User-Agent), „Im Browser anmelden“ (`login_start`, dann Hinweis, bis `zunder://login` kommt; kehrt das Fenster zurück, liest die UI `get_config` neu, falls das Ereignis verloren ging). Daneben weiterhin Server-Adresse + API-Token → `set_config`, danach `sync_now` als Probe. Ein abgelehnter Token oder ein nicht erreichbarer Server wird angezeigt; die Einstellungen bleiben gespeichert („Trotzdem öffnen“). Ist alles eingerichtet, startet die UI sofort mit den lokalen Daten, auch offline.
- **Sync-Engine der Web-UI:** Snapshot = `local_snapshot`, Push = `local_apply`, Pull gibt es nicht. Nach `zunder://changed` lädt die Engine den Snapshot neu (Ausstehendes bleibt obenauf). Die HLC-Knoten-ID ist `node_id`. Eine Ausstehend-Warteschlange führt die Web-UI nur für einen laufenden `local_apply`, gespeichert wird in SQLite.
- **Status:** Die Sync-Anzeige folgt `zunder://sync` (Synchron, Synchronisiert …, Offline, Token abgelehnt, Sync-Fehler) und der Zahl ausstehender Operationen der App; Ablehnungen aus `report.rejected` erscheinen als Meldung, auch aus einem gescheiterten Durchgang. Ein Klick führt zur Ansicht **Verbindung** (`#/verbindung`: Adresse, Token ersetzen, letzter Abgleich, „Jetzt abgleichen“). Sie ersetzt in der App die Token-Seite und „Abmelden“.
- **Serverwechsel** mit ausstehenden Änderungen: Die UI meldet `pending_ops` und bietet „Änderungen verwerfen und wechseln“ an (`discard_pending: true`).
- Verlauf und Flammen laufen über `task_history`, `flame_start` und `flame_stop`.

Geprüft wird das ohne Webview: Vitest mit nachgebautem `invoke`/`listen` (`web/test/tauri.test.ts`, `web/test/desktop.test.ts`) und ein Playwright-Test, der im Browser eine App-Attrappe als `__TAURI_INTERNALS__` einsetzt (`web/e2e/tauri.spec.ts`; prüft auch, dass keine einzige `/api`-Anfrage gestellt wird). Die HTTP-Seite der neuen Befehle testet `zunder-local` (`client.rs` gegen einen Testserver, `tests/server_e2e.rs` gegen den echten Server).

## Erinnerungen

Nach jeder Zustandsänderung (lokal, Sync, Start, Rückkehr in den Vordergrund) berechnet die App die anstehenden Erinnerungen aller offenen Aufgaben (nicht `done`/`dropped`, nicht gelöscht):

- zu `remind_at`,
- zu `due`, wenn `due` eine Uhrzeit hat (reine Daten haben keinen Zeitpunkt); fällt `due` auf denselben Moment wie `remind_at`, gibt es nur eine Erinnerung.

Titel ist der Aufgabentitel, der Text „Erinnerung“ bzw. „Jetzt fällig“, ergänzt um Bereich oder Projekt der Wurzel-Aufgabe.

- **Mobil** plant das Betriebssystem (64 nächste, siehe oben). Unveränderte Erinnerungen bleiben, geänderte werden abgesagt und unter derselben ID neu geplant, entfallene abgesagt. Bereits zugestellte bleiben sichtbar. Jede Erinnerung wird für sich geplant: Lehnt das System eine ab (iOS etwa einen gerade verstrichenen Zeitpunkt), gehen die übrigen trotzdem hinaus; die abgelehnte wird nicht gespeichert und beim nächsten Mal erneut geplant. Schlägt schon das Absagen fehl, wird nichts gespeichert.
- **Desktop:** Das Plugin kann dort nicht planen. Ein Timer in der App zeigt fällige Erinnerungen an, solange die App läuft. Was fällig wird, während die App geschlossen ist, wird nicht nachgeholt.

## Offen

- Die Anmeldung ist auf macOS mit dem Release-Bundle gegen einen lokalen Server durchgespielt (Einrichtung ausgefüllt, Browser geöffnet, Freigabe, Deep-Link zurück, Token gespeichert, Sync, UI offen). Auf dem iPhone noch nicht: dafür muss der Server mit Gerätezugängen laufen.
- Der Verlauf einer Aufgabe zeigt erst nach dem Push etwas; noch ausstehende Änderungen fehlen darin.
- Kein SSE in der App: Änderungen anderer Geräte kommen mit dem 30-s-Takt (oder per `sync_now`).
- API- bzw. Geräte-Token im Schlüsselbund statt in SQLite.
- Android-Build ist nicht eingerichtet; iOS nur lokal gebaut, nicht in der CI und nicht über TestFlight verteilt.
- Desktop-Releases gibt es nur für macOS (Apple Silicon); Linux und Windows baut die CI nicht.
