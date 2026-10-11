# zunder-web

Die Web-Oberfläche von Zunder: SolidJS, Vite und TypeScript (strict), eigenes schlankes CSS mit hellem und dunklem Modus. Sie spricht dasselbe Sync-Protokoll wie alle anderen Clients (Implementierungsvertrag, Abschnitte 2, 3 und 5).

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
  transport.ts    austauschbarer Transport (heute HTTP, später Tauri-Befehle)
  local-store.ts  austauschbarer lokaler Speicher (heute localStorage + Speicher)
src/capture/    Parser der Kurzschreibweise („Zahnarzt anrufen morgen 9:00 #gesundheit“)
src/state/      Solid-Store über der Engine, Router, Aktionen, Auswahl und Overlays
src/components/ Liste, Board, Aufgabendetail, Schnellerfassung, Befehlspalette …
src/views/      Heute, Inbox, Lose, Bereiche → Projekte, Dev, Phönix, Tokens, Login
test/           Vitest (inkl. gemeinsamer Testvektoren aus crates/zunder-core/tests/vectors)
e2e/            Playwright-Tests und Mock-Server
```

### Erweiterungspunkte

- **Offline im Browser:** eine `LocalStore`-Implementierung mit `persistsState = true` (z. B. IndexedDB). Die Engine startet dann mit dem gespeicherten Zustand und holt nur noch die Differenz.
- **Tauri-App (E4):** `configureServices({ transport, api, localStore })` vor dem Start aufrufen und Transport bzw. Speicher über Tauri-Befehle bereitstellen.

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
- Der Tempo-Test misst den Kaltstart (Navigation bis Daten gerendert, Marke `zunder-ready`) und die Zeit vom Tastendruck bis zum gezeichneten DOM-Update.
