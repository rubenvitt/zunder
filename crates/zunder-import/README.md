# zunder-import

Kommandozeilenwerkzeug für den einmaligen Umzug von ClickUp nach Zunder: Komplettsicherung, Zuordnungsvorschlag, Bericht und Import. Grundlage sind Abschnitt 8 der [Spec](../../docs/superpowers/specs/2026-10-11-zunder-design.md) und Abschnitt 6 des [Implementierungsvertrags](../../docs/superpowers/plans/2026-10-11-zunder-implementation.md).

```
zunder-import backup   Roh-JSON: Struktur, Aufgaben, Kommentare, Anhang-Metadaten, Zeiteinträge
zunder-import plan     ClickUp-Struktur lesen, mapping.toml vorschlagen
zunder-import report   zeigen, was ein Import täte (Mengen, Lücken)
zunder-import run      nach Zunder schreiben (idempotent)
```

## Vorbereitung

1. Bauen: `cargo build --release -p zunder-import` (Ergebnis: `target/release/zunder-import`).
2. Persönlichen ClickUp-Token anlegen (ClickUp → Einstellungen → Apps → API Token, beginnt mit `pk_`) und bereitstellen:
   ```sh
   export CLICKUP_TOKEN=pk_…
   ```
   Alternativ bei `backup`, `plan` und `report` mit `--token` übergeben, bei `run` mit `--clickup-token`.
3. In Zunder unter „Tokens“ einen API-Token anlegen (`zt_…`) und zusammen mit der Server-Adresse bereitstellen:
   ```sh
   export ZUNDER_URL=https://zunder.example.org
   export ZUNDER_TOKEN=zt_…
   ```
   Alternativ `run --server … --token …`.

Sieht der ClickUp-Token mehrere Workspaces, wählt `plan --team <id>` einen aus; `report` und `run` nehmen den Workspace aus der Zuordnungsdatei.

## Schritt für Schritt

### 1. Komplettsicherung

```sh
zunder-import backup
```

Schreibt Roh-JSON nach `~/zunder-import-backup/<Zeitstempel>/`:

- Workspaces, Spaces, Ordner, Listen (mit Status) und Kommentare an Listen,
- alle Aufgaben inklusive geschlossener, archivierter und Unteraufgaben sowie Eltern-Aufgaben außerhalb der Listen,
- je Aufgabe die vollständige Antwort von `GET /task/{id}` (`tasks/<id>/task.json`, darin Checklisten und die Metadaten der Anhänge: Titel, Größe, URL),
- je Aufgabe alle Kommentare samt Antworten in Threads (`tasks/<id>/comments.json`, nur wenn es welche gibt; Antworten stehen unter `"replies"` am jeweiligen Kommentar),
- alle Zeiteinträge.

Nicht gesichert werden die Dateien der Anhänge selbst (nur ihre Metadaten; die URLs verfallen mit dem Workspace), ClickUp Docs, Chat, Whiteboards und Ziele. Kommentare übernimmt der Import nicht nach Zunder (Zunder hat keine Kommentare); sie stehen nur in der Sicherung.

Das Verzeichnis ist nur für den eigenen Benutzer lesbar; `manifest.json` fasst die Mengen zusammen (u. a. `comments`, `comment_replies`, `tasks_with_comments`, `attachments`). Details und Kommentare kosten zwei Anfragen je Aufgabe; bei ClickUps Limit von 100 Anfragen pro Minute dauern 3000 Aufgaben rund eine Stunde.

- `--out <dir>` wählt ein anderes Basisverzeichnis (der Unterordner mit Zeitstempel kommt dazu).
- Liegt das Ziel in einem Git-Repository, bricht der Befehl ab, denn die Sicherung enthält private Daten. `--force` hebt das auf.
- Zeiteinträge liefert ClickUp ohne Zeitraum nur für die letzten 30 Tage. Deshalb wird der ganze Zeitraum ab `--since` (Standard `2017-01-01`) in Fenstern von `--window-days` Tagen (Standard 30) abgefragt.

Die Sicherung vor dem finalen Import wiederholen.

### 2. Zuordnung vorschlagen

```sh
zunder-import plan --out mapping.toml
```

Liest Spaces, Ordner, Listen und deren Status (auch archivierte) und schreibt eine kommentierte TOML-Datei:

| ClickUp | Vorschlag in Zunder |
|---|---|
| Space | Bereich (archivierter Space → `state = "archived"`) |
| Ordner | Projekt im Bereich des Spaces; alle Listen des Ordners landen dort |
| Liste ohne Ordner | eigenes Projekt; heißt sie wie der Space oder allgemein („Allgemein“, „Tasks“, „Liste“ …), direkt der Bereich; „Inbox“/„Eingang“ → Zunder-Inbox |
| archivierter Ordner / archivierte Liste | Projekt mit `status = "completed"` |
| Status | ClickUp-Statustyp `closed`/`done` → immer `done` (`dropped`, wenn der Name nach Abbruch klingt: `cancelled`, `verworfen` …), auch bei Namen wie „Reviewed“; sonst nach Namen: `to do`, `open`, `backlog` → `open`; `in progress`, `doing` → `in_progress`; `review`, `qa` → `review`; `blocked`, `waiting`, `on hold` → `blocked`; `complete`, `done`, `closed` → `done`; `cancelled`, `verworfen` → `dropped`; übrige Status vom Typ `open` → `open` |

Unbekannte Status bleiben leer und sind mit `# UNBEKANNT` markiert. Eine vorhandene Datei wird nur mit `--force` überschrieben.

### 3. Zuordnung bearbeiten

Die Datei ist zum Bearbeiten gedacht. Typische Änderungen:

```toml
[projects.werkzeugkiste]
name = "Werkzeugkiste"
area = "werkstatt"
kind = "dev"                       # Board, bug/feature/review erlaubt
repo = "beispiel/werkzeugkiste"    # nur bei kind = "dev"

[areas.zuhause]
name = "Zuhause"
clickup_id = "90010"
existing_id = "01929c3e-…"         # auf einen bestehenden Zunder-Bereich abbilden

[lists."9002101"]
path = "Werkstatt › Werkzeugkiste › Backlog"
target = "project:werkzeugkiste"   # area:<schlüssel> | project:<schlüssel> | inbox | loose | skip
[lists."9002101".statuses]
"eingefroren" = "blocked"          # vorher leer
```

- **IDs:** Bereiche und Projekte bekommen stabile IDs aus ihrer `clickup_id` (UUIDv5). Mit `existing_id` legt der Import nichts an und verweist auf den vorhandenen Eintrag (er muss in Zunder existieren). Bei Projekten muss `kind` dann der Art des vorhandenen Projekts entsprechen, denn davon hängen die Dev-Regeln ab; `run` bricht sonst vor dem Schreiben ab. Eigene Bereiche oder Projekte ohne `clickup_id` sind erlaubt; ihre ID entsteht aus dem Schlüssel.
- **Ziele:** `skip` lässt eine Liste weg, `loose` legt ihre Aufgaben unter „Lose“ ab.
- **Typen:** `[type_tags]` übersetzt ClickUp-Tags in `bug` oder `feature`.
- **Dev-Regeln:** `review`, `bug` und `feature` gibt es nur in Dev-Projekten. Anderswo wird daraus `in_progress` bzw. `aufgabe`; der Bericht listet diese Fälle.
- **Einstellungen:** `timezone` (Standard `Europe/Berlin`) bestimmt, wie Fälligkeiten gelesen werden; `import_archived_tasks = true` importiert auch archivierte ClickUp-Aufgaben.

Tippfehler in Schlüsseln, unbekannte Ziele oder Status meldet jeder Befehl gesammelt.

### 4. Bericht prüfen

```sh
zunder-import report
zunder-import report --json > bericht.json
```

Zeigt die Mengen je Bereich und Projekt (Aufgaben, Unteraufgaben, offen und geschlossen) und listet:

- **nicht zugeordnete Status** und **Aufgaben ohne Ziel** (Liste fehlt in der Datei oder `target` ist leer). Beides blockiert `run`.
- Unteraufgaben, deren Eltern-Aufgabe fehlt oder nicht importiert wird. Sie werden als eigenständige Aufgaben ihrer Liste importiert.
- `bug`/`feature`/`review` außerhalb von Dev-Projekten, angepasste Titel (leer oder länger als 500 Zeichen) und Listen aus der Datei, die es in ClickUp nicht mehr gibt.

So lange wiederholen, bis am Ende „Die Zuordnung ist vollständig“ steht.

### 5. Probelauf und Import

```sh
zunder-import run --dry-run      # zählt nur, schreibt nichts
zunder-import run
```

Der Import

1. liest alle Aufgaben samt Unteraufgaben (beliebige Tiefe) und geschlossener Aufgaben,
2. holt den aktuellen Stand aus Zunder (`GET /api/snapshot`) und berechnet nur die nötigen Änderungen,
3. teilt die Operationen in Pakete (`--batch-size`, Standard 500 Operationen, höchstens 7 MiB je Anfrage): zuerst Typ- und Statusänderungen vorhandener Aufgaben, die überall erlaubt sind (etwa `bug` → `aufgabe`, wenn ein Projekt kein Dev-Projekt mehr ist), dann Bereiche, Projekte und Aufgaben, Eltern vor ihren Unteraufgaben; eine neu angelegte Entität wird nie auf zwei Pakete verteilt,
4. prüft vorab jedes Paket mit denselben Regeln wie der Server, in derselben Reihenfolge auf einer Kopie des Snapshots (der Server prüft jedes Paket für sich); lehnt die Prüfung etwas ab oder ist ein einzelner Eintrag größer als 7 MiB (meist eine sehr lange Beschreibung), wird **nichts** geschrieben,
5. schreibt die Pakete über `POST /api/sync/push`.

Lehnt der Server dennoch etwas ab, bricht der Lauf nach dem Paket ab; bereits geschriebene Pakete bleiben, ein neuer Lauf setzt fort. `--keep-going` schreibt trotz Ablehnungen weiter und lässt zu große Einträge aus. Antwortet der Server mit HTTP 413, hilft eine kleinere `--batch-size`. Hinweise des Servers (z. B. „erledigt, aber Unteraufgaben offen“) werden am Ende gezeigt.

Übernommen werden je Aufgabe: Titel, Beschreibung (Markdown, sonst Klartext), Status laut Zuordnung, Typ (Tags), Stelle (Ziel der Liste; Unteraufgaben über `parent_id`), Fälligkeit, ein Link „ClickUp“ zur Original-Aufgabe, `clickup_id` und die Reihenfolge (`rank` aus dem ClickUp-`orderindex`). Fälligkeiten mit Uhrzeit werden zu Datum und Uhrzeit in der eingestellten Zeitzone, reine Datumsangaben zum Kalenderdatum. Fehlt in der ClickUp-Antwort die Angabe `due_date_time`, gilt 00:00 oder 04:00 Ortszeit als „nur Datum“.

Neu angelegte Aufgaben tragen ihre ClickUp-Zeitpunkte: Die Operationen sind auf `date_created` datiert, der Status geschlossener Aufgaben auf `date_closed` (sonst `date_done`, sonst `date_updated`), jeweils höchstens auf den Importzeitpunkt. So bleibt die Erstellungszeit erhalten, und eine vor Monaten erledigte Aufgabe gilt nicht drei Tage lang als „gerade erledigt“ (sie erscheint also nicht in den Standardansichten und nicht in `filter_tasks` ohne `include_closed`). Änderungen an schon vorhandenen Aufgaben tragen dagegen den Importzeitpunkt, damit sie gegen den Stand in Zunder gewinnen.

### 6. Erneut importieren

Aufgaben-IDs werden aus der ClickUp-ID abgeleitet (UUIDv5), daher aktualisiert ein erneuter Lauf statt zu duplizieren:

- Unveränderte Aufgaben erzeugen keine Operation; ein Lauf ohne Änderungen in ClickUp schreibt nichts.
- Bei den vom Import verwalteten Feldern (Titel, Beschreibung, Typ, Status, Stelle, Eltern, Fälligkeit, Links, Rang, `clickup_id`) gewinnt ClickUp. Ist ein Feld in ClickUp leer geworden, wird es in Zunder geleert.
- Alles andere, was in Zunder ergänzt wurde (Erinnerung, Phönix, Vault-Links, Plan …), bleibt unberührt.
- Bereiche und Projekte der Zuordnung ohne `existing_id` schreibt jeder Lauf auf den Stand aus `mapping.toml` (Bereich: Name, Zustand; Projekt: Name, Bereich, Status, Art, Repo), auch wenn alle ihre Listen auf `skip` stehen. Was in Zunder seitdem geändert wurde und so bleiben soll, bekommt vorher `existing_id`.
- In Zunder gelöschte Einträge bleiben gelöscht.

## Umstiegstag

1. ClickUp einfrieren (nichts Neues mehr anlegen).
2. `zunder-import backup`
3. `zunder-import report` (muss vollständig sein)
4. `zunder-import run`
5. Stichproben in Zunder; über `clickup_id` finden auch MCP-Werkzeuge die alten Verweise.

## Entwicklung

```sh
cargo test -p zunder-import
```

Die Tests laufen gegen synthetische, frei erfundene ClickUp-Antworten (`tests/fixtures/clickup/`), die ein lokaler Mock-Server ausliefert (Seiten, Rate-Limit mit 429, archivierte Einträge, fehlende Eltern), und gegen einen Zunder-Mock, der die echte Validierung aus `zunder-core` anwendet. Echte Daten gehören nicht ins Repository.

Gegen den echten Server (ignoriert, solange nicht ausdrücklich angefordert): `tests/server_e2e.rs` startet die gebaute `zunder-server`-Binärdatei mit `ZUNDER_BOOTSTRAP_TOKEN`, legt wie die Web-UI einen API-Token namens `import` an, ruft die echte CLI `zunder-import run` gegen den ClickUp-Mock auf und prüft Mengen, Felder, die Quelle `api:import` im Verlauf, `api:bootstrap` für den Bootstrap-Token und dass ein zweiter Lauf nichts schreibt.

```sh
cargo build -p zunder-server -p zunder-import
ZUNDER_SERVER_BIN=$PWD/target/debug/zunder-server \
  cargo test -p zunder-import --test server_e2e -- --ignored
```

Aufbau:

| Modul | Inhalt |
|---|---|
| `clickup` | Trait `ClickUpApi` (Rohdaten als JSON), `HttpClickUp` mit Rate-Limit-Behandlung, typisierte Sichten |
| `fetch` | Hierarchie, Aufgaben über alle Seiten, fehlende Eltern, Kommentare (Seiten, Antworten) |
| `mapping` | Zuordnungsdatei: Vorschlag, TOML-Ausgabe, Prüfung |
| `convert` | ClickUp-Aufgaben → gewünschter Zunder-Zustand und Befunde für den Bericht |
| `ops` | Abgleich mit dem Snapshot, Operationen mit HLC-Knoten `import`, Pakete |
| `zunder` | Client für `GET /api/snapshot` und `POST /api/sync/push` |
| `plan`, `report`, `run`, `backup` | die Befehle |
