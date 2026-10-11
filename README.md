# Zunder

Schnelle, selbst gehostete Aufgabenverwaltung für eine Person: Lebensbereiche, Projekte und schnelle Aufgaben, ein Dev-Modus für agentische Softwareentwicklung über MCP, offline-first auf Web, Desktop und Mobile.

Zunder ist das, womit man Feuer entfacht. Aufgaben sind der Brennstoff für fokussierte Arbeit.

## Stand

Planungsphase. Das Design steht in [`docs/superpowers/specs/2026-10-11-zunder-design.md`](docs/superpowers/specs/2026-10-11-zunder-design.md).

## Stack

- Backend: Rust (axum), SQLite
- Gemeinsamer Kern: Rust-Crate `zunder-core` (Domäne, Sync)
- Web-UI: TypeScript (SolidJS)
- Desktop und Mobile: Tauri 2
- Agenten-Schnittstelle: MCP (Streamable HTTP) im Server

## Lizenz

[Apache-2.0](LICENSE)
