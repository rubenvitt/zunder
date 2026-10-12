# Zunder – Design

- **Datum:** 2026-10-11
- **Status:** freigegeben
- **Stichtag Umstieg:** Ende November 2026 (früher ist willkommen, Qualität geht vor)

## 1. Ziel und Erfolgskriterien

Zunder ersetzt ClickUp als persönliche Aufgabenverwaltung. Es ist kein Testprojekt: **Zunder ist fertig, wenn ClickUp gekündigt ist.**

Was an ClickUp stört und was Zunder deshalb anders macht:

| Problem mit ClickUp | Antwort von Zunder |
|---|---|
| Web, Desktop und vor allem Mobile sind langsam, mobil unübersichtlich | Tempo-Budget (< 100 ms je Interaktion), offline-first, eine schlanke UI |
| Zu teuer | Selbst gehostet, ein Container |
| KI überall | Keine KI-Features im MVP; später höchstens ein enges, typisiertes Urteil (Abschnitt 10) |
| Zu viele ungenutzte Features (Whiteboard, Chat …) | Nur Aufgaben. Gute Defaults statt Konfigurierbarkeit |

Erfolgskriterien:

1. ClickUp ist gekündigt, alle bestehenden Aufgaben sind importiert.
2. Alle bestehenden Claude-Skills, die heute die ClickUp-MCP nutzen, laufen gegen den Zunder-MCP.
3. Jede Interaktion in der UI wirkt in unter 100 ms, der Kaltstart der Web-UI liegt unter 1 s.
4. Mobile ist voll nutzbar (nach dem Umstieg, Etappe E4).

**Nutzerkreis:** eine Person. Mehrbenutzerbetrieb wird nicht gebaut, aber auch nicht verbaut: Datensätze haben einen Besitzer, es gibt keine Rollen und keine Einladungen.

**Leitprinzip:** Gute Defaults statt Konfigurierbarkeit. Feste Typen, ein fester Status-Workflow, keine Custom Fields. Änderungen passieren im Code, nicht in Einstellungsdialogen.

## 2. Datenmodell

### Struktur (PARA)

- **Bereich** (Lebensbereich): Name, Vault-Verknüpfung über `zunder_id`, Zustand `aktiv` oder `archiviert`.
- **Projekt**: gehört immer zu genau einem Bereich. Status `aktiv`, `pausiert` oder `abgeschlossen`. Art `standard` oder `dev`. Ein Dev-Projekt trägt zusätzlich das GitHub-Repo (`owner/name`).
- **Aufgabe**: hängt an genau einer Stelle:
  - **Inbox** – uneingeordnet, soll auf null kommen
  - **Lose** – bewusst ohne Zuordnung, bleibt so
  - **Bereich** – laufende Verantwortung ohne Projekt
  - **Projekt**

### Felder einer Aufgabe

| Feld | Bemerkung |
|---|---|
| `id` | CUID2 (24 Zeichen), clientseitig erzeugt (offline-tauglich); alte UUIDs werden fest umgerechnet |
| `title` | Pflicht |
| `description` | Markdown |
| `type` | siehe unten |
| `status` | siehe unten |
| `parent_id` | Unteraufgaben in beliebiger Tiefe |
| `due` | Datum oder Datum mit Uhrzeit |
| `remind_at` | Zeitpunkt für eine Erinnerung |
| `vault_links` | Verweise auf Notizen im Obsidian-Vault |
| `links` | externe Links |
| `phoenix` | optional: Rolle (`feuer_schritt` oder `flamme`) und Feuer-Kennung (z. B. `2026-W45`) |
| `claimed_by`, `claim_expires_at` | Reservierung durch einen Agenten (Dev-Modus) |
| `plan`, `branch`, `pr_url` | strukturierte Ergebnisse aus dem Dev-Modus |
| `timing_ref` | Verknüpfung zum Timer in Timing (Abschnitt 6) |
| `clickup_id` | nur importierte Aufgaben |

### Typen

- `aufgabe` – überall
- `bug`, `feature` – nur in Dev-Projekten

Einen Typ „Epic“ gibt es nicht. Eine Aufgabe mit Unteraufgaben ist das Epic.

### Status

Ein Workflow für alles:

```
offen → in_arbeit → [review] → erledigt
            ↕
        blockiert              verworfen (jederzeit)
```

- `review` gibt es nur in Dev-Projekten (offener PR).
- Kein separates „archiviert“: Erledigte und verworfene Aufgaben verschwinden nach einigen Tagen aus den Standardansichten, bleiben aber durchsuchbar.
- Eine Elternaufgabe zeigt den Fortschritt ihrer Unteraufgaben (z. B. 3/5) und wechselt ihren Status nie automatisch. Wird sie auf `erledigt` gesetzt, während Unteraufgaben offen sind, gibt es einen Hinweis.

Im Code heißen die Status englisch (`open`, `in_progress`, `review`, `blocked`, `done`, `dropped`), in der UI deutsch.

### Phönix als Querschnitt

Das persönliche Produktivitätssystem (Wochenzyklen „Feuer“, Deep-Work-Blöcke „Flammen“) ist **kein Container**, sondern eine Markierung. Eine Aufgabe in jedem Bereich oder Projekt kann zugleich Feuer-Schritt oder Flamme sein. Die Phönix-Sicht filtert danach. Längere Zyklen, Ziele und Reviews bleiben im Vault und im Kalender.

### Bewusst nicht im Modell

- **Kommentare.** Stattdessen ein automatisches Änderungsprotokoll je Aufgabe.
- Custom Fields, Prioritäten, Tags.
- Wiederkehrende Aufgaben (nicht im MVP; das Modell verbaut sie nicht).
- Eigenes Zeittracking (übernimmt Timing, Abschnitt 6).

## 3. Architektur

```
                 ┌──────────── zunder-server (Rust, axum) ─────────────┐
 Web-UI ──HTTP──▶│ Sync-API   │ /mcp (Agenten, Claude) │ Timing-Brücke │
 Tauri-App ─────▶│            zunder-core            │               │
 (E4)            │            SQLite                 │               │
                 └──────────────────────────────────────────────────────┘
  zunder-import (CLI): ClickUp-API → Zunder, Zuordnung clickup_id → zunder_id
```

| Baustein | Aufgabe |
|---|---|
| `zunder-core` | Rust-Crate: Domänenmodell, Validierung, Operationen, Hybrid Logical Clock, Merge. Von Server und Tauri-App gemeinsam genutzt. |
| `zunder-server` | axum, SQLite, Sync-API, MCP-Endpoint, Timing-Brücke, liefert die Web-UI aus. Ein Container hinter einem Reverse Proxy. |
| `zunder-web` | TypeScript-UI (SolidJS). Läuft im Browser und in der Tauri-Hülle. |
| `zunder-app` | Tauri 2 für Desktop, iOS und Android, mit lokaler SQLite über `zunder-core` (E4). |
| `zunder-import` | CLI für den einmaligen ClickUp-Import. |

**Login:** OIDC (Pocket-ID). Für Agenten und MCP gibt es API-Tokens, die einzeln widerrufbar sind.

**Hosting:** All-in-one in einem Container. Eine getrennte Auslieferung der Web-UI (z. B. über Cloudflare) bleibt später möglich.

## 4. Sync (offline-first)

- Jede Änderung ist eine **Operation**: `op_id` (CUID2), Entität, Feld, Wert, HLC-Zeitstempel.
- Clients schicken Operationen (`push`) und holen alle Operationen seit ihrem letzten Cursor (`pull`).
- Der Server ist die Autorität. Er speichert das Operations-Log und den materialisierten Zustand.
- **Konfliktauflösung:** Last-Writer-Wins pro Feld nach HLC. Für einen einzelnen Nutzer reicht das. Eine CRDT-Bibliothek wird bewusst nicht eingesetzt.
- Operationen sind idempotent (`op_id`). Ungültige Operationen werden vom Kern abgelehnt und mit Grund an den Client zurückgemeldet. Nichts verschwindet still.
- Die Web-UI nutzt **von Anfang an dasselbe Protokoll**, zunächst online mit optimistischen Updates. Offline-Fähigkeit im Browser ist später nur ein lokaler Speicher, kein Umbau. Die Tauri-App bringt in E4 lokale SQLite mit.

## 5. MCP und Dev-Modus

Der MCP-Server läuft im Backend unter `/mcp` (Streamable HTTP) und ist damit für Claude Code, claude.ai und andere Agenten von überall erreichbar.

**Werkzeuge** liegen semantisch nah an den heute genutzten ClickUp-Aufrufen, damit die Skill-Umstellung weitgehend mechanisch ist:

`filter_tasks`, `get_task`, `create_task`, `update_task`, `move_task`, `delete_task`, `search`, dazu für den Dev-Modus `claim_task`, `release_task` und `decompose_task`.

- Jedes Werkzeug, das eine Aufgabe adressiert, akzeptiert auch eine `clickup_id` und löst sie auf. Bestehende Verweise im Vault bleiben dadurch gültig.
- **Dev-Modus:** Zunder ersetzt GitHub Issues für eigene Projekte. Agenten arbeiten vollständig auf dem Board: Aufgaben anlegen, zerlegen, reservieren (mit Ablaufzeit), Status setzen sowie Plan, Branch und PR-Link an die Aufgabe schreiben. GitHub bleibt für Code und PRs.

## 6. Integrationen

**Vault (Obsidian) – geteilte Identität statt geteilter Dateien**

- Aufgaben leben nur in Zunder. Der Vault sieht sie über den MCP und über Links.
- Bereiche und Projekte existieren in beiden Welten, verbunden über `zunder_id` im Frontmatter, nicht über den Pfad. Verschieben und Umbenennen im Vault bricht nichts.
- Abgeglichen werden nur Name, Status und Bereichszugehörigkeit. Den Inhalt einer Notiz fasst Zunder nie an.
- Stufe 1 (MVP): Anlegen aus dem Vault läuft über Claude bzw. den MCP. Stufe 2 (später): automatischer Abgleich, der Vault-Änderungen selbst erkennt.
- Checkboxen in Daily Notes werden nicht synchronisiert, sondern weiterhin per Skill triagiert, dann nach Zunder.

**Timing (Zeittracking)**

- Ziel: Die Aktion „Flamme starten“ an einer Aufgabe startet über die Timing-Web-API einen Timer, „beenden“ stoppt ihn. Zunder speichert nur `timing_ref`.
- Fallback, falls die Automatik nicht trägt: Zunder speichert Start- und Endzeitstempel an der Aufgabe, ohne Berichte. Auswertungen laufen über Timing.

**Erinnerungen**

- `remind_at` und `due` sind ab Tag 1 im Modell.
- Zustellung als lokale Benachrichtigung der Tauri-App. Das Gerät kennt alle Aufgaben und plant Erinnerungen selbst, ohne Netz und ohne Push-Server.
- Fallback, falls E4 sich verzögert: Web-Push.
- Server-Push (APNs) erst, wenn ein Agent aktiv benachrichtigen soll.

## 7. Oberfläche und Modi

Modi sind Sichten auf dieselben Daten, keine getrennten Apps.

| Sicht | Inhalt |
|---|---|
| **Heute** | Fällig und überfällig, Erinnerungen von heute, laufende Flamme. Startbildschirm. |
| **Inbox** | Uneingeordnetes. Pro Aufgabe zuordnen, lose markieren oder verwerfen, jeweils mit einer Taste bzw. einem Wisch. |
| **Lose** | Bewusst ungeordnete Aufgaben. |
| **Bereiche → Projekte** | Baum links, Aufgaben rechts. Projekte als Liste, Dev-Projekte standardmäßig als Board (Spalten = Status). Umschaltbar. |
| **Dev** | Board über alle Dev-Projekte mit Reservierungen, Branch und PR. |
| **Phönix** | Aktuelles Feuer: Feuer-Schritte und Flammen über alle Bereiche, mit „Flamme starten“. |
| **Suche** | Befehlspalette (⌘K) für Aufgaben, Projekte und Aktionen. |

- **Schnellerfassung** überall per Tastenkürzel bzw. großem ＋ auf dem Handy. Ein Titel genügt, die Aufgabe landet in der Inbox. Optionale Kurzschreibweise: `Zahnarzt anrufen morgen 9:00 #gesundheit` setzt Erinnerung und Zuordnung.
- **Aufgabendetail** als Seitenpanel, alles inline editierbar, Unteraufgaben als Baum, Änderungsprotokoll unten.
- Mobil: Sichten als Tabs unten.
- Desktop: vollständig per Tastatur bedienbar.

**Bewusst nicht drin:** Dashboards, Whiteboard, Chat, Gantt, Kalenderansicht, mehrere Workspaces, Einstellungsseiten jenseits von Login und Tokens.

## 8. Umstieg von ClickUp (harter Schnitt)

**Import (`zunder-import`, ClickUp-REST-API mit persönlichem Token)**

1. **Probelauf:** Der Import liest Spaces, Ordner und Listen und schlägt eine Zuordnungsdatei vor (Liste → Bereich/Projekt, Status → neuer Workflow). Sie wird geprüft und korrigiert, bevor etwas geschrieben wird.
2. **Bericht:** Mengen je Bereich und Projekt, nicht zuordenbare Status, Aufgaben ohne Ziel.
3. **Import:** alle Aufgaben samt Unteraufgaben, auch geschlossene (als `done`/`dropped`), jeweils mit `clickup_id`.
4. **Komplettsicherung:** Roh-JSON-Export inklusive Zeiteinträgen in ein privates Backup außerhalb des Repos.

**Skill-Umstellung**

- Die Werkzeuge spiegeln die genutzten ClickUp-Aufrufe (Abschnitt 5).
- Status-Namen stehen in den Skills einmal zentral statt verstreut.
- Zeiteinträge ziehen auf Timing um.
- Die Umstellung passiert in einem eigenen Branch und wird gegen die echte Zunder-Instanz getestet.

**Weitere ClickUp-Clients**

- Die bestehende iOS/watchOS-Capture-App **Funke** schreibt heute direkt nach ClickUp und bricht beim harten Schnitt. Vor E3 wird entschieden: Task-Pfad auf die Zunder-Sync-API umhängen oder Funke durch die Tauri-App (E4) ablösen.

**Ablauf am Umstiegstag**

1. ClickUp einfrieren (nichts Neues mehr anlegen).
2. Finaler Import.
3. Skill-Branch mergen, ClickUp-MCP aus den Skills entfernen.
4. Eine Woche Sicherheitsnetz mit Lesezugriff auf ClickUp, danach Kündigung.

Einen Rückweg nach ClickUp gibt es nicht. Das Sicherheitsnetz sind das Roh-Backup und die Woche Lesezugriff.

## 9. Qualität und Betrieb

**Tests**

- `zunder-core` testgetrieben. Property-Tests für den Sync: Dieselben Operationen in beliebiger Reihenfolge ergeben immer denselben Zustand.
- Server: Integrationstests gegen die HTTP-API.
- MCP: Vertragstests, die die Werkzeug-Schemas stabil halten.
- Import: Tests gegen aufgezeichnete, anonymisierte bzw. synthetische ClickUp-Antworten. Keine echten Daten im Repo.
- Web-UI: Playwright für die kritischen Abläufe (erfassen, Inbox abarbeiten, Board verschieben, Flamme starten). Das Tempo-Budget wird in der CI gemessen.

**CI (GitHub Actions):** `cargo fmt --check`, `cargo clippy -- -D warnings`, Tests, Container-Image nach GHCR.

**Betrieb:** ein Container hinter einem Reverse Proxy, Health-Endpoint, SQLite (WAL) mit nächtlichem Backup.

## 10. Etappen

| Bis | Etappe | Ergebnis |
|---|---|---|
| ~25.10.2026 | **E1 Fundament** | Kern, Sync, Server, Login, MCP-Grundwerkzeuge, Import-Probelauf |
| ~08.11.2026 | **E2 Oberfläche** | Web-UI mit allen Sichten, Timing-Brücke, MCP-Parität |
| ~22.11.2026 | **E3 Umstieg** | Skill-Branch, finaler Import, Umschalten, eine Woche Sicherheitsnetz |
| danach | **E4 Apps** | Tauri Desktop + iOS/Android, offline-first, lokale Erinnerungen |

**Später, ohne Termin**

- Automatischer Vault-Abgleich (Stufe 2).
- Automatische Einordnung von Inbox-Aufgaben in Projekte über ein System-One-Modell (z. B. Jev oder ein offener Nachbau): typisierte Frage „Zu welchem Projekt gehört das?“, Antwort ist ein Projekt oder „nichts Passendes“ mit Konfidenz. Hinter einer Schnittstelle, austauschbar.
- Wiederkehrende Aufgaben.

## 11. Entscheidungen und verworfene Alternativen

| Entscheidung | Verworfen | Grund |
|---|---|---|
| Rust-Kern + Tauri 2 + TypeScript-UI | Alles in Rust (Leptos/Dioxus) | Mobile-Reife weniger erprobt, Agenten schreiben TypeScript-UIs zuverlässiger, höheres Risiko für den Stichtag |
| Last-Writer-Wins pro Feld | CRDT-Bibliothek (Loro, Automerge) | Ein Nutzer, seltene Konflikte; Komplexität ohne Gewinn |
| SQLite | PostgreSQL | Ein Nutzer, triviale Backups, ein Container |
| Ein Status-Workflow | Status je Liste wie in ClickUp | Fest verdrahtete Status-Namen waren eine Fehlerquelle in den Skills |
| Kein eigenes Zeittracking | Zeittracking in Zunder | Doppelpflege mit Timing |
| Geteilte Identität mit dem Vault | Vault führt die Struktur / Aufgaben als Markdown | Ordnerstruktur ändert sich regelmäßig; Markdown verträgt sich schlecht mit schneller Mobile-Erfassung |
| Eigenbau | Bestehende Self-hosted-Tools | Bewusste Entscheidung: Modi, Phönix-Querschnitt, Vault-Identität und Dev-Modus mit MCP passen in kein fertiges Tool ohne ähnlichen Anpassungsaufwand |

## 12. Risiken

- **Abbruch-Muster:** Frühere Todo-App-Anläufe sind eingeschlafen. Gegenmittel: echter Umstieg mit Stichtag statt Testprojekt, kritischer Pfad vor neuen Features.
- **Skill-Umstellung unterschätzt:** Sie steht als eigene Etappe auf dem kritischen Pfad (E3).
- **Sync-Fehler zerstören Vertrauen:** Property-Tests im Kern, idempotente Operationen, keine stillen Ablehnungen.
- **Mobile verzögert sich:** Nicht umstiegskritisch, weil ClickUp mobil nicht genutzt wird. Erinnerungen haben Web-Push als Fallback.
- **Hobby-Drift:** Alles außerhalb von E1–E3 wartet bis nach dem Umstieg.
