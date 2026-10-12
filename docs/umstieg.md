# Umstieg von ClickUp (Etappe E3)

Runbook für den harten Schnitt von ClickUp auf Zunder, nach Abschnitt 8 der [Spec](superpowers/specs/2026-10-11-zunder-design.md). Ziel: ClickUp ist gekündigt, alle Aufgaben sind importiert, alle Skills laufen gegen den Zunder-MCP.

Einen Rückweg nach ClickUp gibt es nicht. Das Sicherheitsnetz sind das Roh-Backup und eine Woche Lesezugriff.

## Überblick

| Wann | Was |
|---|---|
| bis Start E3 | Funke auf Zunder umgestellt bauen und testen (siehe [unten](#funke)); claude.ai-Connector verbinden ([`betrieb.md`](betrieb.md#claudeai), OAuth ist umgesetzt) |
| Vorbereitung | Probelauf, Zuordnung prüfen, Probeimport in eine Testinstanz, Skill-Branch |
| Umstiegstag | ClickUp einfrieren, Komplettsicherung, finaler Import, Skills umschalten |
| +1 Woche | Sicherheitsnetz mit Lesezugriff auf ClickUp |
| danach | Kündigung |

## 1. Vorbereitung

### Voraussetzungen

- Zunder läuft produktiv unter `https://zunder.rubeen.dev`, Login über Pocket-ID funktioniert ([`betrieb.md`](betrieb.md)).
- In Zunder ist ein API-Token **`import`** angelegt.
- Ein persönliches ClickUp-API-Token (`pk_…`, in ClickUp unter **Einstellungen → Apps → API Token**).
- Ein Arbeitsverzeichnis **außerhalb des Repos**, z. B. `~/zunder-umstieg/`. Zuordnungsdatei und Backup enthalten echte Namen und Inhalte und gehören nicht ins Repo.

### zunder-import aufrufen

Variante A, aus dem Repo:

```sh
cargo build --release -p zunder-import
alias zunder-import="$PWD/target/release/zunder-import"
```

Variante B, aus dem Container-Image (keine Rust-Toolchain nötig):

```sh
alias zunder-import='docker run --rm -it --user "$(id -u):$(id -g)" \
  -e CLICKUP_TOKEN -e ZUNDER_URL -e ZUNDER_TOKEN \
  -v "$PWD:/work" -w /work --entrypoint zunder-import ghcr.io/rubenvitt/zunder:latest'
```

Konfiguration über Umgebungsvariablen (oder gleichnamige Optionen, z. B. `--clickup-token`):

```sh
cd ~/zunder-umstieg
export CLICKUP_TOKEN=pk_…              # ClickUp, lesend
export ZUNDER_URL=https://zunder.rubeen.dev
export ZUNDER_TOKEN=zt_…               # Token "import"
```

Hilfe zu allen Optionen: `zunder-import --help` bzw. `zunder-import <befehl> --help`.

### Schritt 1: Komplettsicherung (erste)

```sh
zunder-import backup --out ~/zunder-umstieg/backup-$(date +%F)
```

Roh-JSON aller Spaces, Ordner, Listen, Aufgaben (auch geschlossene, je Aufgabe mit vollständigen Details und Anhang-Metadaten), Kommentare samt Antworten und Zeiteinträge. Kommentare übernimmt der Import nicht nach Zunder; die Sicherung ist ihr einziger Ort. Das ist die erste Sicherung; am Umstiegstag folgt die finale. Das Verzeichnis verschlüsselt aufbewahren (z. B. in einem verschlüsselten Archiv im Vault-Backup), nie im Repo.

### Schritt 2: Probelauf → Zuordnungsdatei

```sh
zunder-import plan --out mapping.toml
```

Sieht der ClickUp-Token mehrere Workspaces, bricht `plan` mit „Mehrere Workspaces sichtbar, bitte mit --team wählen“ ab und listet sie auf. Dann den Workspace als Option angeben: `zunder-import plan --team <workspace-id> --out mapping.toml` (eine Umgebungsvariable dafür gibt es nicht). `report` und `run` lesen den Workspace danach aus `mapping.toml`.

Liest Spaces, Ordner und Listen und schlägt eine Zuordnung vor:

- **Liste → Ziel**: Bereich, Projekt (mit Bereich), Inbox, Lose oder überspringen. Neue Bereiche und Projekte legt der Import an; bestehende Zunder-Bereiche/-Projekte lassen sich über ihre ID wiederverwenden.
- **Status → Workflow**: jeder in ClickUp vorkommende Status auf einen Zunder-Code (`open`, `in_progress`, `review`, `blocked`, `done`, `dropped`).
- **Dev-Projekte**: Listen, die Software-Projekte sind, als `kind = "dev"` mit `repo = "owner/name"` markieren. Nur dort sind `bug`, `feature` und `review` erlaubt.

`mapping.toml` von Hand prüfen und korrigieren, bevor irgendetwas geschrieben wird. Typische Korrekturen: Listen zusammenlegen, Listen ohne Bedeutung auf Inbox, Status wie „waiting“ auf `blocked`.

### Schritt 3: Bericht

```sh
zunder-import report --mapping mapping.toml
```

Zeigt Mengen je Bereich und Projekt, **nicht zuordenbare Status** und **Aufgaben ohne Ziel**. Zielzustand vor dem Import:

- [ ] keine nicht zuordenbaren Status
- [ ] keine Aufgaben ohne Ziel (oder bewusst als Inbox/Lose zugeordnet)
- [ ] Mengen plausibel (Stichprobe gegen ClickUp)

Schritte 2 und 3 so oft wiederholen, bis der Bericht sauber ist.

### Schritt 4: Probeimport in eine Testinstanz

Nicht gegen die Produktion: Ein Probeimport dort würde Aufgaben schon vor dem Schnitt sichtbar machen, und spätere Änderungen in Zunder würde der finale Import wieder überschreiben (er trägt den neueren Zeitstempel).

```sh
# Testinstanz lokal, eigener Datenordner, Login ohne OIDC
ZUNDER_DEV_AUTH=true ZUNDER_DATA_DIR=/tmp/zunder-probe ZUNDER_BOOTSTRAP_TOKEN=zt_probe… \
  cargo run --release -p zunder-server

# zweites Terminal
ZUNDER_URL=http://127.0.0.1:8080 ZUNDER_TOKEN=zt_probe… \
  zunder-import run --mapping mapping.toml
```

`ZUNDER_BOOTSTRAP_TOKEN` muss das Token-Format `zt_<32 Byte base64url>` haben, z. B. `zt_$(openssl rand -base64 32 | tr '+/' '-_' | tr -d '=')`.

Prüfen in der Web-UI der Testinstanz (`http://127.0.0.1:8080`):

- [ ] Bereiche und Projekte vollständig, richtig verschachtelt
- [ ] Stichproben: Titel, Beschreibung (Markdown), Fälligkeit, Unteraufgaben-Baum
- [ ] Geschlossene Aufgaben als `done`/`dropped` vorhanden und über die Suche auffindbar
- [ ] `get_task` mit einer `clickup_id` aus dem Vault findet die Aufgabe
- [ ] Zweiter `run` mit derselben Zuordnung erzeugt keine Duplikate (Idempotenz über `clickup_id`)

Abgelehnte Operationen meldet `run` mit Code und Grund. Jede Ablehnung klären (meist Zuordnung korrigieren), bis der Probeimport ohne Ablehnungen durchläuft.

### Schritt 5: Skill-Branch

Im Skill-Repo einen Branch `zunder` anlegen und dort umstellen (Anleitung in [Abschnitt 3](#3-skills-umstellen)). Getestet wird gegen die echte Zunder-Instanz, aber nur lesend und mit eigens angelegten Test-Aufgaben in einem Bereich „Test“, der nach dem Umstieg gelöscht wird.

## 2. Umstiegstag

### Checkliste

- [ ] **ClickUp einfrieren:** nichts Neues mehr anlegen oder ändern. Laufende ClickUp-Timer stoppen. Automationen und Integrationen in ClickUp deaktivieren.
- [ ] **Funke** mit Zunder-Server-URL und eigenem API-Token auf iPhone und Watch installiert und gegen die Produktion getestet (siehe [unten](#funke)).
- [ ] **Finale Komplettsicherung:** `zunder-import backup --out ~/zunder-umstieg/backup-final-$(date +%F)`
- [ ] **Bericht** gegen den eingefrorenen Stand: `zunder-import report --mapping mapping.toml` ist sauber.
- [ ] **Manuelles Zunder-Backup** vor dem Import ([`betrieb.md`](betrieb.md#manuelles-backup-z-b-vor-einem-update)).
- [ ] **Finaler Import** gegen die Produktion: `zunder-import run --mapping mapping.toml` (mit `ZUNDER_URL=https://zunder.rubeen.dev` und dem Token `import`). Keine Ablehnungen.
- [ ] **Stichproben** in der Produktion wie beim Probeimport, dazu Mengen aus dem Bericht gegen die Web-UI.
- [ ] **Skill-Branch mergen**, ClickUp-MCP aus den Skills entfernen.
- [ ] **ClickUp-MCP abklemmen** in Claude Code (`claude mcp remove clickup`) und in claude.ai (Connector entfernen); Zunder-MCP verbunden ([`betrieb.md`](betrieb.md#mcp-verbinden)).
- [ ] **Smoke-Test der Skills:** jeder umgestellte Skill einmal gegen die Produktion (lesen, eine Test-Aufgabe anlegen, ändern, verschieben, löschen).
- [ ] Token `import` widerrufen.
- [ ] Bereich „Test“ samt Test-Aufgaben löschen.

**Erinnerungen ab dem Umstieg:** Zunder stellt Erinnerungen nur über die Tauri-App zu (Desktop, solange sie läuft; mobil erst mit einem auf dem Gerät geprüften Build). Die Web-UI im Browser benachrichtigt nicht, sie zeigt fällige Erinnerungen nur in „Heute“. Einen Web-Push-Fallback gibt es nicht (Vertrag, Abschnitt 5.1 App-Befehle).

**Links auf Aufgaben im Vault:** `https://zunder.rubeen.dev/#/aufgabe/<id>` öffnet eine Aufgabe, `https://zunder.rubeen.dev/#/clickup/<clickup_id>` eine importierte über ihre alte ClickUp-ID. Alte `app.clickup.com/t/<id>`-Links lassen sich so ersetzen.

## 3. Skills umstellen

Die Zunder-Werkzeuge spiegeln die genutzten ClickUp-Aufrufe. Die Umstellung ist weitgehend mechanisch: Werkzeugname tauschen, Parameter umbenennen, Statusnamen aus einem zentralen Block beziehen.

### Zuordnung der Werkzeuge

| ClickUp-MCP | Zunder-MCP | Hinweise |
|---|---|---|
| `clickup_filter_tasks` | `filter_tasks` | `list_ids` → `project_id`, `area_id` oder `location` (`inbox`, `loose`, `area`, `project`). `statuses` → `status[]` mit englischen Codes. `due_date_gt`/`due_date_lt` → `due_after`/`due_before`. `date_closed_gt`/`date_closed_lt` → `closed_after`/`closed_before`. `page` → `offset` (Seitengröße `limit`, höchstens 500; das Ergebnis nennt `next_offset`). `order_by`/`reverse` → `order_by` (`due`, `created`, `updated`, `status_changed`) und `reverse`. Geschlossene nur mit `include_closed: true` (oder einem `closed_*`-Filter). Zuständige, Tags und Prioritäten gibt es nicht. Neu: `type[]`, `phoenix_role`, `phoenix_fire`, `parent_id` (UUID oder ClickUp-ID), `claimed_by`, `flame_running`. |
| `clickup_get_task` | `get_task` | `task_id` (Zunder-ID oder Kennung wie `ZUN-42`) **oder** `clickup_id`. Liefert den Unteraufgaben-Baum und den Fortschritt immer mit; ein Schalter für Unteraufgaben entfällt. |
| `clickup_create_task` | `create_task` | `list_id` → Stelle (`location` bzw. Projekt/Bereich); ohne Stelle landet die Aufgabe in der Inbox. `parent` → `parent_id` (die ClickUp-ID einer importierten Aufgabe funktioniert unverändert). `markdown_description` → `description`. `due_date` als `YYYY-MM-DD` oder RFC 3339. Priorität, Tags, Zuständige entfallen. |
| `clickup_update_task` | `update_task` | `task_id` oder `clickup_id` plus die zu ändernden Felder. Status als englischer Code. `null` setzt ein optionales Feld zurück. Verschieben läuft über `move_task`. |
| `clickup_move_task` | `move_task` | Neue Stelle und/oder neuer `parent_id` (ID, Kennung oder ClickUp-ID). Unteraufgaben wandern mit ihrer Wurzel. |
| `clickup_delete_task` | `delete_task` | Setzt einen Tombstone (Aufgabe samt Unteraufgaben) statt endgültig zu löschen; der Verlauf bleibt über `get_task_history` lesbar. **Nicht rückgängig zu machen** über MCP oder die Oberfläche: Alle anderen Werkzeuge melden die Aufgabe als gelöscht. Das Datenmodell kann zwar wiederherstellen (`deleted: false`, [Vertrag](superpowers/plans/2026-10-11-zunder-implementation.md) Abschnitt 2.3), aber nur über eine rohe Operation an `/api/sync/push`. Skills sollten deshalb vor dem Löschen nachfragen oder lieber auf `dropped` setzen. |
| `clickup_search` | `search` | Volltext über Titel, Beschreibung und Plan, auch erledigte Aufgaben. Trifft zusätzlich Projekte und Bereiche. |
| `clickup_create_task_comment` | – | Zunder hat **keine Kommentare** (bewusst, Spec Abschnitt 2). Ersatz siehe unten. |
| `clickup_add_time_entry`, `clickup_start_time_tracking` (und Stoppen) | `start_flame` / `stop_flame` oder Timing direkt | Zeiterfassung liegt bei **Timing**. An einer Aufgabe: `start_flame`/`stop_flame` startet bzw. stoppt über die Timing-Brücke einen Timer und merkt sich `timing_ref`. Wie `clickup_stop_time_tracking` braucht `stop_flame` keine Aufgabe: ohne `task_id`/`clickup_id` endet die laufende Flamme; `filter_tasks {flame_running: true}` ersetzt `clickup_get_current_time_entry`. Nachträgliche Einträge ohne Aufgabenbezug direkt über den Timing-MCP (`create_time_entry`, `start_timer`, `stop_timer`). |
| Hierarchie-/Listen-Abfragen (falls genutzt) | `list_structure` | Bereiche und Projekte mit IDs, statt Spaces/Ordner/Listen. |

Für den Dev-Modus kommen `claim_task`, `release_task` und `decompose_task` dazu, für die Struktur `create_area`, `update_area`, `create_project`, `update_project`, für den Verlauf `get_task_history`.

### Ersatz für Kommentare

Je nach Zweck des bisherigen Kommentars:

| Bisheriger Kommentar | In Zunder |
|---|---|
| Notiz, Zwischenstand, Kontext | an die Beschreibung anhängen: `update_task` mit `description` = bisherige Beschreibung + Abschnitt `## Notizen` mit Datum je Eintrag. Vorher `get_task` lesen, damit nichts überschrieben wird. |
| Ergebnis eines Agenten im Dev-Modus | strukturierte Felder `plan`, `branch`, `pr_url` per `update_task` |
| „Wer hat wann was geändert?“ | entfällt als Kommentar: Das Änderungsprotokoll (`get_task_history`) zeichnet jede Änderung mit Zeit und Quelle (`web`, `api:<token-name>`, z. B. `api:import` für den ClickUp-Import, `mcp:<token-name>`, `server`) automatisch auf. |
| Verweis auf eine Notiz | `vault_links` (Vault-Pfad oder `obsidian://`-URI) |
| Verweis auf eine Webseite | `links` mit `url` und optional `title` |

### Zentraler Statusblock für Skills

Statusnamen stehen in den Skills **einmal zentral** statt verstreut. Diesen Block in eine gemeinsame Datei der Skills übernehmen (z. B. `zunder-konventionen.md`) und von allen Skills aus darauf verweisen:

```markdown
## Zunder-Konventionen (zentral, nur hier pflegen)

Status (MCP-Code → Anzeige):
| Code          | Anzeige    | Bedeutung                                   |
|---------------|------------|---------------------------------------------|
| `open`        | Offen      | noch nicht begonnen (Standard)              |
| `in_progress` | In Arbeit  | wird gerade bearbeitet                      |
| `review`      | Review     | nur in Dev-Projekten: PR offen              |
| `blocked`     | Blockiert  | wartet auf etwas oder jemanden              |
| `done`        | Erledigt   | abgeschlossen                               |
| `dropped`     | Verworfen  | bewusst nicht gemacht                       |

Werkzeuge erwarten und liefern immer den Code, nie die Anzeige.
Übergänge sind frei (auch `done` → `open`); `review` nur in Dev-Projekten.
Erledigte und verworfene Aufgaben verschwinden nach 3 Tagen aus den
Standardansichten; `filter_tasks` braucht dafür `include_closed: true`.

Typen: `aufgabe` (Standard), `bug` und `feature` (nur Dev-Projekte).
Stellen (`location.kind`): `inbox`, `loose` (Lose), `area`, `project`.
Phönix: `phoenix.role` = `feuer_schritt` | `flamme`, `phoenix.fire` = ISO-Woche `JJJJ-Www`.
Aufgaben adressieren: `task_id` oder `clickup_id` (alte Verweise im Vault bleiben gültig).
```

Die Übersetzung alter ClickUp-Statusnamen auf diese Codes gehört nicht in die Skills, sondern nur in die `mapping.toml` des Imports.

### Vorgehen je Skill

1. Alle Vorkommen von `clickup_` im Skill suchen.
2. Werkzeug nach der Tabelle oben ersetzen, Parameter umbenennen.
3. Fest verdrahtete Statusnamen und Listen-IDs durch Verweise auf den zentralen Block bzw. `list_structure` ersetzen.
4. Kommentare und Zeiteinträge nach den Abschnitten oben ersetzen.
5. Gegen die echte Instanz testen (Bereich „Test“).

## 4. Eine Woche Sicherheitsnetz

- ClickUp bleibt **lesend** verfügbar: nicht mehr hineinschreiben, aber nachsehen, falls in Zunder etwas fehlt.
- Fehlt etwas: in Zunder von Hand nachtragen oder, bei größeren Lücken, `mapping.toml` korrigieren und `zunder-import run` erneut laufen lassen. Der Lauf aktualisiert über `clickup_id` statt zu duplizieren, **überschreibt dabei aber importierte Felder**, die seit dem Umstieg in Zunder geändert wurden. Was er anfasst:
  - **Aufgaben** nur aus Listen, die nicht auf `target = "skip"` stehen. Alle anderen Listen auf `skip` zu setzen, begrenzt also die Aufgaben auf die betroffene Liste.
  - **Bereiche und Projekte** dagegen immer **alle** aus der Zuordnung, egal welche Listen aktiv sind: beim Bereich Name und Zustand, beim Projekt Name, Bereich, Status, Art und Repo. Wer seit dem Umstieg in Zunder einen Bereich oder ein Projekt umbenannt, archiviert, abgeschlossen oder auf Dev umgestellt hat, trägt dort vorher `existing_id = "<Zunder-ID>"` ein (ID aus `list_structure` oder aus der Adresse `#/bereiche/project/<id>` in der Web-UI). Dann lässt der Import den Eintrag unverändert. Ohne das setzt er die Werte aus `mapping.toml` zurück; enthält ein inzwischen auf Dev umgestelltes Projekt Bugs, Features oder Reviews, bricht der Lauf sogar mit `dev_tasks_present` ab, ohne etwas zu schreiben.
  - Immer zuerst ein manuelles Zunder-Backup, dann `zunder-import run --dry-run`. Der Probelauf zeigt, wie viele Einträge sich ändern würden, und meldet Ablehnungen der lokalen Prüfung, ohne etwas zu schreiben.
- Jeden Tag kurz notieren, was gefehlt hat. Am Ende der Woche ist die Liste leer oder abgearbeitet.

## 5. Kündigung

- [ ] Sicherheitsnetz-Liste abgearbeitet, nichts fehlt mehr.
- [ ] Finale Komplettsicherung ist lesbar (Stichprobe: eine Aufgabe, ein Kommentar und ein Zeiteintrag im JSON gefunden; `manifest.json` zählt Kommentare) und verschlüsselt an zwei Orten abgelegt.
- [ ] Zeiteinträge, die noch gebraucht werden, sind in Timing.
- [ ] ClickUp-API-Token widerrufen, ClickUp-Connector überall entfernt.
- [ ] Funke schreibt nicht mehr nach ClickUp.
- [ ] ClickUp-Abo kündigen, Workspace löschen.
- [ ] In der Spec und im README den Umstieg als erledigt vermerken.

## Funke

Die iOS-/watchOS-Capture-App **Funke** schreibt heute direkt nach ClickUp und bricht beim harten Schnitt. Entschieden ist Option A (siehe Entscheidung am Ende des Abschnitts).

### Option A: Task-Pfad auf die Zunder-Sync-API umhängen

Funke ersetzt den ClickUp-Aufruf durch einen Push an Zunder. Die erfasste Aufgabe landet in der Inbox, wie bei der Schnellerfassung in der Web-UI.

- Endpoint: `POST https://zunder.rubeen.dev/api/sync/push`
- Auth: `Authorization: Bearer zt_…` mit eigenem Token je Gerät (z. B. `funke-iphone`), abgelegt im Schlüsselbund und per App Group mit der Watch geteilt.
- Anlegen = mehrere Feld-Operationen mit derselben `entity_id` in **einem** Push.
- IDs: seit Schema 4 CUID2-Form, 24 Zeichen `^[a-z][a-z0-9]{23}$` (z. B. ein zufälliger Kleinbuchstabe und 23 zufällige Zeichen aus `0-9a-z`). Eine kanonische UUID (klein, mit Bindestrichen; Swifts `UUID().uuidString` muss `lowercased()` werden) nimmt der Server weiter an und rechnet sie fest um; die Antwort nennt die IDs so, wie sie gesendet wurden. Ein Funke, der noch UUIDv7 erzeugt, läuft also unverändert weiter.
- HLC: `"<13 Stellen Unix-ms>:<5 Stellen Zähler>:<Knoten>"`, Knoten z. B. `funke-3f9a1c2e` (`[a-z0-9-]`, höchstens 36 Zeichen, je Gerät fest). Innerhalb einer Millisekunde zählt der Zähler hoch.
- Offline: Operationen in einer lokalen Warteschlange halten und **mit denselben `op_id`s** erneut senden. Der Server erkennt Wiederholungen und nimmt sie als `accepted` an, ohne sie doppelt zu speichern.

Push, den Funke für „Zahnarzt anrufen“ mit Erinnerung sendet:

```http
POST /api/sync/push HTTP/1.1
Host: zunder.rubeen.dev
Authorization: Bearer zt_…
Content-Type: application/json
```

```json
{
  "client_id": "funke-3f9a1c2e",
  "ops": [
    {
      "op_id": "01a1f805-eba1-782e-95ca-e75007201e12",
      "entity": "task",
      "entity_id": "01a1f805-eba0-77ee-a761-5ef35f30e49b",
      "field": "title",
      "value": "Zahnarzt anrufen",
      "hlc": "1795162500000:00000:funke-3f9a1c2e"
    },
    {
      "op_id": "01a1f805-eba2-717b-8fed-a7e1647796ff",
      "entity": "task",
      "entity_id": "01a1f805-eba0-77ee-a761-5ef35f30e49b",
      "field": "location",
      "value": { "kind": "inbox" },
      "hlc": "1795162500000:00001:funke-3f9a1c2e"
    },
    {
      "op_id": "01a1f805-eba3-722b-aa8e-d02a82a17593",
      "entity": "task",
      "entity_id": "01a1f805-eba0-77ee-a761-5ef35f30e49b",
      "field": "remind_at",
      "value": "2026-11-21T09:00:00+01:00",
      "hlc": "1795162500000:00002:funke-3f9a1c2e"
    }
  ]
}
```

`location` ist optional (Default Inbox), macht die Absicht aber explizit. `type` und `status` bleiben auf ihren Defaults (`aufgabe`, `open`). Antwort:

```json
{
  "accepted": [
    "01a1f805-eba1-782e-95ca-e75007201e12",
    "01a1f805-eba2-717b-8fed-a7e1647796ff",
    "01a1f805-eba3-722b-aa8e-d02a82a17593"
  ],
  "rejected": [],
  "warnings": [],
  "cursor": 4711
}
```

Steht etwas in `rejected` (mit `code` und deutscher `message`), wurde die Aufgabe nicht angelegt. Funke sollte das sichtbar melden und nicht still verwerfen. HTTP `401` heißt: Token ungültig oder widerrufen.

Skizze in Swift:

```swift
struct Op: Encodable {
    let op_id: String
    let entity = "task"
    let entity_id: String
    let field: String
    let value: AnyEncodable          // eigener Wrapper: String, Objekt oder null
    let hlc: String
}

struct PushRequest: Encodable {
    let client_id: String
    let ops: [Op]
}

/// UUIDv7: 48 Bit Unix-ms, Version 7, Variante RFC 9562, Rest zufällig.
func uuidV7(ms: UInt64) -> String {
    var b = [UInt8](repeating: 0, count: 16)
    for i in 0..<6 { b[i] = UInt8(truncatingIfNeeded: ms >> (8 * (5 - i))) }
    for i in 6..<16 { b[i] = UInt8.random(in: 0...255) }
    b[6] = (b[6] & 0x0F) | 0x70
    b[8] = (b[8] & 0x3F) | 0x80
    let hex = b.map { String(format: "%02x", $0) }.joined()
    let p = Array(hex)
    return [0..<8, 8..<12, 12..<16, 16..<20, 20..<32]
        .map { String(p[$0]) }.joined(separator: "-")
}

/// HLC im Wire-Format; `counter` zählt innerhalb derselben Millisekunde hoch.
func hlc(ms: UInt64, counter: Int, node: String) -> String {
    String(format: "%013llu:%05d:%@", ms, counter, node)
}
```

Aufwand: klein (ein HTTP-Aufruf, Token-Verwaltung, UUIDv7 und HLC). Funke bleibt die Watch-Erfassung und ist ab dem Umstiegstag einsatzbereit. Nachteil: ein weiterer Client, der mit dem Vertrag Schritt halten muss (nur die Push-Schnittstelle, kein Pull und kein Merge).

### Option B: Funke durch die Tauri-App ablösen

Die Tauri-App (E4) übernimmt die mobile Erfassung, Funke wird eingestellt.

- Kein zusätzlicher Code in Funke.
- E4 kommt **nach** dem Umstieg. Zwischen Umstiegstag und fertiger App gibt es keine mobile Erfassung außer der Web-UI im mobilen Browser.
- Tauri 2 unterstützt iOS und Android, aber **kein watchOS**. Die Erfassung von der Uhr entfiele dauerhaft.

### Entscheidung

| | Option A: umhängen | Option B: ablösen |
|---|---|---|
| Aufwand vor E3 | klein | keiner |
| Erfassung ab Umstiegstag | iPhone und Watch | nur mobiler Browser bis E4 |
| Watch-Erfassung langfristig | ja | nein |
| Pflege | zweiter Client (nur Push) | ein Client |

**Entscheidung (12.10.2026): Option A.** Funke schreibt direkt nach Zunder, ClickUp ist aus Funke entfernt ([rubenvitt/funke#1](https://github.com/rubenvitt/funke/pull/1)). Aufgaben gehen per Push in die Inbox, die Heute-Liste kommt über MCP `filter_tasks`. Auf den Umstiegstag gehört deshalb: Funke-Build mit Zunder-Server-URL und eigenem API-Token auf iPhone und Watch, gegen die Produktion getestet.
