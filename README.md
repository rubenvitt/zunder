# Zunder

Schnelle, selbst gehostete Aufgabenverwaltung für eine Person: Lebensbereiche, Projekte und schnelle Aufgaben, ein Dev-Modus für agentische Softwareentwicklung über MCP, offline-first auf Web, Desktop und Mobile.

Zunder ist das, womit man Feuer entfacht. Aufgaben sind der Brennstoff für fokussierte Arbeit.

## Stand

Zunder ersetzt ClickUp; Stichtag für den Umstieg ist Ende November 2026. Das Design steht in der [Spec](docs/superpowers/specs/2026-10-11-zunder-design.md), die verbindlichen Schnittstellen im [Implementierungsvertrag](docs/superpowers/plans/2026-10-11-zunder-implementation.md).

| Etappe | Inhalt | Stand |
|---|---|---|
| **E1 Fundament** | [`zunder-core`](crates/zunder-core) (Modell, HLC, Operationen, LWW-Merge, Validierung, gemeinsame Testvektoren für Rust und TypeScript) | umgesetzt |
| | [`zunder-server`](crates/zunder-server) (Sync-API, OIDC-Login, API-Tokens, SSE, Backups, Auslieferung der Web-UI) | umgesetzt |
| | [`zunder-import`](crates/zunder-import) (Sicherung, Zuordnung, Bericht, idempotenter Import; Ende-zu-Ende gegen den echten Server getestet) | umgesetzt |
| | Container-Image, CI, Portainer-Stack, Betriebsdoku | umgesetzt |
| **E2 Oberfläche** | [Web-UI](web) mit allen Sichten, MCP-Endpunkt für Agenten, Timing-Brücke für Flammen; Playwright gegen Mock und echten Server | umgesetzt |
| **E3 Umstieg** | Import, Skill-Umstellung, Umschalten, Sicherheitsnetz | Runbook in [`docs/umstieg.md`](docs/umstieg.md); offen: Entscheidung zu Funke, OAuth für claude.ai |
| **E4 Apps** | [Tauri 2](apps/zunder-app) für Desktop, iOS und Android | Gerüst: lokaler Speicher, Sync und Erinnerungen in `zunder-local` (gegen den echten Server getestet); Web-UI noch ohne Tauri-Transport, Mobile-Builds nicht verifiziert |

## Schnellstart (lokale Entwicklung)

Voraussetzungen: Rust 1.97, Node 22 mit npm.

```sh
# Terminal 1: Server, Login ohne OIDC (nur lokal!), Daten in ./data
ZUNDER_DEV_AUTH=true cargo run -p zunder-server

# Terminal 2: Web-UI mit Hot Reload
cd web
npm install
npm run dev
```

Dann `http://localhost:5173` öffnen. Die Web-UI leitet `/api`, `/auth`, `/mcp` und `/healthz` an den Server auf `127.0.0.1:8080` weiter.

Ohne Vite: `(cd web && npm install && npm run build)`, danach liefert der Server die gebaute Oberfläche selbst unter `http://localhost:8080` aus (`ZUNDER_WEB_DIR`, Standard `./web/dist`).

## Dokumentation

- [Entwicklung](docs/entwicklung.md): Aufbau, lokaler Start, Tests, Qualitätsschranken, CI
- [Betrieb](docs/betrieb.md): Container, Portainer/Traefik, Pocket-ID, API-Tokens, MCP verbinden, Backups, Updates
- [Umstieg](docs/umstieg.md): Runbook für E3 – Import, Skill-Umstellung, Umstiegstag, Funke
- [Spec](docs/superpowers/specs/2026-10-11-zunder-design.md) und [Implementierungsvertrag](docs/superpowers/plans/2026-10-11-zunder-implementation.md)

## Qualitätsschranken

Vor jedem Commit (Details in [`docs/entwicklung.md`](docs/entwicklung.md)):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
(cd web && npm run typecheck && npm test && npm run build)
# E2E gegen Mock und echten Server
cargo build -p zunder-server
(cd web && npx playwright test && ZUNDER_E2E_BACKEND=real npx playwright test)
```

## Stack

- Backend: Rust (axum), SQLite
- Gemeinsamer Kern: Rust-Crate `zunder-core` (Domäne, Sync)
- Web-UI: TypeScript (SolidJS)
- Desktop und Mobile: Tauri 2
- Agenten-Schnittstelle: MCP (Streamable HTTP) im Server
- Betrieb: ein Container (amd64/arm64) hinter Traefik, Image auf GHCR

## Lizenz

[Apache-2.0](LICENSE)
