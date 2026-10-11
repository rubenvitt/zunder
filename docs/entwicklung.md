# Entwicklung

Wie man an Zunder arbeitet: Aufbau, lokaler Start, Tests und die Qualitätsschranken, die vor jedem Commit grün sein müssen.

## Grundregeln

- **Vertrag zuerst.** Schnittstellen, Wire-Format, Felder, Validierung, HTTP-API, MCP-Werkzeuge und Umgebungsvariablen stehen im [Implementierungsvertrag](superpowers/plans/2026-10-11-zunder-implementation.md). Wer davon abweicht oder eine Lücke füllt, passt den Vertrag im selben Commit an.
- **Sprache:** Code-Bezeichner und Kommentare englisch, Oberfläche und Doku deutsch.
- **Keine echten Daten im Repo.** Testdaten sind synthetisch oder anonymisiert. `mapping.toml`, Import-Backups und befüllte `stack.env.local` bleiben außerhalb von Git.
- **Testgetrieben** wo sinnvoll, besonders in `zunder-core` (Merge, Validierung, HLC).

## Aufbau

| Pfad | Inhalt |
|---|---|
| `crates/zunder-core` | Domäne, Validierung, HLC, Merge (ohne I/O) |
| `crates/zunder-server` | axum, SQLite, Sync-API, `/mcp`, OIDC, Timing-Brücke |
| `crates/zunder-import` | CLI für den ClickUp-Import |
| `web/` | SolidJS + Vite + TypeScript, Vitest, Playwright |
| `apps/zunder-app` | Tauri 2 (E4), nicht Teil des Cargo-Workspace |
| `deploy/` | Portainer-Stack ([`betrieb.md`](betrieb.md)) |
| `Dockerfile`, `.github/workflows/ci.yml` | Image und CI |

## Voraussetzungen

- Rust **1.97** (dieselbe Version wie `RUST_VERSION` im `Dockerfile` und `RUST_TOOLCHAIN` in der CI; beim Anheben beide Stellen ändern)
- Node **22** mit npm
- für E2E-Tests: Chromium für Playwright (`npx playwright install chromium`, passend zu `@playwright/test` in `web/package.json`)
- optional: Docker mit Buildx für Image-Builds

## Lokal starten

```sh
# Terminal 1: Server mit Login ohne OIDC, Daten in ./data
ZUNDER_DEV_AUTH=true cargo run -p zunder-server

# Terminal 2: Web-UI mit Hot Reload, leitet /api, /auth, /mcp und /healthz an den Server weiter
cd web
npm install
npm run dev
```

Die Web-UI läuft dann unter `http://localhost:5173`, der Server unter `http://127.0.0.1:8080`. Ein anderer Server-Port wird der Web-UI über `ZUNDER_DEV_BACKEND` mitgeteilt. `ZUNDER_DEV_AUTH=true` ist nur für die lokale Entwicklung gedacht; der Server warnt beim Start im Log.

Für MCP-Tests ohne Web-Login einen festen Token setzen:

```sh
ZUNDER_DEV_AUTH=true ZUNDER_BOOTSTRAP_TOKEN="zt_$(openssl rand -base64 32 | tr '+/' '-_' | tr -d '=')" \
  cargo run -p zunder-server
```

## Qualitätsschranken

Vor jedem Commit, für die Teile, die man angefasst hat (die CI prüft alles):

```sh
# Rust
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace            # oder gezielt: cargo test -p zunder-core

# Web
cd web
npm run typecheck
npm test
npm run build
```

E2E gegen den echten Server:

```sh
cargo build -p zunder-server
cd web
npm run build
ZUNDER_E2E_BACKEND=real npm run e2e
```

Die Playwright-Konfiguration startet den Server aus `ZUNDER_SERVER_BIN` (Default `../target/debug/zunder-server`) mit frischem Datenordner und `ZUNDER_DEV_AUTH=true`. Die CI nutzt einen Release-Build, weil sie das Tempo-Budget misst.

Shell-Skripte (z. B. `deploy/generate-secrets.sh`) mit `shellcheck` prüfen.

## CI

[`.github/workflows/ci.yml`](../.github/workflows/ci.yml) läuft bei Pull Requests und bei Pushes auf `main` sowie auf Tags `v*`:

| Job | Inhalt |
|---|---|
| `rust` | `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (mit Cache) |
| `web` | `npm ci`, `typecheck`, `test`, `build` in `web/` |
| `e2e` | Server (Release) und Web-UI bauen, Chromium installieren, Playwright mit `ZUNDER_E2E_BACKEND=real`; bei Fehlern wird der Report hochgeladen |
| `image` | nur `main` und Tags, nach grünen Checks: Multi-Arch-Image (amd64 + arm64) nach `ghcr.io/<owner>/zunder` mit den Tags `latest`, `sha-<commit>` und SemVer |

Die Tauri-App wird in der CI nicht gebaut: `src-tauri` gehört nicht zum Workspace, braucht unter Linux webkit2gtk/GTK und für Mobile Xcode bzw. Android-SDK und ist nicht Teil des ausgelieferten Containers. Die gemeinsame Logik steckt in `zunder-core` und wird im Job `rust` getestet.

## Image lokal bauen

```sh
docker build -t zunder .
docker run --rm -p 8080:8080 -e ZUNDER_DEV_AUTH=true -v zunder-dev:/data zunder
```

Mehr zum Image (Cross-Kompilierung, Caching, Laufzeit) in [`betrieb.md`](betrieb.md#image-selbst-bauen) und als Kommentar im `Dockerfile`.

## Commits

- Kleine, in sich geschlossene Commits mit aussagekräftiger Nachricht (deutsch oder englisch).
- Release: Tag `vX.Y.Z` auf `main` setzen; die CI baut und veröffentlicht das Image mit den passenden Versions-Tags.
