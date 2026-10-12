//! Tool definitions (names, descriptions, input schemas, annotations).
//!
//! Names and schemas are contract (implementation contract section 4) and
//! frozen by the snapshot test `tests/snapshots/mcp_tools.json`.
//!
//! Schemas avoid top-level `oneOf`/`anyOf`, which several clients reject;
//! "`task_id` or `clickup_id`" is described in words and checked at runtime.

use serde_json::{json, Map, Value};
use zunder_core::{
    AreaState, PhoenixRole, ProjectKind, ProjectStatus, ProjectView, TaskStatus, TaskType,
};

/// One tool.
pub struct ToolDef {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    pub read_only: bool,
    pub destructive: bool,
    pub idempotent: bool,
}

impl ToolDef {
    /// The entry in the `tools/list` result.
    pub fn to_json(&self) -> Value {
        let mut annotations = json!({
            "title": self.title,
            "readOnlyHint": self.read_only,
            "openWorldHint": false,
        });
        if !self.read_only {
            annotations["destructiveHint"] = json!(self.destructive);
            annotations["idempotentHint"] = json!(self.idempotent);
        }
        json!({
            "name": self.name,
            "title": self.title,
            "description": self.description,
            "inputSchema": self.input_schema,
            "annotations": annotations,
        })
    }

    /// Property names allowed by the input schema.
    pub fn properties(&self) -> impl Iterator<Item = &str> {
        self.input_schema["properties"]
            .as_object()
            .into_iter()
            .flat_map(|p| p.keys().map(String::as_str))
    }

    /// Required property names.
    pub fn required(&self) -> impl Iterator<Item = &str> {
        self.input_schema["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
    }
}

// ----- schema fragments ------------------------------------------------------

fn string(description: &str) -> Value {
    json!({"type": "string", "description": description})
}

fn nullable_string(description: &str) -> Value {
    json!({"type": ["string", "null"], "description": description})
}

fn boolean(description: &str) -> Value {
    json!({"type": "boolean", "description": description})
}

fn integer(description: &str, min: u64, max: u64, default: u64) -> Value {
    json!({
        "type": "integer",
        "description": description,
        "minimum": min,
        "maximum": max,
        "default": default,
    })
}

fn enumeration(codes: Vec<&'static str>, description: &str) -> Value {
    json!({"type": "string", "enum": codes, "description": description})
}

fn enum_list(codes: Vec<&'static str>, description: &str) -> Value {
    json!({
        "type": "array",
        "items": {"type": "string", "enum": codes},
        "description": description,
    })
}

fn location() -> Value {
    json!({
        "type": "object",
        "description": "Stelle der Aufgabe: {\"kind\":\"inbox\"}, {\"kind\":\"loose\"}, \
            {\"kind\":\"area\",\"id\":<Bereichs-ID>} oder {\"kind\":\"project\",\"id\":<Projekt-ID>}. \
            IDs liefert list_structure.",
        "properties": {
            "kind": {"type": "string", "enum": ["inbox", "loose", "area", "project"]},
            "id": {"type": "string", "description": "ID des Bereichs bzw. Projekts (nur bei area/project)."},
        },
        "required": ["kind"],
        "additionalProperties": false,
    })
}

fn links() -> Value {
    json!({
        "type": "array",
        "description": "Externe Links; ersetzt die ganze Liste.",
        "items": {
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "Absolute URL (beliebiges Schema)."},
                "title": {"type": "string", "description": "Optionaler Anzeigetext."},
            },
            "required": ["url"],
            "additionalProperties": false,
        },
    })
}

fn vault_links() -> Value {
    json!({
        "type": "array",
        "description": "Verweise auf Notizen im Obsidian-Vault (Pfade oder obsidian://-URIs); ersetzt die ganze Liste.",
        "items": {"type": "string", "minLength": 1},
    })
}

fn phoenix() -> Value {
    json!({
        "type": ["object", "null"],
        "description": "Phönix-Markierung oder null zum Entfernen.",
        "properties": {
            "role": {"type": "string", "enum": PhoenixRole::codes()},
            "fire": {"type": "string", "description": "Feuer als ISO-Woche JJJJ-Www, z. B. 2026-W45."},
        },
        "required": ["role", "fire"],
        "additionalProperties": false,
    })
}

fn object(properties: Vec<(&str, Value)>, required: &[&str]) -> Value {
    let mut props = Map::new();
    for (name, schema) in properties {
        props.insert(name.to_owned(), schema);
    }
    json!({
        "type": "object",
        "properties": props,
        "required": required,
        "additionalProperties": false,
    })
}

const DUE: &str = "Fälligkeit: Datum JJJJ-MM-TT oder RFC-3339-Zeitpunkt mit Offset.";
const REMIND_AT: &str = "Erinnerung als RFC-3339-Zeitpunkt mit Offset.";
const STATUS: &str =
    "Status: open, in_progress, review (nur Dev-Projekte), blocked, done, dropped.";
const TYPE: &str = "Typ: aufgabe (Standard), bug oder feature (nur in Dev-Projekten).";

fn task_ref() -> Vec<(&'static str, Value)> {
    vec![
        (
            "task_id",
            string("ID der Aufgabe (UUID) oder ihre Kennung wie „ZUN-42“. Alternativ clickup_id angeben."),
        ),
        (
            "clickup_id",
            string("ClickUp-ID einer importierten Aufgabe (statt task_id)."),
        ),
    ]
}

/// Settable task fields shared by create_task and update_task.
fn task_fields(nullable: bool) -> Vec<(&'static str, Value)> {
    let text = |d: &str| {
        if nullable {
            nullable_string(&format!("{d} null entfernt den Wert."))
        } else {
            string(d)
        }
    };
    vec![
        ("description", text("Beschreibung (Markdown).")),
        ("type", enumeration(TaskType::codes(), TYPE)),
        ("status", enumeration(TaskStatus::codes(), STATUS)),
        ("due", text(DUE)),
        ("remind_at", text(REMIND_AT)),
        ("links", links()),
        ("vault_links", vault_links()),
        ("phoenix", phoenix()),
        ("plan", text("Plan (Markdown), z. B. aus dem Dev-Modus.")),
        ("branch", text("Git-Branch der Umsetzung.")),
        ("pr_url", text("URL des Pull Requests (http/https).")),
    ]
}

fn with(
    mut base: Vec<(&'static str, Value)>,
    more: Vec<(&'static str, Value)>,
) -> Vec<(&'static str, Value)> {
    base.extend(more);
    base
}

/// All tools in a stable order.
pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "filter_tasks",
            title: "Aufgaben filtern",
            description: "Listet Aufgaben nach Filtern. Ohne include_closed werden erledigte und \
                verworfene Aufgaben ausgeblendet, deren Status sich vor mehr als 3 Tagen geändert hat. \
                Gelöschte Aufgaben erscheinen nie. Alle Filter werden mit UND verknüpft. Sortierung \
                (order_by): due = Fälligkeit (ohne Fälligkeit zuletzt), dann Erstellung (Standard); \
                created, updated oder status_changed; reverse kehrt sie um. Mehr als limit Treffer \
                holt man seitenweise mit offset (next_offset im Ergebnis).",
            input_schema: object(
                vec![
                    (
                        "location",
                        enumeration(
                            vec!["inbox", "loose", "area", "project"],
                            "Art der wirksamen Stelle (bei Unteraufgaben die der Wurzelaufgabe).",
                        ),
                    ),
                    (
                        "area_id",
                        string("Nur Aufgaben in diesem Bereich, direkt oder in einem seiner Projekte."),
                    ),
                    ("project_id", string("Nur Aufgaben in diesem Projekt.")),
                    ("status", enum_list(TaskStatus::codes(), "Nur diese Status.")),
                    ("type", enum_list(TaskType::codes(), "Nur diese Typen.")),
                    (
                        "phoenix_role",
                        enumeration(PhoenixRole::codes(), "Nur Aufgaben mit dieser Phönix-Rolle."),
                    ),
                    (
                        "phoenix_fire",
                        string("Nur Aufgaben dieses Feuers (ISO-Woche JJJJ-Www)."),
                    ),
                    (
                        "due_before",
                        string("Fällig spätestens an diesem Tag (JJJJ-MM-TT oder RFC 3339, inklusive, Vergleich nach Kalendertag)."),
                    ),
                    (
                        "due_after",
                        string("Fällig frühestens an diesem Tag (JJJJ-MM-TT oder RFC 3339, inklusive, Vergleich nach Kalendertag)."),
                    ),
                    (
                        "include_closed",
                        boolean("Auch erledigte und verworfene Aufgaben, die älter als 3 Tage sind (Standard false)."),
                    ),
                    (
                        "closed_after",
                        string("Nur erledigte oder verworfene Aufgaben, deren Status sich an oder nach diesem Tag geändert hat (JJJJ-MM-TT, Kalendertag in der Zeitzone des Servers, inklusive; oder RFC-3339-Zeitpunkt). Schließt include_closed ein."),
                    ),
                    (
                        "closed_before",
                        string("Nur erledigte oder verworfene Aufgaben, deren Status sich an oder vor diesem Tag geändert hat (wie closed_after)."),
                    ),
                    (
                        "parent_id",
                        string("Nur direkte Unteraufgaben dieser Aufgabe (UUID, Kennung wie „ZUN-42“ oder ClickUp-ID einer importierten Aufgabe)."),
                    ),
                    (
                        "claimed_by",
                        string("Nur Aufgaben mit gültiger Reservierung durch diesen Agenten."),
                    ),
                    (
                        "flame_running",
                        boolean("true: nur Aufgaben mit laufender Flamme (unabhängig von Status und Alter); false: nur ohne."),
                    ),
                    (
                        "order_by",
                        enumeration(
                            vec!["due", "created", "updated", "status_changed"],
                            "Sortierung (Standard due).",
                        ),
                    ),
                    ("reverse", boolean("Sortierung umkehren (Standard false).")),
                    (
                        "offset",
                        integer("So viele Treffer der Sortierung überspringen.", 0, 1_000_000, 0),
                    ),
                    ("limit", integer("Höchstzahl der Ergebnisse.", 1, 500, 100)),
                ],
                &[],
            ),
            read_only: true,
            destructive: false,
            idempotent: true,
        },
        ToolDef {
            name: "get_task",
            title: "Aufgabe lesen",
            description: "Liefert eine Aufgabe mit allen Feldern, Fortschritt und dem Baum ihrer \
                Unteraufgaben (subtasks). Adressierung über task_id oder clickup_id.",
            input_schema: object(task_ref(), &[]),
            read_only: true,
            destructive: false,
            idempotent: true,
        },
        ToolDef {
            name: "create_task",
            title: "Aufgabe anlegen",
            description: "Legt eine Aufgabe an. Nur title ist Pflicht; ohne location landet sie in \
                der Inbox. Mit parent_id entsteht eine Unteraufgabe (ihre Stelle ist die der Wurzelaufgabe).",
            input_schema: object(
                with(
                    vec![
                        ("title", string("Titel (Pflicht, höchstens 500 Zeichen).")),
                        ("location", location()),
                        (
                            "parent_id",
                            string("Elternaufgabe für eine Unteraufgabe: UUID, Kennung wie „ZUN-42“ oder ClickUp-ID einer importierten Aufgabe."),
                        ),
                    ],
                    task_fields(false),
                ),
                &["title"],
            ),
            read_only: false,
            destructive: false,
            idempotent: false,
        },
        ToolDef {
            name: "update_task",
            title: "Aufgabe ändern",
            description: "Ändert Felder einer Aufgabe (task_id oder clickup_id). Nur angegebene \
                Felder ändern sich; null entfernt optionale Werte. Stelle und Elternaufgabe ändert \
                move_task, Reservierungen claim_task/release_task.",
            input_schema: object(
                with(
                    with(task_ref(), vec![("title", string("Neuer Titel."))]),
                    task_fields(true),
                ),
                &[],
            ),
            read_only: false,
            destructive: false,
            idempotent: true,
        },
        ToolDef {
            name: "move_task",
            title: "Aufgabe verschieben",
            description: "Verschiebt eine Aufgabe an eine neue Stelle und/oder unter eine andere \
                Elternaufgabe. Nur location: die Aufgabe wird an diese Stelle gelegt und von einer \
                Elternaufgabe gelöst. parent_id: wird zur Unteraufgabe. parent_id null: wird \
                eigenständig und behält ihre bisherige wirksame Stelle, sofern keine location angegeben ist.",
            input_schema: object(
                with(
                    task_ref(),
                    vec![
                        ("location", location()),
                        (
                            "parent_id",
                            nullable_string("Neue Elternaufgabe (UUID, Kennung wie „ZUN-42“ oder ClickUp-ID einer importierten Aufgabe) oder null, um die Aufgabe zu lösen."),
                        ),
                    ],
                ),
                &[],
            ),
            read_only: false,
            destructive: false,
            idempotent: true,
        },
        ToolDef {
            name: "delete_task",
            title: "Aufgabe löschen",
            description: "Löscht eine Aufgabe samt allen Unteraufgaben (Tombstone; bleibt im \
                Änderungsprotokoll nachvollziehbar).",
            input_schema: object(task_ref(), &[]),
            read_only: false,
            destructive: true,
            idempotent: true,
        },
        ToolDef {
            name: "search",
            title: "Suchen",
            description: "Volltextsuche über Titel, Beschreibung und Plan von Aufgaben sowie Namen \
                von Projekten und Bereichen. Findet auch erledigte und verworfene Aufgaben, aber keine \
                gelöschten. Groß-/Kleinschreibung egal, „ä“ findet „a“ und „ae“. Eine exakte ClickUp-ID \
                findet die importierte Aufgabe.",
            input_schema: object(
                vec![
                    ("query", string("Suchbegriffe; alle Wörter müssen vorkommen.")),
                    ("limit", integer("Höchstzahl der Treffer.", 1, 200, 20)),
                ],
                &["query"],
            ),
            read_only: true,
            destructive: false,
            idempotent: true,
        },
        ToolDef {
            name: "claim_task",
            title: "Aufgabe reservieren",
            description: "Reserviert eine Aufgabe für einen Agenten mit Ablaufzeit (Dev-Modus). \
                Schlägt fehl, wenn ein anderer Agent sie gültig reserviert hat. Derselbe Agent kann \
                seine Reservierung damit verlängern.",
            input_schema: object(
                with(
                    task_ref(),
                    vec![
                        ("agent", string("Name des Agenten, z. B. „claude-code@laptop“.")),
                        ("ttl_minutes", integer("Gültigkeit in Minuten.", 1, 10080, 120)),
                    ],
                ),
                &["agent"],
            ),
            read_only: false,
            destructive: false,
            idempotent: false,
        },
        ToolDef {
            name: "release_task",
            title: "Reservierung aufheben",
            description: "Hebt die Reservierung einer Aufgabe auf. Nur der reservierende Agent darf \
                das, außer die Reservierung ist abgelaufen oder force ist true.",
            input_schema: object(
                with(
                    task_ref(),
                    vec![
                        ("agent", string("Name des reservierenden Agenten (Pflicht ohne force).")),
                        ("force", boolean("Fremde, gültige Reservierung trotzdem aufheben (Standard false).")),
                    ],
                ),
                &[],
            ),
            read_only: false,
            destructive: false,
            idempotent: true,
        },
        ToolDef {
            name: "decompose_task",
            title: "Aufgabe zerlegen",
            description: "Legt mehrere Unteraufgaben einer Aufgabe in einem Schritt an (alles oder \
                nichts). Ohne type erhalten sie den Typ aufgabe.",
            input_schema: object(
                with(
                    task_ref(),
                    vec![(
                        "subtasks",
                        json!({
                            "type": "array",
                            "description": "Die neuen Unteraufgaben in Reihenfolge.",
                            "minItems": 1,
                            "maxItems": 50,
                            "items": object(
                                vec![
                                    ("title", string("Titel (Pflicht).")),
                                    ("description", string("Beschreibung (Markdown).")),
                                    ("type", enumeration(TaskType::codes(), TYPE)),
                                    ("status", enumeration(TaskStatus::codes(), STATUS)),
                                    ("due", string(DUE)),
                                    ("remind_at", string(REMIND_AT)),
                                    ("plan", string("Plan (Markdown).")),
                                ],
                                &["title"],
                            ),
                        }),
                    )],
                ),
                &["subtasks"],
            ),
            read_only: false,
            destructive: false,
            idempotent: false,
        },
        ToolDef {
            name: "list_structure",
            title: "Struktur auflisten",
            description: "Listet Bereiche mit ihren Projekten (IDs für location, Vault-Abgleich über \
                die ID = zunder_id) sowie die Zahl offener Aufgaben in Inbox, Lose und je Projekt.",
            input_schema: object(
                vec![(
                    "include_archived",
                    boolean("Auch archivierte Bereiche und abgeschlossene Projekte (Standard false)."),
                )],
                &[],
            ),
            read_only: true,
            destructive: false,
            idempotent: true,
        },
        ToolDef {
            name: "create_area",
            title: "Bereich anlegen",
            description: "Legt einen Bereich an. Die ID ist die zunder_id im Vault-Frontmatter; eine \
                vorhandene zunder_id kann als id übergeben werden.",
            input_schema: object(
                vec![
                    ("name", string("Name (Pflicht).")),
                    ("state", enumeration(AreaState::codes(), "Zustand (Standard active).")),
                    ("vault_path", string("Pfad der Notiz im Vault (nur Info).")),
                    ("id", string("Optionale eigene ID (UUID, klein, mit Bindestrichen).")),
                ],
                &["name"],
            ),
            read_only: false,
            destructive: false,
            idempotent: false,
        },
        ToolDef {
            name: "update_area",
            title: "Bereich ändern",
            description: "Ändert Name, Zustand oder Vault-Pfad eines Bereichs.",
            input_schema: object(
                vec![
                    ("area_id", string("ID des Bereichs.")),
                    ("name", string("Neuer Name.")),
                    ("state", enumeration(AreaState::codes(), "active oder archived.")),
                    ("vault_path", nullable_string("Pfad der Notiz im Vault; null entfernt ihn.")),
                ],
                &["area_id"],
            ),
            read_only: false,
            destructive: false,
            idempotent: true,
        },
        ToolDef {
            name: "create_project",
            title: "Projekt anlegen",
            description: "Legt ein Projekt in einem Bereich an. Dev-Projekte (kind dev) erlauben \
                bug, feature und review und tragen optional das GitHub-Repo.",
            input_schema: object(
                vec![
                    ("name", string("Name (Pflicht).")),
                    ("area_id", string("ID des Bereichs (Pflicht).")),
                    ("status", enumeration(ProjectStatus::codes(), "Status (Standard active).")),
                    ("kind", enumeration(ProjectKind::codes(), "Art (Standard standard).")),
                    ("repo", string("GitHub-Repo owner/name (nur bei kind dev).")),
                    ("key", string("Schlüssel für die Kennungen der Aufgaben, z. B. ZUN → ZUN-1, ZUN-2 … (Großbuchstaben und Ziffern, höchstens 10 Zeichen, eindeutig).")),
                    ("description", string("Beschreibung (Markdown).")),
                    (
                        "view",
                        enumeration(ProjectView::codes(), "Ansicht (Standard: board bei dev, sonst list)."),
                    ),
                    ("id", string("Optionale eigene ID (UUID, klein, mit Bindestrichen).")),
                ],
                &["name", "area_id"],
            ),
            read_only: false,
            destructive: false,
            idempotent: false,
        },
        ToolDef {
            name: "update_project",
            title: "Projekt ändern",
            description: "Ändert Felder eines Projekts, auch den Bereich (area_id).",
            input_schema: object(
                vec![
                    ("project_id", string("ID des Projekts.")),
                    ("name", string("Neuer Name.")),
                    ("area_id", string("Neuer Bereich.")),
                    ("status", enumeration(ProjectStatus::codes(), "active, paused oder completed.")),
                    ("kind", enumeration(ProjectKind::codes(), "standard oder dev.")),
                    ("repo", nullable_string("GitHub-Repo owner/name; null entfernt es.")),
                    ("key", nullable_string("Schlüssel für die Kennungen (z. B. ZUN); null entfernt ihn. Vergebene Kennungen bleiben, neue Aufgaben zählen unter dem neuen Schlüssel.")),
                    ("description", nullable_string("Beschreibung (Markdown); null entfernt sie.")),
                    (
                        "view",
                        json!({
                            "type": ["string", "null"],
                            "enum": [ProjectView::List.as_str(), ProjectView::Board.as_str(), null],
                            "description": "list oder board; null stellt den Standard der Art wieder her.",
                        }),
                    ),
                ],
                &["project_id"],
            ),
            read_only: false,
            destructive: false,
            idempotent: true,
        },
        ToolDef {
            name: "start_flame",
            title: "Flamme starten",
            description: "Startet eine Flamme (Deep-Work-Block) an einer Aufgabe. Eine laufende \
                Flamme an einer anderen Aufgabe endet dabei. Ist Timing angebunden, startet ein \
                Timer in Timing (timing_ref); sonst werden nur Start- und Endzeit gespeichert. \
                Das Ergebnis sagt, ob Timing genutzt wurde.",
            input_schema: object(task_ref(), &[]),
            read_only: false,
            destructive: false,
            idempotent: true,
        },
        ToolDef {
            name: "stop_flame",
            title: "Flamme beenden",
            description: "Beendet die laufende Flamme einer Aufgabe und stoppt den zugehörigen \
                Timing-Timer, falls er noch läuft. Ohne task_id und clickup_id endet die gerade \
                laufende Flamme, wo immer sie brennt.",
            input_schema: object(task_ref(), &[]),
            read_only: false,
            destructive: false,
            idempotent: false,
        },
        ToolDef {
            name: "get_task_history",
            title: "Änderungsprotokoll",
            description: "Liefert das Änderungsprotokoll einer Aufgabe (auch gelöschter): jede \
                Feldänderung mit Wert, Zeitpunkt und Quelle (web, api:<token>, mcp:<token>, server).",
            input_schema: object(
                with(
                    task_ref(),
                    vec![(
                        "limit",
                        integer("Höchstzahl der Einträge (die neuesten).", 1, 1000, 100),
                    )],
                ),
                &[],
            ),
            read_only: true,
            destructive: false,
            idempotent: true,
        },
    ]
}
