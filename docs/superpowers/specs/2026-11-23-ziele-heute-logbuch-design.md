# Zunder-Kern: Ziele, Heute als Reihenfolge, Erledigt-Logbuch – Design

- **Datum:** 2026-11-23
- **Status:** Entwurf, wartet auf Rubens Freigabe (Fragen aus dem Builder-Check beantwortet)
- **Aufgabe:** ZUN-58 (Stufe 1 von 3; danach ZUN-59 Wochenplanung, ZUN-60 Reviews, ZUN-61 Langfrist; KI-Anbindung ZUN-62)
- **Bezug:** [Zunder-Design](2026-10-11-zunder-design.md), [Implementierungsvertrag](../plans/2026-10-11-zunder-implementation.md)

## 1. Warum

Zunder wird das zentrale Planungswerkzeug (Entscheidung 23.11.2026). Das Vault Cockpit bleibt eine App zum Lesen.

Heute besteht Rubens Kalender nur aus Zeitblöcken, und der Rückblick zeigt fast nur, was nicht geklappt hat. Wo die Ziele stehen, ist nirgends zu sehen, und der investierte Input auch nicht. Zeitblock-Planer (SkedPal, Motion, Claude-Zeitblöcke) haben jedes Mal überfordert. Stufe 1 dreht das um:

- **Heute** ist eine Reihenfolge ohne Uhrzeiten mit einer Aufgabe im Fokus, statt eines Kalenders voller Blöcke.
- **Ziele** sind in Zunder sichtbar, und zwar mit dem **Input**, der in der Woche hineinfloss, unabhängig vom Ergebnis.
- **Erledigt** ist ein Logbuch dessen, was geschafft und woran gearbeitet wurde.

Hintergrund im Vault: `wiki/sources/steenfatt-der-beste-task-manager.md`, `wiki/synthesis/tages-queue-vs-zeitbloecke-in-der-tages-planung.md`, `wiki/synthesis/execution-score-vs-input-ziele.md`.

## 2. Ziele, Nicht-Ziele, Gegen-Ziele

**Ziele**

1. Ziele als eigener, synchronisierter Datentyp mit MCP-Werkzeugen. Claude pflegt die Feuersturm-Ziele aus dem Vault und ordnet Aufgaben ihr Ziel zu. **Ruben pflegt nichts.**
2. Heute als geordnete Tagesreihenfolge mit genau einer Aufgabe im Fokus. Liegengebliebenes kommt still in die Reihenfolge, der rote Block „Überfällig“ entfällt.
3. Logbuch „Erledigt“: was geschafft wurde und woran gearbeitet wurde, mit Flammen-Minuten.
4. Ziele-Ansicht: Input pro Ziel in der laufenden Woche.
5. MCP so, dass `/tages-planung` die Reihenfolge mit einem Aufruf nach Zunder schreibt.

**Nicht-Ziele (Stufe 1)**

- Wochenplanung, Feuer-Planung, Reisewochen (ZUN-59).
- Review-Funktion in Zunder (ZUN-60). Stufe 1 liefert nur die Daten, die ein Review lesen kann.
- KI-Vorschläge in Zunder für Ziel-Zuordnung oder Reihenfolge (ZUN-62).
- Zeiterfassung jenseits der Flammen. Timing spiegelt nur die Flammen (`timing_ref`) und liefert keine eigenen Zahlen. Die Wahrheit sind die Flammen-Minuten in Zunder.
- Die Anpassung des Skills `/tages-planung` selbst. Er liegt im Vault (`~/rnotes/_System/claude-skills/tages-planung/`) und wird nach dem Zunder-Teil angepasst. Stufe 1 liefert die MCP-Schnittstelle dafür.

**Gegen-Ziele** (Stufe 1 ist gescheitert, wenn sie technisch läuft, aber eines davon zeigt):

- Neue rote Versagens-Anzeigen: Überfällig, verfehlte Quoten, leere Balken, Streaks.
- Pflegeaufwand für Ruben (Ziele anlegen, Aufgaben zuordnen).
- Uhrzeiten in der Heute-Reihenfolge. Nur Flammen bleiben Kalenderblöcke, und die entstehen außerhalb von Zunder.

## 3. Bauweg

| Weg | Inhalt | Abwägung |
|---|---|---|
| **1. Alles im synchronisierten Modell (gewählt)** | Neue Entität `goal`; neue Aufgabenfelder `goal_id`, `planned_on`, `day_rank`, `flame_log`. Regeln in `zunder-core` und gespiegelt in TS, mit gemeinsamen Testvektoren (Muster `week_balance`). | Größerer Umbau (Migration wie 0006, App zieht im Gleichschritt nach), dafür alles offline, ein einziger Mechanismus, gleiche Regeln überall. |
| 2. Einstellungen und Server | Ziele und Tagesplan als Einstellungszeilen (wie „Erst für dich“), Flammen-Minuten vom Server aus dem Op-Log. | Kaum Modelländerung, aber Ziele und Logbuch-Zeiten nur online, eigene Endpunkte und App-Befehle, zwei Speicherarten. |

Ruben hat Weg 1 gewählt (23.11.2026), weil die Ziele über das Quartal hinaus bleiben und Zunder als zentrales Werkzeug offline auf dem Handy funktionieren soll.

**Befunde, die den Weg prägen**

- `rank` ist schon die Reihenfolge in Projektlisten, Boards und Unteraufgaben (`web/src/sync/derived.ts`, `web/src/state/actions.ts`). Heute über `rank` zu ordnen würde beim Umsortieren die Projektreihenfolge mitverschieben. Heute bekommt deshalb **eigene Felder**. Das weicht bewusst vom Wortlaut der Aufgabe („Feld `rank`“) ab.
- Eine Aufgabe hält nur ihre **letzte** Flamme (`flame_started_at`/`flame_ended_at`). Frühere Flammen stehen nur im Op-Log des Servers. Für Minuten pro Woche braucht es eine Flammen-Historie an der Aufgabe.

## 4. Datenmodell

### 4.1 Entität `goal` (neu)

Synchronisiert wie `area`, `project` und `comment` (Feld-Operationen, LWW, Tombstone `deleted`). Die Migration baut `ops` und `entities` mit der erweiterten CHECK-Liste neu auf, wie `0006_comments.sql`.

| Feld | Typ | Bemerkung |
|---|---|---|
| `name` | Titel, Pflicht | „Sichtbar werden“ |
| `emoji` | Text, optional | „🎤“; zeigt das Ziel kompakt in Zeilen |
| `lead` | Text, optional | Leitkennzahl in Worten, z. B. „1 Sichtbarkeits-Zug pro Woche“ oder „3×, 2× in Reisewochen; 15 min zählen voll“ |
| `lead_per_week` | Zahl ≥ 1, optional | Zahl für das stille Häkchen (6.3). Ausnahmen wie Reisewochen stehen nur in `lead`. |
| `storm` | Text, optional | Bezeichnung des Feuersturms, z. B. „Feuersturm 2026-Q4“ |
| `starts_on`, `ends_on` | Datum `JJJJ-MM-TT`, optional | Zeitraum des Feuersturms (28.09.–20.12.2026) |
| `state` | `active` \| `closed` | Claude schließt die Ziele am Ende des Feuersturms |
| `rank` | Text, optional | Reihenfolge der Ziele in der Ansicht (gleiche Rangschlüssel wie bei Aufgaben) |
| `deleted` | Tombstone | |

Die Feuersturm-Ziele stammen aus `~/rnotes/20-lebensbereiche/Phönix-Progression/feuerstürme/Feuersturm 2026-Q4.md` (Abschnitte „Ziele“ und „Wochen-Tracker“). Das Zunder-Projekt „Ziele“ (Bereich Phönix) enthält nur Kandidaten für künftige Ziele und hat mit dieser Entität nichts zu tun.

### 4.2 Neue Felder an der Aufgabe

| Feld | Typ | Wer schreibt | Bemerkung |
|---|---|---|---|
| `goal_id` | ID eines Ziels, optional | Claude (MCP), Ruben bei Bedarf im Detail | Verweis auf ein gelöschtes Ziel zählt wie keins. Unteraufgaben ohne eigenes `goal_id` zählen für das Ziel ihrer Wurzelaufgabe (effektives Ziel, analog zur effektiven Stelle). |
| `planned_on` | Datum `JJJJ-MM-TT`, optional | `plan_day`, Heute-Oberfläche | Tag, für den die Aufgabe eingeplant ist. |
| `day_rank` | Rangschlüssel, optional | `plan_day`, Ziehen in Heute | Position innerhalb des Tages, unabhängig von `rank`. |
| `flame_log` | Liste `[{"start": RFC 3339, "end": RFC 3339}]` | nur der Server | Abgeschlossene Flammen der Aufgabe. Der Flammen-Dienst hängt beim Beenden einen Eintrag an. |

`flame_log` wird einmalig aus dem Op-Log nachgefüllt: aus allen bisherigen `flame_started_at`/`flame_ended_at`-Paaren je Aufgabe. Der Befund 28.09.–22.11. ergab 8 Flammen mit rund 10 h, das ist der Prüfwert für die Nachfüllung.

## 5. Heute

### 5.1 Welche Aufgaben, in welcher Reihenfolge

Eine gemeinsame Regel in `zunder-core` und `web/src/sync/` mit Testvektoren. Betrachtet werden nur lebende Aufgaben mit Status `open`, `in_progress`, `blocked` oder `review`. „Heute“ ist der Kalendertag des Clients, wie bisher.

1. **Eingeplant für heute:** `planned_on` = heute, nach `day_rank`.
2. **Liegengeblieben:** `planned_on` < heute, nach `planned_on`, dann `day_rank`. Ohne Markierung und ohne Farbe.
3. **Fällig, aber nicht eingeplant:** `due` ≤ heute und `planned_on` leer, sortiert wie bisher (Gewicht, dann Fälligkeit). Auch hier ohne Rot und ohne Überschrift „Überfällig“.

Eine Aufgabe steht höchstens einmal in der Liste. Die Gruppen 1 bis 3 bilden **eine** Liste ohne Zwischenüberschriften.

**Fokus** ist die laufende Flamme, falls eine läuft. Sonst ist es die erste Aufgabe der Liste mit Status `open` oder `in_progress` und ohne `waiting_on`, mit den Ausnahmen aus 5.2 (Tag ohne Plan, Plan erledigt). Alle weiteren Aufgaben stehen unter „Danach“.

```
Heute · Montag, 23. November
┌ Fokus ────────────────────────────────┐
│ ○ Krafttraining 15 min      🏃 Sport  │
│   [Flamme starten]                    │
└───────────────────────────────────────┘
Danach
  ○ Gliederung Blogartikel    🎤
  ○ Funkgeräte-Liste prüfen   🚒
  ○ Steuerunterlagen sortieren
Erinnerungen · Wiedervorlage   (wie bisher)
Tagesvorschau · Wochenspiegel  (wie bisher)
```

Erinnerungen, Wiedervorlage, Tagesvorschau, Wochenspiegel und die Zusagen-Bremse bleiben unverändert unter bzw. über der Reihenfolge. Eine Aufgabe, die schon in der Reihenfolge steht, erscheint nicht zusätzlich bei den Erinnerungen. Jede Zeile zeigt das Zeichen ihres effektiven Ziels.

### 5.2 „Erst für dich“ geht im Plan auf (Entscheidung Ruben)

- **Ein Plan für heute** besteht, sobald eine lebende Aufgabe `planned_on` = heute trägt, egal mit welchem Status, also auch erledigte (Entscheidung Ruben: „Plan bleibt Plan“). Ist alles Geplante erledigt, sagt der Fokus-Kasten ruhig „Plan für heute erledigt.“ und schlägt nichts Neues vor. Weitere Aufgaben der Liste (Liegengebliebenes, Fälliges) stehen dann ohne Fokus darunter.
- Gibt es einen Plan für heute, entfällt der Bereich „Erst für dich“. Der Grundsatz „erst mich selbst bezahlen“ wird zur Regel von `/tages-planung`: eine Glut-Aufgabe für mich bevorzugt nach vorn.
- Gibt es keinen Plan für heute, gilt im Fokus-Kasten die bisherige Regel von „Erst für dich“ (Vertrag 5.1, `web/src/sync/focus.ts`) **vor** der ersten Aufgabe der Liste. **Eigene Entscheidung, damit es sich wie heute anfühlt:**
  - Gibt es eine passende Glut-Aufgabe für mich, steht sie ohne Klick im Fokus. Liegengebliebene und fällige Aufgaben folgen unter „Danach“.
  - Gibt es keine, zeigt der Kasten wie bisher leise „Heute noch nichts für dich geplant.“ mit bis zu drei Glut-Aufgaben zum Wählen. Den Fokus übernimmt dann die erste Aufgabe der Liste, darunter.
  - Wählen plant die Aufgabe ein (`planned_on` = heute, vorn), statt die Einstellung `focus.chosen_*` zu schreiben.
  - „Für heute ausblenden“ blendet nur diesen Vorschlag aus. Der Fokus fällt dann auf die erste Aufgabe der Liste.
  - Der Vorschlag gehört zur Oberfläche, nicht zur gemeinsamen Regel in 5.1. Er bleibt in TS, `plan_day` liefert ihn nicht mit, und er wird nicht nach Rust portiert.

### 5.3 Bedienung

- Ziehen in Heute setzt `day_rank` und, bei bisher nicht eingeplanten Aufgaben, `planned_on` = heute.
- Kontextmenü und Detail: „Für heute einplanen“ (hinten anhängen) und „Aus Heute nehmen“ (`planned_on` = null; fällige Aufgaben bleiben in Gruppe 3).
- „Heute nicht“ wirkt wie bisher auf `due` und den Aufschubzähler. **Eigene Entscheidung:** Ist die Aufgabe eingeplant, wandert `planned_on` mit auf den Zieltag.
- Erledigen oder Verwerfen lässt `planned_on` stehen. Die Regel blendet geschlossene Aufgaben ohnehin aus, und das Logbuch braucht den Wert nicht.
- Die Zahl an „Heute“ in der Seitenleiste nennt die offenen Aufgaben der Reihenfolge plus Erinnerungen und Wiedervorlage, ohne Doppelte. Die laufende Flamme zählt wie bisher nicht mit.

## 6. Ziele-Ansicht

### 6.1 Input pro Ziel und Woche

Gemeinsame Regel in Core und TS mit Testvektoren (wie `week_balance`). Die Woche ist die ISO-Woche (Montag bis Sonntag) in der Ortszeit des Aufrufers.

- **Züge:** lebende Aufgaben mit Status `done`, deren `status_changed_at` in die Woche fällt und deren effektives Ziel dieses Ziel ist. Unteraufgaben zählen einzeln. Verworfene zählen nicht.
- **Minuten:** Summe der Flammen aus `flame_log` (plus der laufenden Flamme bis jetzt) auf Aufgaben mit diesem effektiven Ziel, zugeordnet nach dem Tag des **Starts**. Der Status der Aufgabe spielt keine Rolle (Input unabhängig vom Output).
- Dev-Projekte werden hier **nicht** ausgeschlossen: Wer einem Ziel eine Dev-Aufgabe zuordnet, meint sie.

### 6.2 Darstellung

Eigener Eintrag „Ziele“ in der Seitenleiste (Route `#/ziele`). Gezeigt werden die aktiven Ziele nach `rank`, darunter der Feuersturm mit der Restlaufzeit in Wochen.

```
Ziele · Woche 48
🎤 Sichtbar werden        1 Zug   · 40 min   (1×) ✓
🏃 Sport                  2 Züge  · 30 min   (3×)
🚒 I&K-Gruppe             0 Züge             (1×)
   Feuersturm 2026-Q4 · noch 4 Wochen
```

### 6.3 Leitkennzahl ruhig daneben (Entscheidung Ruben)

Die Zeile zeigt `lead_per_week` in Klammern, ohne Farbe und ohne Balken. Ist die Zahl der Züge erreicht, erscheint ein stilles Häkchen. Ist sie nicht erreicht, passiert nichts, auch nicht am Ende der Woche. Ein Klick auf ein Ziel zeigt `lead` als Text und die Aufgaben der Woche mit diesem Ziel (erledigt und mit Flammen).

## 7. Logbuch „Erledigt“

Eigener Eintrag „Erledigt“ in der Seitenleiste (Route `#/erledigt`).

- Tage untereinander, der neueste oben. Anfangs die letzten 14 Tage, darunter „Ältere zeigen“ für jeweils 14 weitere.
- **Erledigt:** lebende Aufgaben mit Status `done`, deren `status_changed_at` auf den Tag fällt. Ohne verworfene und ohne Aufgaben aus Dev-Projekten (wie im Wochenspiegel, denn Agenten schließen dort viele Aufgaben). Die Zeile zeigt Titel, Ziel-Zeichen und die Flammen-Minuten dieser Aufgabe **an diesem Tag**.
- **Daran gearbeitet** (Entscheidung Ruben): Aufgaben, die an diesem Tag Flammen hatten, aber an diesem Tag nicht erledigt wurden. Sie stehen ruhig unter dem Erledigten, mit eigenem Zeichen und Minuten, ebenfalls ohne Dev-Projekte.
- Die Tagessumme im Kopf ist die Summe aller Flammen-Minuten des Tages (beider Gruppen).
- Tage ohne Einträge entfallen. Es gibt keine Leerzustände pro Tag und keine Hinweise wie „nichts geschafft“.

```
Erledigt
Montag, 23. November                          80 min
  ✓ Krafttraining 15 min        🏃            15 min
  ✓ Gliederung Blogartikel      🎤            40 min
  ✓ Steuerunterlagen sortieren
  Daran gearbeitet
  ◐ Funkgeräte-Liste prüfen     🚒            25 min
Sonntag, 22. November                         …
```

## 8. MCP

Neue Werkzeuge (deutsche Beschreibungen und Fehlermeldungen wie die bestehenden):

| Werkzeug | Zweck |
|---|---|
| `list_goals` | Ziele mit allen Feldern. Standard nur aktive, `include_closed` für alle. |
| `create_goal`, `update_goal` | Ziel anlegen bzw. ändern (Felder wie 4.1). Schließen über `state: closed`. Kein Löschwerkzeug. |
| `plan_day` | `date` (Standard heute) und geordnete `task_ids` (IDs oder Kennungen). Setzt `planned_on` = `date` und `day_rank` in dieser Reihenfolge. Der Aufruf bestimmt den ganzen Tag (Entscheidung Ruben): Nicht geschlossene Aufgaben mit `planned_on` ≤ `date`, die nicht in der Liste stehen, bekommen `planned_on` = null. Liegengebliebenes, das Claude bewusst nicht aufnimmt, fällt also aus Heute heraus und bleibt an seiner Stelle (fällige Aufgaben stehen weiter in Gruppe 3). Erledigte Aufgaben behalten ihr `planned_on`. Ohne Planung an einem Tag rutscht Liegengebliebenes weiter still mit. Ein Aufruf, eine Push-Gruppe. Liefert die resultierende Heute-Liste (5.1) zurück. |
| `goal_input` | Input pro Ziel für eine ISO-Woche (6.1), Standard die laufende. |
| `done_log` | Logbuch (Kapitel 7) für einen Zeitraum `from`/`to`, für Reviews durch Claude (ZUN-60). |

Bestehende Werkzeuge:

- `create_task`, `update_task`: neu `goal_id` (null entfernt), `planned_on`, `day_rank`. `flame_log` ist nicht schreibbar.
- `get_task`, `filter_tasks`: zeigen `goal_id`, das effektive Ziel mit Name und Zeichen, `planned_on` und `flame_log`. `filter_tasks` filtert nach `goal_id` (inklusive „ohne Ziel“, damit Claude Lücken findet) und nach `planned_on`.
- `list_structure` bleibt unverändert, Ziele liefert `list_goals`.
- Die Server-Anweisungen des MCP erklären Ziele, die Tagesreihenfolge und „Claude pflegt, Ruben nicht“.

## 9. Randbedingungen

- **Qualitätsschranken** aus `docs/entwicklung.md`: fmt, clippy, Tests, TS-Typecheck und -Tests, Playwright gegen Mock und echten Server. Der Mock-Server bildet die neuen Regeln nach.
- **Vertrag nachziehen:** Datenmodell (Abschnitt 2), Sichten (5.1) und MCP im Implementierungsvertrag ergänzen.
- **App (Tauri):** Die Web-Oberfläche ist eingebettet und zieht automatisch mit. `zunder-local` nutzt `zunder-core` und muss mit der neuen Entität und den neuen Feldern im selben Release gebaut werden. Der Builder prüft, wie ältere App-Stände reagieren, wenn sie Operationen einer unbekannten Entität oder eines unbekannten Felds ziehen. Bricht der Sync dabei dauerhaft, braucht es einen verträglichen Weg (zum Beispiel Überspringen unbekannter Operationen). Die Entscheidung kommt in den PR.
- **Konventionen:** Conventional Commits auf Deutsch (`CLAUDE.md`). Die Oberfläche ist auf Deutsch und verwendet die Begriffe dieses Entwurfs.
- **Tempo-Budget** < 100 ms je Interaktion bleibt. Ziele-Input und Logbuch werden aus dem lokalen Zustand berechnet.

## 10. Was dem Builder überlassen bleibt

- Schnitt in Commits und PRs. Ein Vorschlag wäre: (1) Modell und Migration, (2) Flammen-Historie mit Nachfüllung, (3) Heute, (4) Ziele-Ansicht, (5) Logbuch, (6) MCP. Jeder Schritt sollte einzeln lieferbar sein.
- Genaue Gestaltung (Abstände, Zeichen für „Daran gearbeitet“, Ziehen auf Mobilgeräten), im Stil der bestehenden Sichten.
- Icons der neuen Seitenleisten-Einträge (Icons8 Fluency Systems Regular wie bisher).
- Wie die Nachfüllung von `flame_log` läuft (Migration beim Start oder einmaliger Server-Job), solange sie idempotent ist.
- Tastenkürzel für die neuen Sichten im Schema `g …`.

## 11. Danach

- Ruben benutzt Stufe 1 eine Weile. Erst dann folgen ZUN-60 (Reviews) und ZUN-59 (Wochenplanung).
- `/tages-planung` im Vault auf `plan_day` umstellen. Im Kalender bleiben nur die Flammen als Blöcke.
- KI-Vorschläge für Ziel-Zuordnung oder Reihenfolge: ZUN-62.
