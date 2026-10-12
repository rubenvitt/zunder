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
| `apps/zunder-app` | Tauri 2 (E4): eigener Cargo-Workspace in `src-tauri` mit der Hülle `zunder-app` und der webview-freien Crate `zunder-local` (Speicher, Sync, Erinnerungen) |
| `deploy/` | Portainer-Stack ([`betrieb.md`](betrieb.md)) |
| `Dockerfile`, `.github/workflows/ci.yml` | Image und CI |

## Voraussetzungen

- Rust **1.97** (dieselbe Version wie `RUST_VERSION` im `Dockerfile` und `RUST_TOOLCHAIN` in der CI; beim Anheben beide Stellen ändern)
- Node **22** mit npm
- für E2E-Tests: Chromium für Playwright (`npx playwright install chromium`, passend zu `@playwright/test` in `web/package.json`; entfällt, wenn die Umgebung den Browser schon mitbringt, z. B. über `PLAYWRIGHT_BROWSERS_PATH`)
- für die Tauri-Hülle unter Linux: webkit2gtk und Co. (siehe [`apps/zunder-app/README.md`](../apps/zunder-app/README.md)); `zunder-local` braucht sie nicht
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

Die Web-UI läuft dann unter `http://localhost:5173`, der Server unter `http://127.0.0.1:8080`. Ein anderer Server-Port wird der Web-UI über `ZUNDER_DEV_BACKEND` mitgeteilt. `ZUNDER_DEV_AUTH=true` ist nur für die lokale Entwicklung gedacht; der Server warnt beim Start im Log. Mit Dev-Auth lauscht er ohne `ZUNDER_BIND` nur auf `127.0.0.1:8080`, verweigert den Start bei einer nicht lokalen Bind-Adresse und lässt Anfragen ohne Zugangsdaten nur dann als Besitzer durch, wenn ihr `Host` `localhost`, `127.0.0.1` oder `[::1]` ist (Schutz gegen Zugriffe aus dem Netz und DNS-Rebinding).

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

# Tauri-Workspace (zunder-local; clippy für zunder-app braucht webkit2gtk)
cd apps/zunder-app/src-tauri
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings   # ohne webkit2gtk: -p zunder-local
cargo test -p zunder-local
cd -

# Web
cd web
npm run typecheck
npm test                          # inkl. gemeinsamer Testvektoren aus crates/zunder-core/tests/vectors
npm run build
```

E2E (Playwright), gegen den Mock-Server und gegen den echten Server:

```sh
cargo build -p zunder-server      # im Repo-Wurzelverzeichnis
cd web
npm run build
npx playwright test                              # Mock (Standard)
ZUNDER_E2E_BACKEND=real npx playwright test      # echter Server
```

Mit `ZUNDER_E2E_BACKEND=real` startet die Playwright-Konfiguration den Server aus `ZUNDER_SERVER_BIN` (Default `../target/debug/zunder-server`, relativ zu `web/`) mit frischem Datenordner, `ZUNDER_DEV_AUTH=true` und `ZUNDER_WEB_DIR=dist` auf `127.0.0.1:18080`. `npm run e2e` baut vorher zusätzlich die Web-UI. Die CI nutzt einen Release-Build, weil sie das Tempo-Budget misst.

Ende-zu-Ende-Tests anderer Clients gegen den echten Server (ignoriert, solange `ZUNDER_SERVER_BIN` fehlt):

```sh
cargo build -p zunder-server
export ZUNDER_SERVER_BIN=$PWD/target/debug/zunder-server
cargo test -p zunder-import --test server_e2e -- --ignored
cargo test --manifest-path apps/zunder-app/src-tauri/Cargo.toml -p zunder-local --test server_e2e -- --ignored
```

Shell-Skripte (z. B. `deploy/generate-secrets.sh`) mit `shellcheck` prüfen.

## CI

[`.github/workflows/ci.yml`](../.github/workflows/ci.yml) läuft bei Pull Requests und bei Pushes auf `main` sowie auf Tags `v*`:

| Job | Inhalt |
|---|---|
| `rust` | `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (mit Cache), danach `zunder-import run` gegen den echten Server |
| `local` | `zunder-local` im Tauri-Workspace: `fmt`, `clippy -p zunder-local`, Tests, Sync gegen den echten Server |
| `web` | `npm ci`, `typecheck`, `test` (inkl. gemeinsamer Testvektoren), `build` in `web/` |
| `e2e` | Server (Release) und Web-UI bauen, Chromium installieren, Playwright mit `ZUNDER_E2E_BACKEND=real` (Binärdatei aus `ZUNDER_SERVER_BIN`), danach gegen den Mock; bei Fehlern wird der Report hochgeladen |
| `image` | nur `main` und Tags, nach grünen Checks: Multi-Arch-Image (amd64 + arm64) nach `ghcr.io/<owner>/zunder` mit den Tags `latest`, `sha-<commit>` und SemVer |

Die Tauri-Hülle (`zunder-app`) wird in der CI nicht gebaut: `src-tauri` gehört nicht zum Root-Workspace, braucht unter Linux webkit2gtk/GTK und für Mobile Xcode bzw. Android-SDK und ist nicht Teil des ausgelieferten Containers. Ihre Logik steckt in `zunder-core` (Job `rust`) und `zunder-local` (Job `local`).

## Image lokal bauen

```sh
docker build -t zunder .
docker run --rm -p 127.0.0.1:8080:8080 -e ZUNDER_DEV_AUTH=true -e ZUNDER_DEV_AUTH_ALLOW_REMOTE=true -v zunder-dev:/data zunder
curl -fsS http://localhost:8080/healthz                  # {"status":"ok",…}
docker inspect --format '{{.State.Health.Status}}' <container>   # HEALTHCHECK: zunder-server healthcheck
```

Mehr zum Image (Cross-Kompilierung, Caching, Laufzeit) in [`betrieb.md`](betrieb.md#image-selbst-bauen) und als Kommentar im `Dockerfile`.

## Commits

- Kleine, in sich geschlossene Commits mit aussagekräftiger Nachricht (deutsch oder englisch).
- Release: Tag `vX.Y.Z` auf `main` setzen; die CI baut und veröffentlicht das Image mit den passenden Versions-Tags.
