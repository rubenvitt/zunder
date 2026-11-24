# Timing-Stunden pro Ziel – Design

- **Datum:** 2026-11-24
- **Status:** Entwurf, wartet auf Freigabe
- **Aufgabe:** ZUN-66 (Stufe 1b; nach ZUN-58, vor ZUN-59)
- **Bezug:** [Ziele, Heute, Logbuch](2026-11-23-ziele-heute-logbuch-design.md) (Kap. 4.1, 6, 8), [Zunder-Design](2026-10-11-zunder-design.md)

## 1. Warum

Die Ziele-Ansicht und `goal_input` zeigen den Input einer Woche als Züge (erledigte Aufgaben) und Flammen-Minuten. Vom 28.09. bis 22.11.2026 gab es aber nur rund 10 h Flammen, während der Output deutlich größer war (3 Blogposts, 4 CfPs, Website, Vortrag, 3 DAs). In Timing standen im selben Zeitraum 40,9 h Zeiteinträge auf DRK und 9 h auf Lifeline Hub. `goal_input` zeigte für „Sichtbar werden“ in W47 eine 0, obwohl Website und Vortrag in diese Woche fielen.

Flammen allein unterschätzen den Input. Der Entwurf zu ZUN-58 hatte „Zeiterfassung jenseits der Flammen“ als Nicht-Ziel; Ruben wollte sie aber haben. Dieser Entwurf holt das nach: **Zu jedem Ziel erscheinen die Timing-Stunden der Woche**, sachlich und ohne Soll.

## 2. Ziele, Nicht-Ziele, Gegen-Ziele

**Ziele**

1. Ein Ziel nennt die Timing-Projekte, deren Zeit auf es einzahlt. **Claude pflegt diese Zuordnung über MCP, Ruben pflegt nichts.**
2. Ziele-Ansicht und `goal_input` zeigen je Ziel die Timing-Zeit der Woche neben Zügen und Flammen-Minuten.
3. Die Timing-Zeit ist ehrlich: nur gebuchte Zeiteinträge, keine Doppelzählung mit Flammen.

**Nicht-Ziele**

- Automatisch erfasste App-Nutzung (Bildschirmzeit) zählt **nicht** (Entscheidung Ruben, 24.11.2026). Es zählen nur Zeiteinträge. Fehlt Zeit, bucht Claude sie in Timing nach (Skill `timing`), statt dass Zunder rät.
- Timing-Zeit im Logbuch „Erledigt“ (pro Tag). Eigene Folgeaufgabe.
- Timing-Stunden offline. Ohne Verbindung zum Server oder ohne Timing-Token fehlen sie einfach (Ruben: „die Stunden kommen dann eben später“).
- Timing-Projekte für Ziele, die noch keins haben (Sichtbarkeit, Sport), sind Arbeit in Timing, nicht in Zunder. Claude legt sie an, sobald die Zuordnung steht. Zeit in der INNOQ-Arbeitszeit bleibt bei INNOQ und wird keinem Ziel zugeordnet.
- Soll-Werte, Quoten oder Balken für Stunden.

**Gegen-Ziele** (gescheitert, wenn es technisch läuft, aber eines davon zeigt):

- Eine neue sichtbare „0“ oder ein Leerzustand für Stunden. Ohne Zuordnung, ohne Zeit oder ohne Verbindung steht schlicht nichts da.
- Fehlermeldungen oder Warnfarben in der Ziele-Ansicht, wenn Timing nicht erreichbar ist.
- Pflegeaufwand für Ruben.
- Die Ziele-Ansicht wird langsamer, weil sie auf Timing wartet: Züge und Minuten erscheinen sofort, die Stunden ergänzen sich.

## 3. Bauweg

| Weg | Inhalt | Abwägung |
|---|---|---|
| **A. Zuordnung am Ziel, Stunden live vom Server (gewählt)** | Neues synchronisiertes Feld `timing_projects` am Ziel. Der Server fragt Timings Bericht ab und rechnet je Ziel zusammen, mit kurzem Zwischenspeicher. Die Oberfläche holt die Zahlen wie den PR-Status. | Zuordnung liegt beim Ziel, ist offline sichtbar und wird mit `update_goal` gepflegt. Kostet ein neues Feld, also müssen ältere App-Stände aktualisiert werden (wie bei ZUN-64). |
| B. Zuordnung in den Server-Einstellungen | Mapping als Einstellungszeile, Modell unverändert. | Ältere Apps unberührt, aber Zieldaten an zwei Orten, Reste nach gelöschten Zielen, `update_goal` schreibt in einen zweiten Speicher. |
| C. Stunden ins synchronisierte Modell schreiben | Server trägt die Wochenstunden regelmäßig ins Ziel ein. | Offline der letzte Stand, aber laufend Server-Operationen im Op-Log, und ein veralteter Wert sieht aus wie ein aktueller. |

Ruben hat Weg A gewählt (24.11.2026).

## 4. Datenmodell

Neues Feld am Ziel (Entität `goal`, Feld-Operation mit LWW wie die übrigen Felder):

| Feld | Typ | Wer schreibt | Bemerkung |
|---|---|---|---|
| `timing_projects` | Liste von Self-Links `"/projects/<Zahl>"`, optional, leer wie fehlend | Claude (MCP) | Ohne Unterprojekte: `/projects/A` zählt nur Einträge, die direkt in A liegen. Soll ein Unterprojekt mitzählen, steht es selbst in der Liste. Beispiel I&K: `["/projects/3708302322996078848"]` (DRK ohne Lifeline Hub). |

- Validierung in `zunder-core` und gespiegelt in `web/src/sync/validate.ts`: Liste ohne Duplikate, jeder Eintrag `/projects/` gefolgt von Ziffern, höchstens 50 Einträge. MCP normalisiert vorher absolute URLs (`https://web.timingapp.com/api/v1/projects/123`) und reine Zahlen auf diese Form.
- Gleiches Feld in `Goal` (Rust) und im TS-Typ; Testvektoren in `zunder-core/tests/vectors.rs` und `web/test/vectors.test.ts` erweitern.
- Keine Migration nötig: `ops.field` ist freier Text, nur die Entitätsarten stehen in einem CHECK (`0010_goals.sql`).
- **Ältere App-Stände:** Ein unbekanntes Feld lehnt die alte Validierung ab. Wie in ZUN-64: iOS-App neu bauen, **bevor** Claude die erste Zuordnung schreibt. Das steht im PR und in `docs/betrieb.md`.

## 5. Timing-Zeit pro Ziel und Woche

### 5.1 Regel

- **Woche:** ISO-Woche (Montag 00:00 bis Sonntag 24:00) in der Zeitzone des Servers (`ServerZone`, wie `goal_input`). An Timing gehen die Grenzen als RFC-3339-Zeitpunkte mit Offset, nicht als reine Daten, damit Timings Zeitzone keine Rolle spielt.
- **Was zählt:** Zeiteinträge (`include_app_usage` nicht gesetzt bzw. `false`) in genau den Projekten aus `timing_projects` (`include_child_projects` = false), so wie Timings Bericht sie für das Wochenfenster liefert. **Eigene Entscheidung:** Wie Timing Einträge an den Wochengrenzen behandelt (nach Start oder anteilig) und ob ein laufender Timer mitzählt, übernimmt Zunder unverändert. Kleine Abweichungen an den Rändern sind in Ordnung; dafür bleibt es bei `/report`, das keine Titel und Notizen liefert (9.).
- **Je Ziel:** Summe der Dauern aller Projekte des Ziels, in ganzen Minuten (abgerundet nach dem Summieren).
- **Projekt bei mehreren Zielen:** Ein Projekt zählt für jedes aktive Ziel, das es nennt. MCP verhindert das im Normalfall (8.), die Rechnung bleibt trotzdem eindeutig.
- **Flammen:** Das Flammen-Projekt aus den Timing-Einstellungen (`timing.project`) darf keinem Ziel zugeordnet werden (MCP lehnt ab, 8.). **Eigene Entscheidung:** Zusätzlich lässt die Rechnung das jeweils gültige Flammen-Projekt weg, falls es doch in einer Zuordnung steht (etwa weil die Einstellung später geändert wurde). Ist `timing.project` ein Titel statt eines Self-Links, löst der Server ihn über die Projektliste auf; gelingt das nicht, rechnet er ohne diesen Ausschluss. Flammen zählen weiter nur über `flame_log`.
- **Nur aktive Ziele**, wie in `goal_input`.

### 5.2 Abfrage bei Timing

- Neue Operation der Timing-Brücke (`crates/zunder-server/src/timing.rs`, Trait `TimingApi`): `report(from, to, projects) -> Vec<(project_self, seconds)>` über `GET /report` mit `columns[]=project`, `projects[]=…`, `include_child_projects=false`, ohne Gruppierung nach Zeitspanne. Genaue Parameter und Antwortform prüft der Builder an <https://web.timingapp.com/docs/> und mit einem Mock-Test wie bei den übrigen Operationen.
- **Ein Aufruf je Woche** mit der Vereinigung aller Projekte aller aktiven Ziele. Der Server verteilt das Ergebnis auf die Ziele.
- **Zwischenspeicher** im Speicher des Servers, Schlüssel = (Woche, sortierte Projektliste): laufende Woche 5 min, vergangene Wochen 1 h. Fehler werden nicht gespeichert. Ändert Claude eine Zuordnung, ergibt sich ein neuer Schlüssel.
- Kein Token → kein Aufruf, Antwort „nicht eingerichtet“. Timing nicht erreichbar oder Fehlerstatus → Antwort „nicht verfügbar“ mit der deutschen Meldung aus `TimingError`.

## 6. Server-Schnittstelle

`GET /api/goal-hours?week=JJJJ-Www` (Standard: laufende Woche), gleiche Anmeldung wie die übrigen `/api`-Routen:

```json
{
  "week": "2026-W48",
  "status": "ok",
  "goals": { "<goal_id>": 360 }
}
```

- `status`: `ok`, `not_configured` (kein Timing-Token) oder `unavailable` (Timing hat nicht geantwortet; dann zusätzlich `message`).
- `goals` enthält nur Ziele mit nicht leerer Zuordnung, Werte in Minuten. Bei `not_configured`/`unavailable` ist `goals` leer.
- Der Server liest die Zuordnungen aus seinem synchronisierten Zustand.

## 7. Oberfläche

**Eigene Entscheidung:** Die Stunden erscheinen überall, wo die Ziele-Ansicht läuft, also auch in der App (macOS, iOS). Die App ruft `/api` nicht direkt auf; dafür bekommt sie einen Befehl `goal_hours` in `zunder-local` (`client.rs`) und `apps/zunder-app/src-tauri/src/commands.rs`, nach dem Muster von `pr_status`. Antwortet ein älterer Server ohne diese Route, zeigt die App still keine Stunden. Der Befehl kommt in denselben App-Stand, der für das neue Feld (4.) ohnehin gebaut wird.

### 7.1 Ziele-Ansicht

Die Zeile bekommt nach den Flammen-Minuten die Timing-Zeit:

```
Ziele · Woche 48
🎤 Sichtbar werden   1 Zug  · 40 min · 3,5 h Timing   (1×) ✓
🚒 I&K-Gruppe        0 Züge          · 6 h Timing     (1×)
🏃 Sport             2 Züge · 30 min                  (3×)
   Feuersturm 2026-Q4 · noch 4 Wochen
```

- Anzeige ab 60 min in Stunden mit höchstens einer Nachkommastelle und deutschem Komma („3,5 h Timing“, „6 h Timing“), darunter in Minuten („40 min Timing“). Das Wort „Timing“ unterscheidet sie von den Flammen-Minuten.
- **Nichts** anzeigen bei: keine Zuordnung, 0 min, `not_configured`, `unavailable`, offline, noch keine Antwort. Kein Fehlertext, kein Platzhalter.
- Das Häkchen (`lead_per_week`) bleibt an Zügen gekoppelt, Stunden ändern daran nichts.
- Die Zahlen kommen über einen kleinen Store nach dem Muster von `web/src/state/prs.ts`: Abruf beim Öffnen der Ansicht, erneut alle 5 min, solange sie offen ist, und wenn das Fenster wieder sichtbar wird. Die Zeilen erscheinen sofort aus dem lokalen Zustand, die Stunden ergänzen sich, ohne dass die Zeilen springen (eigener Platz am Zeilenende).
- Klick auf ein Ziel (Detail mit `lead` und Aufgaben der Woche): zusätzlich eine Zeile „Timing: 6 h“. Aufschlüsselung nach Projekten bleibt dem Builder überlassen.

### 7.2 Keine Pflege in der Oberfläche

Die Zuordnung ist in der Oberfläche nicht editierbar (Claude pflegt). Ob das Ziel-Detail die zugeordneten Projekte als Text zeigt, entscheidet der Builder; nötig ist es nicht.

## 8. MCP

- `create_goal`, `update_goal`: neuer Parameter `timing_projects` (Liste; `[]` oder `null` entfernt die Zuordnung). Absolute URLs und reine Zahlen werden auf `/projects/<Zahl>` normalisiert. Abgelehnt mit deutscher Begründung:
  - das Flammen-Projekt aus den Timing-Einstellungen (ein Titel dort wird über die Projektliste aufgelöst; ist Timing nicht erreichbar, wird gespeichert und gewarnt),
  - ein Projekt, das schon einem **anderen aktiven** Ziel zugeordnet ist (Meldung nennt das Ziel),
  - wenn Timing erreichbar ist: ein Projekt, das Timing nicht kennt (Abgleich mit `GET /projects`, archivierte eingeschlossen). Ist Timing nicht erreichbar, wird gespeichert und eine Warnung zurückgegeben.
- `list_goals`: zeigt `timing_projects`. Titel der Projekte nur, wenn die Projektliste schnell zu haben ist (Zwischenspeicher); sonst nur die Self-Links.
- `goal_input`: je Ziel zusätzlich `timing_minutes` (Zahl oder `null` bei fehlender Zuordnung bzw. Timing nicht verfügbar) und oben `timing_status` (`ok`, `not_configured`, `unavailable`, ggf. `timing_message`). `goal_input` wartet höchstens einige Sekunden auf Timing; dauert es länger, kommt `unavailable` und die übrigen Werte trotzdem.
- Server-Anweisungen des MCP: ein Satz zur Zuordnung („Ein Ziel nennt seine Timing-Projekte in timing_projects, ohne Unterprojekte; das Flammen-Projekt nie. Claude pflegt das.“).

## 9. Randbedingungen

- **Qualitätsschranken** aus `docs/entwicklung.md`: fmt, clippy, Tests, TS-Typecheck und -Tests, Playwright gegen Mock und echten Server. Der Mock-Server bekommt `GET /api/goal-hours` mit festen Werten.
- **Tests:** Timing-Client gegen einen Mock-Server (Parameter, Antwort, Fehler, 0 Einträge); Verteilung auf Ziele (mehrere Projekte je Ziel, Projekt ohne Zeit, Ziel ohne Zuordnung, geschlossenes Ziel); Zwischenspeicher; MCP-Ablehnungen; Oberfläche zeigt nichts bei `unavailable`.
- **Doku:** Entwurf zu ZUN-58 (Kap. 2, Nicht-Ziel „Zeiterfassung jenseits der Flammen“) bekommt einen Verweis auf diesen Entwurf. Implementierungsvertrag (Ziel-Felder, MCP) und `docs/betrieb.md` (Timing-Token wird jetzt auch für Berichte genutzt; Reihenfolge iOS-Update vor erster Zuordnung) nachziehen.
- **Konventionen:** Conventional Commits auf Deutsch, Bereich `server`, `web`, `core`, `local` oder `app`; nutzersichtbar ist `feat`.
- **Datenschutz:** Zunder speichert nur Self-Links und liefert Minuten. Titel und Notizen von Zeiteinträgen werden weder abgefragt noch gespeichert.

## 10. Was dem Builder überlassen bleibt

- Schnitt in Commits, z. B. (1) Feld im Modell, (2) Timing-Bericht in der Brücke, (3) Endpunkt mit Zwischenspeicher, (4) App-Befehl, (5) Ziele-Ansicht, (6) MCP.
- Genaue Parameter von `GET /report` (Doku nicht aus jeder Umgebung erreichbar; im Zweifel per Mock-Test und einem Probeaufruf mit echtem Token klären).
- Ob ein Token-Wechsel den Zwischenspeicher leert, und wie ein wieder geöffnetes Ziel mit schon vergebenem Projekt behandelt wird.
- Genaue Gestaltung und Abstände der Stundenangabe im Stil der Ziele-Ansicht.
- Ob die Projektliste für MCP-Titel und -Prüfung eigens zwischengespeichert wird.
- Zeitlimit für Timing-Aufrufe (einige Sekunden).

## 11. Danach

- iOS-App neu bauen, dann ordnet Claude zu: I&K → DRK. Für Sichtbarkeit legt Claude ein privates Timing-Projekt an und bucht W47 (Website, Vortrag) nach; Sport bekommt eins, wenn Ruben Sportzeit in Timing bucht.
- Folgeaufgabe: Timing-Zeit pro Tag im Logbuch „Erledigt“.
