# zunder-web

Die Web-Oberfläche von Zunder: SolidJS, Vite und TypeScript (strict), eigenes schlankes CSS mit hellem und dunklem Modus. Sie spricht dasselbe Sync-Protokoll wie alle anderen Clients (Implementierungsvertrag, Abschnitte 2, 3 und 5). Dieselbe Oberfläche läuft auch in der Tauri-App (`apps/zunder-app`), dort über die Befehle der App statt HTTP (siehe [In der Tauri-App](#in-der-tauri-app)).

## Befehle

| Befehl | Zweck |
|---|---|
| `npm install` | Abhängigkeiten installieren (Node ≥ 22.18) |
| `npm run dev` | Entwicklungsserver; `/api`, `/auth`, `/mcp` und `/healthz` gehen an `http://127.0.0.1:8080` (anderes Ziel über `ZUNDER_DEV_BACKEND`) |
| `npm run build` | Produktions-Build nach `dist/` (liefert der Server über `ZUNDER_WEB_DIR` aus) |
| `npm run typecheck` | `tsc --noEmit` für App, Tests und Konfiguration |
| `npm test` | Unit-Tests (Vitest) |
| `npm run e2e` | Baut und startet die Playwright-Tests |

Ohne laufenden Rust-Server lässt sich die Oberfläche mit dem Mock-Server ausprobieren:

```sh
npm run build
node e2e/mock-server.ts   # http://127.0.0.1:4173, mit Beispieldaten
```

## Aufbau

```
src/sync/       Protokollschicht ohne UI-Abhängigkeiten
  types.ts        Wire-Typen (Op, HLC, Felder, API-Antworten)
  hlc.ts          Hybrid Logical Clock im Vertragsformat
  merge.ts        Last-Writer-Wins je (Entität, Feld), Gleichstand nach op_id
  model.ts        Materialisierung mit Vertrags-Defaults
  derived.ts      wirksame Stelle, Fortschritt, status_changed_at, 3-Tage-Regel
  validate.ts     clientseitige Spiegelung der Kernvalidierung (gleiche Codes) und Lösch-Kaskade
  rank.ts         sortierbare Ränge für Listen und Board
  engine.ts       Sync-Engine: Snapshot, optimistische Änderungen, Push-Warteschlange,
                  Rollback abgelehnter Operationen, Pull über SSE bzw. Polling, Backoff
  transport.ts    austauschbarer Transport; HttpTransport für den Browser
  local-store.ts  austauschbarer lokaler Speicher (Browser: localStorage + Speicher)
src/platform/   Laufzeit: runtime.ts (Erkennung der App), tauri.ts (TauriTransport,
                nur in der App per dynamischem Import geladen, samt @tauri-apps/api)
src/capture/    Parser der Kurzschreibweise („Zahnarzt anrufen morgen 9:00 #gesundheit“)
src/state/      Solid-Store über der Engine, Router, Aktionen, Auswahl und Overlays;
                desktop.ts: Verbindung und Sync-Status der App (nur in der App geladen)
src/components/ Liste, Board, Aufgabendetail, Schnellerfassung, Befehlspalette …
src/views/      Heute, Inbox, Lose, Bereiche → Projekte, Dev, Phönix, Tokens, Einstellungen, Login;
                Desktop.tsx: Start, Einrichtung und Verbindung in der App (nur dort geladen)
test/           Vitest (inkl. gemeinsamer Testvektoren aus crates/zunder-core/tests/vectors)
e2e/            Playwright-Tests und Mock-Server
```

### Erweiterungspunkte

- **Offline im Browser:** eine `LocalStore`-Implementierung mit `persistsState = true` (z. B. IndexedDB). Die Engine startet dann mit dem gespeicherten Zustand und holt nur noch die Differenz.
- **Andere Transporte:** `configureServices({ transport, api, localStore })` vor `startSync()` aufrufen. So bindet sich auch die Tauri-App ein.

## In der Tauri-App

Läuft die Oberfläche im Webview der App (`window.__TAURI_INTERNALS__` vorhanden), lädt `App.tsx` per dynamischem Import `views/Desktop.tsx`; erst darüber kommen `state/desktop.ts`, `platform/tauri.ts` und `@tauri-apps/api` (`invoke`, `listen`) dazu. Im Browser-Bundle steckt davon nur die Erkennung, die Verzweigung und der Lade-Helfer von Vite (gut 2 kB gzip); die App-Teile liegen in eigenen Chunks (zusammen rund 7 kB gzip), die nur die App lädt.

- **Keine Anmeldung:** Ohne Server-Adresse oder Token zeigt die App die Einrichtung (Server-Adresse + API-Token → `set_config`, dann `sync_now` als Probe). Einen Token erstellt man in der Web-Oberfläche unter „API-Tokens“.
- **Daten:** `configureServices()` bekommt einen `TauriTransport`: Snapshot = `local_snapshot`, Push = `local_apply` (die App prüft lokal und pusht selbst), kein Pull. Nach `zunder://changed` lädt die Engine den Snapshot neu (`SyncEngine.reload()`, ausgelöst über `onReset` der Subscription). Knoten-ID der HLC ist `node_id` der App.
- **Server-Aufrufe:** Verlauf und Flammen über die App-Befehle `task_history`, `flame_start`, `flame_stop` (die CSP der App erlaubt nur IPC, der Server kein CORS). Token-Verwaltung braucht eine Browser-Sitzung und ist in der App nur als Hinweis sichtbar.
- **Status:** Die Sync-Anzeige folgt `zunder://sync` und der Zahl ausstehender Operationen der App; die Ansicht **Verbindung** (`#/verbindung`) zeigt Adresse, letzten Abgleich und „Jetzt abgleichen“ und ersetzt „API-Tokens“ und „Abmelden“.

Befehls- und Ereignisvertrag: [`apps/zunder-app/README.md`](../apps/zunder-app/README.md).

## Bedienung

Tastatur: `n` erfassen, `⌘K`/`Strg+K` oder `/` Suche, `j`/`k` bewegen, `Enter` öffnen, `x` erledigt, `Esc` schließen, in der Inbox `a` zuordnen, `l` lose, `d` verwerfen, auf dem Board `h`/`l` Spalte wechseln, `Shift+H`/`Shift+L` Karte verschieben, `Shift+J`/`Shift+K` umsortieren, `v` Liste/Board, `g h/i/l/b/d/p` Sichtwechsel, `?` Übersicht.

Mobil: Sichten als Tabs unten, großes ＋ zum Erfassen, in der Inbox nach rechts wischen = lose, nach links = verwerfen.

## Tests

- `npm test` prüft HLC, Merge (auch in vielen Permutationen), Ableitungen, Validierung, Ränge, Sync-Engine und Parser. Die gemeinsamen Testvektoren aus `crates/zunder-core/tests/vectors/` (Merge, HLC-Ordnung und Validierung gegen den Kern) sind Pflicht: Fehlt das Verzeichnis, schlägt der Test fehl. Ein anderes Verzeichnis lässt sich über `ZUNDER_VECTOR_DIR` setzen.
- `npm run e2e` startet standardmäßig den Mock-Server (`ZUNDER_E2E_BACKEND=mock`). Mit `ZUNDER_E2E_BACKEND=real` startet Playwright stattdessen die fertig gebaute Binärdatei des Servers (`ZUNDER_SERVER_BIN`, Standard `../target/debug/zunder-server`) mit `ZUNDER_DEV_AUTH=true`, einem frischen temporären Datenverzeichnis und `ZUNDER_WEB_DIR=dist` auf `127.0.0.1:18080`. Testdaten entstehen in beiden Fällen über `POST /api/sync/push` (mit `X-Zunder-Client: web`).

```sh
cargo build -p zunder-server            # im Repo-Wurzelverzeichnis
cd web && npm run build
ZUNDER_E2E_BACKEND=real npx playwright test
```

- Abgelehnte Änderungen werden in beiden Modi echt provoziert (veralteter Client-Zustand: Pull und SSE werden kurz blockiert, ein anderer Client löscht den Zielbereich, der Server lehnt mit `invalid_reference` ab). Nur der Test der Login-Seite läuft allein gegen den Mock, weil Dev-Auth jede Anfrage ohne Sitzung als angemeldet behandelt.
- `test/tauri.test.ts` und `test/desktop.test.ts` prüfen die App-Anbindung mit nachgebautem `invoke`/`listen` (Fehlerzuordnung, Befehle und Argumente, Neuladen nach `zunder://changed`, kein veralteter Snapshot über einer gerade angewendeten Änderung, Einrichtung inkl. `pending_ops`). `e2e/tauri.spec.ts` setzt im Browser eine App-Attrappe als `__TAURI_INTERNALS__` ein und spielt Einrichtung, Erfassen, Sync-Ereignisse, Flamme, Verlauf und Verbindung durch; der Test schlägt fehl, sobald die Seite eine `/api`-Anfrage stellt. Er braucht keinen Server-Zustand und läuft in beiden Backend-Modi.
- Der Tempo-Test misst den Kaltstart (Navigation bis Daten gerendert, Marke `zunder-ready`) und die Zeit vom Tastendruck bis zum gezeichneten DOM-Update.
