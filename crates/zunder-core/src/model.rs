//! Typed domain model: field names, enums, value types, typed entity views
//! and field-level value validation (contract section 2.4).

use std::fmt;
use std::str::FromStr;

use crate::ids::Id;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::{Date, OffsetDateTime, Weekday};

use crate::op::EntityKind;
use crate::validate::RejectCode;

/// Field names per entity kind.
pub mod fields {
    use crate::op::EntityKind;

    /// Tombstone field, shared by all kinds.
    pub const DELETED: &str = "deleted";

    pub mod area {
        pub const NAME: &str = "name";
        pub const STATE: &str = "state";
        pub const VAULT_PATH: &str = "vault_path";
        pub const DELETED: &str = super::DELETED;
        pub const ALL: &[&str] = &[NAME, STATE, VAULT_PATH, DELETED];
        pub const REQUIRED: &[&str] = &[NAME];
    }

    pub mod project {
        pub const NAME: &str = "name";
        pub const AREA_ID: &str = "area_id";
        pub const STATUS: &str = "status";
        pub const KIND: &str = "kind";
        pub const REPO: &str = "repo";
        pub const DESCRIPTION: &str = "description";
        pub const VIEW: &str = "view";
        /// Prefix of the task references (`ZUN` → `ZUN-42`), unique among
        /// live projects.
        pub const KEY: &str = "key";
        pub const DELETED: &str = super::DELETED;
        pub const ALL: &[&str] = &[
            NAME,
            AREA_ID,
            STATUS,
            KIND,
            REPO,
            DESCRIPTION,
            VIEW,
            KEY,
            DELETED,
        ];
        pub const REQUIRED: &[&str] = &[NAME, AREA_ID];
    }

    pub mod task {
        pub const TITLE: &str = "title";
        pub const DESCRIPTION: &str = "description";
        pub const TYPE: &str = "type";
        pub const STATUS: &str = "status";
        pub const LOCATION: &str = "location";
        pub const PARENT_ID: &str = "parent_id";
        pub const DUE: &str = "due";
        pub const REMIND_AT: &str = "remind_at";
        pub const VAULT_LINKS: &str = "vault_links";
        pub const LINKS: &str = "links";
        pub const PHOENIX: &str = "phoenix";
        pub const CLAIMED_BY: &str = "claimed_by";
        pub const CLAIM_EXPIRES_AT: &str = "claim_expires_at";
        pub const PLAN: &str = "plan";
        pub const BRANCH: &str = "branch";
        pub const PR_URL: &str = "pr_url";
        pub const TIMING_REF: &str = "timing_ref";
        pub const FLAME_STARTED_AT: &str = "flame_started_at";
        pub const FLAME_ENDED_AT: &str = "flame_ended_at";
        pub const CLICKUP_ID: &str = "clickup_id";
        pub const RANK: &str = "rank";
        /// When the owner decided to leave the task without a due date on
        /// purpose ("bewusst undatiert"); unset = no decision.
        pub const UNDATED_ACK: &str = "undated_ack";
        /// Tasks that have to be finished before this one (IDs). The reverse
        /// direction ("blocks") is derived, not stored.
        pub const BLOCKED_BY: &str = "blocked_by";
        /// Human-readable reference (`ZUN-42`). Assigned once by the server,
        /// never changed afterwards (see [`crate::numbering`]).
        pub const REF: &str = "ref";
        pub const DELETED: &str = super::DELETED;
        pub const ALL: &[&str] = &[
            TITLE,
            DESCRIPTION,
            TYPE,
            STATUS,
            LOCATION,
            PARENT_ID,
            DUE,
            REMIND_AT,
            VAULT_LINKS,
            LINKS,
            PHOENIX,
            CLAIMED_BY,
            CLAIM_EXPIRES_AT,
            PLAN,
            BRANCH,
            PR_URL,
            TIMING_REF,
            FLAME_STARTED_AT,
            FLAME_ENDED_AT,
            CLICKUP_ID,
            RANK,
            UNDATED_ACK,
            BLOCKED_BY,
            REF,
            DELETED,
        ];
        pub const REQUIRED: &[&str] = &[TITLE];
    }

    pub mod comment {
        /// The task the comment belongs to.
        pub const TASK_ID: &str = "task_id";
        /// Markdown text, not empty.
        pub const BODY: &str = "body";
        /// Who wrote it (agent or tool name); unset = the owner.
        pub const AUTHOR: &str = "author";
        pub const DELETED: &str = super::DELETED;
        pub const ALL: &[&str] = &[TASK_ID, BODY, AUTHOR, DELETED];
        pub const REQUIRED: &[&str] = &[TASK_ID, BODY];
    }

    /// All known fields of an entity kind.
    pub fn all(kind: EntityKind) -> &'static [&'static str] {
        match kind {
            EntityKind::Area => area::ALL,
            EntityKind::Project => project::ALL,
            EntityKind::Task => task::ALL,
            EntityKind::Comment => comment::ALL,
        }
    }

    /// Fields every live entity of this kind must have.
    pub fn required(kind: EntityKind) -> &'static [&'static str] {
        match kind {
            EntityKind::Area => area::REQUIRED,
            EntityKind::Project => project::REQUIRED,
            EntityKind::Task => task::REQUIRED,
            EntityKind::Comment => comment::REQUIRED,
        }
    }

    pub fn is_known(kind: EntityKind, field: &str) -> bool {
        all(kind).contains(&field)
    }
}

/// Maximum length (in characters) of task titles and area/project names.
pub const MAX_TITLE_CHARS: usize = 500;

/// Maximum number of entries in a task's `blocked_by`.
pub const MAX_BLOCKED_BY: usize = 100;

macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        $name:ident, default = $default:ident {
            $($variant:ident => $code:literal, $label:literal;)+
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub enum $name {
            $(
                #[doc = $label]
                #[serde(rename = $code)]
                $variant,
            )+
        }

        impl Default for $name {
            fn default() -> Self {
                $name::$default
            }
        }

        impl $name {
            /// All values in declaration order.
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            /// The wire code (English, as in the contract).
            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $code),+ }
            }

            /// German label for the UI and messages.
            pub fn label_de(self) -> &'static str {
                match self { $($name::$variant => $label),+ }
            }

            /// All wire codes, e.g. for JSON schemas.
            pub fn codes() -> Vec<&'static str> {
                Self::ALL.iter().map(|v| v.as_str()).collect()
            }
        }

        impl From<$name> for Value {
            fn from(v: $name) -> Value {
                Value::String(v.as_str().to_owned())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = UnknownCode;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($code => Ok($name::$variant),)+
                    other => Err(UnknownCode(other.to_owned())),
                }
            }
        }
    };
}

/// Error for an unknown enum code.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown code {0:?}")]
pub struct UnknownCode(pub String);

string_enum! {
    /// Task type. `bug` and `feature` are only allowed in dev projects.
    TaskType, default = Aufgabe {
        Aufgabe => "aufgabe", "Aufgabe";
        Bug => "bug", "Bug";
        Feature => "feature", "Feature";
    }
}

string_enum! {
    /// Task status. `review` is only allowed in dev projects.
    TaskStatus, default = Open {
        Open => "open", "offen";
        InProgress => "in_progress", "in Arbeit";
        Review => "review", "Review";
        Blocked => "blocked", "blockiert";
        Done => "done", "erledigt";
        Dropped => "dropped", "verworfen";
    }
}

string_enum! {
    /// Area state.
    AreaState, default = Active {
        Active => "active", "aktiv";
        Archived => "archived", "archiviert";
    }
}

string_enum! {
    /// Project status.
    ProjectStatus, default = Active {
        Active => "active", "aktiv";
        Paused => "paused", "pausiert";
        Completed => "completed", "abgeschlossen";
    }
}

string_enum! {
    /// Project kind.
    ProjectKind, default = Standard {
        Standard => "standard", "Standard";
        Dev => "dev", "Dev";
    }
}

string_enum! {
    /// Default presentation of a project.
    ProjectView, default = List {
        List => "list", "Liste";
        Board => "board", "Board";
    }
}

string_enum! {
    /// Role of a task in the Phönix system.
    PhoenixRole, default = FeuerSchritt {
        FeuerSchritt => "feuer_schritt", "Feuer-Schritt";
        Flamme => "flamme", "Flamme";
    }
}

impl TaskType {
    /// Whether this type is only allowed in dev projects.
    pub fn is_dev_only(self) -> bool {
        matches!(self, TaskType::Bug | TaskType::Feature)
    }
}

impl TaskStatus {
    /// Whether this status is only allowed in dev projects.
    pub fn is_dev_only(self) -> bool {
        self == TaskStatus::Review
    }

    /// `done` or `dropped`.
    pub fn is_closed(self) -> bool {
        matches!(self, TaskStatus::Done | TaskStatus::Dropped)
    }
}

impl ProjectView {
    /// Default view when none is set: `board` for dev projects, else `list`.
    pub fn default_for(kind: ProjectKind) -> Self {
        match kind {
            ProjectKind::Dev => ProjectView::Board,
            ProjectKind::Standard => ProjectView::List,
        }
    }
}

/// Where a task lives. Serialized as `{"kind":"inbox"}`,
/// `{"kind":"area","id":"…"}` etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", try_from = "RawLocation")]
pub enum Location {
    #[default]
    Inbox,
    Loose,
    Area {
        id: Id,
    },
    Project {
        id: Id,
    },
}

/// Strict wire form of [`Location`]: exactly `kind` plus `id` for area and
/// project (internally tagged enums cannot reject unknown keys on unit
/// variants).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLocation {
    kind: String,
    #[serde(default)]
    id: Option<Id>,
}

impl TryFrom<RawLocation> for Location {
    type Error = String;
    fn try_from(raw: RawLocation) -> Result<Self, Self::Error> {
        match (raw.kind.as_str(), raw.id) {
            ("inbox", None) => Ok(Location::Inbox),
            ("loose", None) => Ok(Location::Loose),
            ("area", Some(id)) => Ok(Location::Area { id }),
            ("project", Some(id)) => Ok(Location::Project { id }),
            (kind, _) => Err(format!("invalid location of kind {kind:?}")),
        }
    }
}

impl Location {
    pub fn to_value(self) -> Value {
        serde_json::to_value(self).expect("location serializes")
    }

    /// The referenced entity, if any.
    pub fn target(self) -> Option<(EntityKind, Id)> {
        match self {
            Location::Inbox | Location::Loose => None,
            Location::Area { id } => Some((EntityKind::Area, id)),
            Location::Project { id } => Some((EntityKind::Project, id)),
        }
    }
}

impl From<Location> for Value {
    fn from(l: Location) -> Value {
        l.to_value()
    }
}

/// Due date: a calendar date (`YYYY-MM-DD`) or an RFC 3339 timestamp with
/// offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Due {
    Date(Date),
    DateTime(OffsetDateTime),
}

impl Due {
    /// Parses `YYYY-MM-DD` or RFC 3339.
    pub fn parse(s: &str) -> Option<Self> {
        if let Some(date) = parse_plain_date(s) {
            return Some(Due::Date(date));
        }
        parse_rfc3339(s).map(Due::DateTime)
    }

    /// The calendar date (for timestamps: in their own offset).
    pub fn date(&self) -> Date {
        match self {
            Due::Date(d) => *d,
            Due::DateTime(dt) => dt.date(),
        }
    }

    /// Wire string (`YYYY-MM-DD` or RFC 3339).
    pub fn to_wire(&self) -> String {
        match self {
            Due::Date(d) => format_date(*d),
            Due::DateTime(dt) => dt.format(&Rfc3339).expect("valid timestamp formats"),
        }
    }
}

impl From<Due> for Value {
    fn from(d: Due) -> Value {
        Value::String(d.to_wire())
    }
}

impl Serialize for Due {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_wire())
    }
}

impl<'de> Deserialize<'de> for Due {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Due::parse(&s).ok_or_else(|| de::Error::custom(format!("invalid due date {s:?}")))
    }
}

/// ISO week identifier of a "Feuer", e.g. `2026-W45`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IsoWeek {
    year: i32,
    week: u8,
}

impl IsoWeek {
    /// Validates year and week (week 53 only exists in some years).
    pub fn new(year: i32, week: u8) -> Option<Self> {
        if !(0..=9999).contains(&year) {
            return None;
        }
        Date::from_iso_week_date(year, week, Weekday::Monday).ok()?;
        Some(Self { year, week })
    }

    /// The ISO week containing `date`.
    pub fn of(date: Date) -> Self {
        let (year, week, _) = date.to_iso_week_date();
        Self { year, week }
    }

    pub fn year(self) -> i32 {
        self.year
    }

    pub fn week(self) -> u8 {
        self.week
    }

    /// Monday of this week.
    pub fn monday(self) -> Date {
        Date::from_iso_week_date(self.year, self.week, Weekday::Monday)
            .expect("validated on construction")
    }

    /// Strictly parses `YYYY-Www`.
    pub fn parse(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        if b.len() != 8 || b[4] != b'-' || b[5] != b'W' {
            return None;
        }
        if !b[..4].iter().chain(&b[6..]).all(u8::is_ascii_digit) {
            return None;
        }
        Self::new(s[..4].parse().ok()?, s[6..].parse().ok()?)
    }
}

impl fmt::Display for IsoWeek {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-W{:02}", self.year, self.week)
    }
}

impl FromStr for IsoWeek {
    type Err = UnknownCode;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| UnknownCode(s.to_owned()))
    }
}

impl Serialize for IsoWeek {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for IsoWeek {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        IsoWeek::parse(&s).ok_or_else(|| de::Error::custom(format!("invalid ISO week {s:?}")))
    }
}

/// Phönix marker of a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Phoenix {
    pub role: PhoenixRole,
    pub fire: IsoWeek,
}

impl From<Phoenix> for Value {
    fn from(p: Phoenix) -> Value {
        serde_json::to_value(p).expect("phoenix serializes")
    }
}

/// External link of a task.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Typed view of an area.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Area {
    pub id: Id,
    pub name: String,
    pub state: AreaState,
    pub vault_path: Option<String>,
    pub deleted: bool,
}

/// Typed view of a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: Id,
    pub name: String,
    pub area_id: Id,
    pub status: ProjectStatus,
    pub kind: ProjectKind,
    pub repo: Option<String>,
    pub description: Option<String>,
    /// Effective view (explicit value or the default for `kind`).
    pub view: ProjectView,
    pub key: Option<String>,
    pub deleted: bool,
}

/// Typed view of a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: Id,
    pub title: String,
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub task_type: TaskType,
    pub status: TaskStatus,
    /// The task's own location. For subtasks it is ignored; use
    /// [`crate::state::State::effective_location`].
    pub location: Location,
    pub parent_id: Option<Id>,
    pub due: Option<Due>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub remind_at: Option<OffsetDateTime>,
    pub vault_links: Vec<String>,
    pub links: Vec<Link>,
    pub phoenix: Option<Phoenix>,
    pub claimed_by: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub claim_expires_at: Option<OffsetDateTime>,
    pub plan: Option<String>,
    pub branch: Option<String>,
    pub pr_url: Option<String>,
    pub timing_ref: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub flame_started_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub flame_ended_at: Option<OffsetDateTime>,
    pub clickup_id: Option<String>,
    pub rank: Option<String>,
    /// When the task was marked as deliberately undated.
    #[serde(with = "time::serde::rfc3339::option")]
    pub undated_ack: Option<OffsetDateTime>,
    /// IDs of the tasks this one waits for (as stored; may name deleted
    /// tasks, which no longer block).
    pub blocked_by: Vec<Id>,
    /// Reference such as `ZUN-42`, once the server has assigned it.
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub deleted: bool,
}

/// Typed view of a comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub id: Id,
    pub task_id: Id,
    pub body: String,
    pub author: Option<String>,
    pub deleted: bool,
}

/// Rejection of a single field value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueError {
    pub code: RejectCode,
    pub message: String,
}

impl ValueError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: RejectCode::InvalidValue,
            message: message.into(),
        }
    }
}

/// The value shape of a field.
#[derive(Debug, Clone, Copy)]
enum Shape {
    /// Non-empty (after trimming) text of at most [`MAX_TITLE_CHARS`] chars.
    Title,
    Text,
    /// Non-empty (after trimming) text without length limit.
    Body,
    Bool,
    Enum(fn() -> Vec<&'static str>),
    Id,
    Location,
    Due,
    Timestamp,
    StringList,
    /// Distinct canonical IDs, at most [`MAX_BLOCKED_BY`].
    IdList,
    Links,
    Phoenix,
    Repo,
    HttpUrl,
    ProjectKey,
    TaskRef,
}

fn shape_of(kind: EntityKind, field: &str) -> Option<Shape> {
    use fields::{area as a, comment as c, project as p, task as t};
    if field == fields::DELETED {
        return Some(Shape::Bool);
    }
    let shape = match kind {
        EntityKind::Area => match field {
            a::NAME => Shape::Title,
            a::STATE => Shape::Enum(AreaState::codes),
            a::VAULT_PATH => Shape::Text,
            _ => return None,
        },
        EntityKind::Project => match field {
            p::NAME => Shape::Title,
            p::AREA_ID => Shape::Id,
            p::STATUS => Shape::Enum(ProjectStatus::codes),
            p::KIND => Shape::Enum(ProjectKind::codes),
            p::REPO => Shape::Repo,
            p::DESCRIPTION => Shape::Text,
            p::VIEW => Shape::Enum(ProjectView::codes),
            p::KEY => Shape::ProjectKey,
            _ => return None,
        },
        EntityKind::Task => match field {
            t::TITLE => Shape::Title,
            t::TYPE => Shape::Enum(TaskType::codes),
            t::STATUS => Shape::Enum(TaskStatus::codes),
            t::LOCATION => Shape::Location,
            t::PARENT_ID => Shape::Id,
            t::DUE => Shape::Due,
            t::REMIND_AT
            | t::CLAIM_EXPIRES_AT
            | t::FLAME_STARTED_AT
            | t::FLAME_ENDED_AT
            | t::UNDATED_ACK => Shape::Timestamp,
            t::VAULT_LINKS => Shape::StringList,
            t::BLOCKED_BY => Shape::IdList,
            t::LINKS => Shape::Links,
            t::PHOENIX => Shape::Phoenix,
            t::PR_URL => Shape::HttpUrl,
            t::REF => Shape::TaskRef,
            t::DESCRIPTION
            | t::CLAIMED_BY
            | t::PLAN
            | t::BRANCH
            | t::TIMING_REF
            | t::CLICKUP_ID
            | t::RANK => Shape::Text,
            _ => return None,
        },
        EntityKind::Comment => match field {
            c::TASK_ID => Shape::Id,
            c::BODY => Shape::Body,
            c::AUTHOR => Shape::Title,
            _ => return None,
        },
    };
    Some(shape)
}

/// Validates the value of a single field operation.
///
/// `null` is accepted for every optional field (it resets the field) and
/// rejected for required fields. `deleted` accepts `true` (tombstone) as
/// well as `false`/`null` (restore).
pub fn validate_field_value(
    kind: EntityKind,
    field: &str,
    value: &Value,
) -> Result<(), ValueError> {
    let Some(shape) = shape_of(kind, field) else {
        return Err(ValueError {
            code: RejectCode::UnknownField,
            message: format!("Unbekanntes Feld „{field}“ für {}.", kind.label_de()),
        });
    };
    if value.is_null() {
        if fields::required(kind).contains(&field) {
            return Err(ValueError::invalid(format!(
                "Pflichtfeld „{field}“ kann nicht zurückgesetzt werden."
            )));
        }
        return Ok(());
    }
    check_shape(shape, field, value)
}

fn check_shape(shape: Shape, field: &str, value: &Value) -> Result<(), ValueError> {
    let text = || {
        value
            .as_str()
            .ok_or_else(|| ValueError::invalid(format!("„{field}“ muss ein Text sein.")))
    };
    match shape {
        Shape::Title => {
            let s = text()?;
            if s.trim().is_empty() {
                return Err(ValueError::invalid(format!(
                    "„{field}“ darf nicht leer sein."
                )));
            }
            if s.chars().count() > MAX_TITLE_CHARS {
                return Err(ValueError::invalid(format!(
                    "„{field}“ darf höchstens {MAX_TITLE_CHARS} Zeichen lang sein."
                )));
            }
        }
        Shape::Text => {
            text()?;
        }
        Shape::Body => {
            if text()?.trim().is_empty() {
                return Err(ValueError::invalid(format!(
                    "„{field}“ darf nicht leer sein."
                )));
            }
        }
        Shape::Bool => {
            if !value.is_boolean() {
                return Err(ValueError::invalid(format!(
                    "„{field}“ muss true oder false sein."
                )));
            }
        }
        Shape::Enum(codes) => {
            let s = text()?;
            let codes = codes();
            if !codes.contains(&s) {
                return Err(ValueError::invalid(format!(
                    "Ungültiger Wert „{s}“ für „{field}“. Erlaubt: {}.",
                    codes.join(", ")
                )));
            }
        }
        Shape::Id => {
            let s = text()?;
            if Id::parse(s).is_none() {
                return Err(ValueError::invalid(format!(
                    "„{field}“ muss eine ID sein (24 Kleinbuchstaben und Ziffern, beginnend mit einem Buchstaben)."
                )));
            }
        }
        Shape::Location => {
            // Only canonical IDs are stored (legacy UUIDs are converted on input).
            let canonical_id = value
                .get("id")
                .and_then(Value::as_str)
                .is_none_or(|s| Id::parse(s).is_some());
            serde_json::from_value::<Location>(value.clone())
                .ok()
                .filter(|_| canonical_id)
                .ok_or(())
                .map_err(|_| {
                    ValueError::invalid(
                        "„location“ muss {\"kind\":\"inbox\"}, {\"kind\":\"loose\"}, \
                     {\"kind\":\"area\",\"id\":…} oder {\"kind\":\"project\",\"id\":…} sein.",
                    )
                })?;
        }
        Shape::Due => {
            let s = text()?;
            if Due::parse(s).is_none() {
                return Err(ValueError::invalid(format!(
                    "„{field}“ muss ein Datum (JJJJ-MM-TT) oder ein RFC-3339-Zeitpunkt mit Offset sein."
                )));
            }
        }
        Shape::Timestamp => {
            let s = text()?;
            if parse_rfc3339(s).is_none() {
                return Err(ValueError::invalid(format!(
                    "„{field}“ muss ein RFC-3339-Zeitpunkt mit Offset sein."
                )));
            }
        }
        Shape::StringList => {
            let ok = value.as_array().is_some_and(|items| {
                items
                    .iter()
                    .all(|v| v.as_str().is_some_and(|s| !s.is_empty()))
            });
            if !ok {
                return Err(ValueError::invalid(format!(
                    "„{field}“ muss eine Liste nicht-leerer Texte sein."
                )));
            }
        }
        Shape::IdList => {
            let ok = value.as_array().is_some_and(|items| {
                let mut seen = std::collections::HashSet::new();
                items.len() <= MAX_BLOCKED_BY
                    && items.iter().all(|v| {
                        v.as_str()
                            .and_then(Id::parse)
                            .is_some_and(|id| seen.insert(id))
                    })
            });
            if !ok {
                return Err(ValueError::invalid(format!(
                    "„{field}“ muss eine Liste verschiedener Aufgaben-IDs sein (höchstens {MAX_BLOCKED_BY})."
                )));
            }
        }
        Shape::Links => {
            let links = serde_json::from_value::<Vec<Link>>(value.clone()).map_err(|_| {
                ValueError::invalid(
                    "„links“ muss eine Liste von {\"url\": Text, \"title\"?: Text} sein.",
                )
            })?;
            if let Some(bad) = links.iter().find(|l| !is_absolute_url(&l.url)) {
                return Err(ValueError::invalid(format!(
                    "„{}“ ist keine gültige absolute URL.",
                    bad.url
                )));
            }
        }
        Shape::Phoenix => {
            serde_json::from_value::<Phoenix>(value.clone()).map_err(|_| {
                ValueError::invalid(
                    "„phoenix“ muss null oder {\"role\":\"feuer_schritt\"|\"flamme\",\
                     \"fire\":\"JJJJ-Www\"} mit gültiger ISO-Woche sein.",
                )
            })?;
        }
        Shape::Repo => {
            let s = text()?;
            if !is_valid_repo(s) {
                return Err(ValueError::invalid(format!(
                    "„{field}“ muss die Form „owner/name“ haben."
                )));
            }
        }
        Shape::HttpUrl => {
            let s = text()?;
            if !is_http_url(s) {
                return Err(ValueError::invalid(format!(
                    "„{field}“ muss eine http(s)-URL sein."
                )));
            }
        }
        Shape::ProjectKey => {
            let s = text()?;
            if !crate::numbering::is_valid_key(s) {
                return Err(ValueError::invalid(format!(
                    "„{field}“ muss mit einem Großbuchstaben beginnen und darf nur Großbuchstaben \
                     und Ziffern enthalten (höchstens {} Zeichen), z. B. „ZUN“.",
                    crate::numbering::MAX_KEY_CHARS
                )));
            }
        }
        Shape::TaskRef => {
            let s = text()?;
            if crate::numbering::parse_ref(s).is_none() {
                return Err(ValueError::invalid(format!(
                    "„{field}“ muss die Form „SCHLÜSSEL-Nummer“ haben, z. B. „ZUN-42“."
                )));
            }
        }
    }
    Ok(())
}

/// `owner/name` in GitHub style.
pub fn is_valid_repo(s: &str) -> bool {
    let Some((owner, name)) = s.split_once('/') else {
        return false;
    };
    let owner_ok = (1..=39).contains(&owner.len())
        && owner
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && !owner.starts_with('-')
        && !owner.ends_with('-');
    let name_ok = (1..=100).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        && name != "."
        && name != "..";
    owner_ok && name_ok
}

/// Absolute URL with a scheme (any scheme).
pub fn is_absolute_url(s: &str) -> bool {
    url::Url::parse(s).is_ok()
}

/// Absolute `http`/`https` URL with a host.
pub fn is_http_url(s: &str) -> bool {
    url::Url::parse(s)
        .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
}

/// Parses an RFC 3339 timestamp (offset required).
pub fn parse_rfc3339(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s, &Rfc3339).ok()
}

/// Formats a timestamp as RFC 3339.
pub fn format_rfc3339(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).expect("valid timestamp formats")
}

/// Strictly parses `YYYY-MM-DD`.
pub fn parse_plain_date(s: &str) -> Option<Date> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    Date::parse(s, format_description!("[year]-[month]-[day]")).ok()
}

/// Formats a date as `YYYY-MM-DD`.
pub fn format_date(d: Date) -> String {
    d.format(format_description!("[year]-[month]-[day]"))
        .expect("valid date formats")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ID: &str = "tz4a98xxat96iws9zmbrgj3a";

    fn ok(kind: EntityKind, field: &str, value: Value) {
        if let Err(e) = validate_field_value(kind, field, &value) {
            panic!("{kind}.{field} = {value} rejected: {e:?}");
        }
    }

    fn bad(kind: EntityKind, field: &str, value: Value) -> RejectCode {
        match validate_field_value(kind, field, &value) {
            Ok(()) => panic!("{kind}.{field} = {value} accepted"),
            Err(e) => {
                assert!(!e.message.is_empty());
                e.code
            }
        }
    }

    #[test]
    fn enums_serialize_as_contract_codes() {
        assert_eq!(
            serde_json::to_value(TaskType::Aufgabe).unwrap(),
            json!("aufgabe")
        );
        assert_eq!(
            serde_json::to_value(TaskStatus::InProgress).unwrap(),
            json!("in_progress")
        );
        assert_eq!(
            serde_json::to_value(PhoenixRole::FeuerSchritt).unwrap(),
            json!("feuer_schritt")
        );
        assert_eq!(TaskStatus::default(), TaskStatus::Open);
        assert_eq!(TaskType::default(), TaskType::Aufgabe);
        assert_eq!(ProjectKind::default(), ProjectKind::Standard);
        assert_eq!(ProjectStatus::default(), ProjectStatus::Active);
        assert_eq!(AreaState::default(), AreaState::Active);
        assert_eq!(
            ProjectView::default_for(ProjectKind::Dev),
            ProjectView::Board
        );
        assert_eq!(
            ProjectView::default_for(ProjectKind::Standard),
            ProjectView::List
        );
        assert_eq!(
            "blocked".parse::<TaskStatus>().unwrap(),
            TaskStatus::Blocked
        );
        assert!("Blocked".parse::<TaskStatus>().is_err());
        assert_eq!(TaskStatus::Done.label_de(), "erledigt");
    }

    #[test]
    fn location_serde() {
        assert_eq!(Location::Inbox.to_value(), json!({"kind": "inbox"}));
        assert_eq!(Location::Loose.to_value(), json!({"kind": "loose"}));
        let id = Id::parse(ID).unwrap();
        assert_eq!(
            Location::Project { id }.to_value(),
            json!({"kind": "project", "id": ID})
        );
        assert_eq!(
            serde_json::from_value::<Location>(json!({"kind": "area", "id": ID})).unwrap(),
            Location::Area { id }
        );
        for v in [
            json!({"kind": "inbox", "id": ID}),
            json!({"kind": "area"}),
            json!({"kind": "project", "id": "x"}),
            json!({"kind": "project", "id": ID.to_uppercase()}),
            json!({"kind": "space", "id": ID}),
            json!("inbox"),
        ] {
            assert!(
                serde_json::from_value::<Location>(v.clone()).is_err(),
                "{v}"
            );
        }
    }

    #[test]
    fn due_and_week_parsing() {
        assert!(matches!(Due::parse("2026-10-11"), Some(Due::Date(_))));
        assert!(matches!(
            Due::parse("2026-10-11T09:00:00+02:00"),
            Some(Due::DateTime(_))
        ));
        assert!(Due::parse("2026-10-11T09:00:00").is_none());
        assert!(Due::parse("2026-13-01").is_none());
        assert!(Due::parse("2026-1-01").is_none());
        assert!(Due::parse("11.10.2026").is_none());
        assert_eq!(Due::parse("2026-10-11").unwrap().to_wire(), "2026-10-11");

        assert_eq!(IsoWeek::parse("2026-W45").unwrap().to_string(), "2026-W45");
        assert!(IsoWeek::parse("2026-W53").is_some()); // 2026 has 53 ISO weeks
        assert!(IsoWeek::parse("2025-W53").is_none());
        assert!(IsoWeek::parse("2026-W00").is_none());
        assert!(IsoWeek::parse("2026-W5").is_none());
        assert!(IsoWeek::parse("2026W45").is_none());
        assert!(IsoWeek::parse("2026-w45").is_none());
        let date = parse_plain_date("2026-10-11").unwrap();
        assert_eq!(IsoWeek::of(date).to_string(), "2026-W41");
        assert_eq!(format_date(IsoWeek::of(date).monday()), "2026-10-05");
    }

    #[test]
    fn unknown_fields() {
        assert_eq!(
            bad(EntityKind::Task, "priority", json!(1)),
            RejectCode::UnknownField
        );
        assert_eq!(
            bad(EntityKind::Area, "kind", json!("dev")),
            RejectCode::UnknownField
        );
        assert_eq!(
            bad(EntityKind::Project, "title", json!("x")),
            RejectCode::UnknownField
        );
        for kind in EntityKind::ALL {
            for field in fields::all(kind) {
                assert!(
                    shape_of(kind, field).is_some(),
                    "{kind}.{field} has no shape"
                );
            }
        }
    }

    #[test]
    fn titles_and_names() {
        ok(EntityKind::Task, "title", json!("Zahnarzt anrufen"));
        ok(EntityKind::Task, "title", json!("ä".repeat(500)));
        assert_eq!(
            bad(EntityKind::Task, "title", json!("ä".repeat(501))),
            RejectCode::InvalidValue
        );
        bad(EntityKind::Task, "title", json!(""));
        bad(EntityKind::Task, "title", json!("   "));
        bad(EntityKind::Task, "title", json!(null));
        bad(EntityKind::Task, "title", json!(5));
        ok(EntityKind::Area, "name", json!("Gesundheit"));
        bad(EntityKind::Area, "name", json!(null));
        bad(EntityKind::Project, "name", json!(""));
        bad(EntityKind::Project, "area_id", json!(null));
    }

    #[test]
    fn enum_fields() {
        ok(EntityKind::Task, "type", json!("bug"));
        ok(EntityKind::Task, "type", json!(null));
        bad(EntityKind::Task, "type", json!("epic"));
        ok(EntityKind::Task, "status", json!("in_progress"));
        bad(EntityKind::Task, "status", json!("closed"));
        bad(EntityKind::Task, "status", json!(1));
        ok(EntityKind::Area, "state", json!("archived"));
        bad(EntityKind::Area, "state", json!("deleted"));
        ok(EntityKind::Project, "status", json!("paused"));
        ok(EntityKind::Project, "kind", json!("dev"));
        ok(EntityKind::Project, "view", json!("board"));
        bad(EntityKind::Project, "view", json!("gantt"));
    }

    #[test]
    fn ids_and_locations() {
        ok(EntityKind::Task, "parent_id", json!(ID));
        ok(EntityKind::Task, "parent_id", json!(null));
        bad(EntityKind::Task, "parent_id", json!("abc"));
        ok(EntityKind::Project, "area_id", json!(ID));
        ok(EntityKind::Task, "location", json!({"kind": "loose"}));
        ok(
            EntityKind::Task,
            "location",
            json!({"kind": "project", "id": ID}),
        );
        ok(EntityKind::Task, "location", json!(null));
        bad(EntityKind::Task, "location", json!({"kind": "project"}));
    }

    #[test]
    fn dates() {
        ok(EntityKind::Task, "due", json!("2026-10-12"));
        ok(EntityKind::Task, "due", json!("2026-10-12T09:00:00Z"));
        ok(
            EntityKind::Task,
            "due",
            json!("2026-10-12T09:00:00.123+02:00"),
        );
        bad(EntityKind::Task, "due", json!("morgen"));
        bad(EntityKind::Task, "due", json!("2026-02-30"));
        ok(
            EntityKind::Task,
            "remind_at",
            json!("2026-10-12T09:00:00+02:00"),
        );
        bad(EntityKind::Task, "remind_at", json!("2026-10-12"));
        bad(EntityKind::Task, "remind_at", json!("2026-10-12T09:00:00"));
        ok(
            EntityKind::Task,
            "claim_expires_at",
            json!("2026-10-12T09:00:00Z"),
        );
        ok(
            EntityKind::Task,
            "flame_started_at",
            json!("2026-10-12T09:00:00Z"),
        );
        bad(EntityKind::Task, "flame_ended_at", json!(1728650000000u64));
        ok(
            EntityKind::Task,
            "undated_ack",
            json!("2026-10-12T09:00:00Z"),
        );
        ok(EntityKind::Task, "undated_ack", json!(null));
        bad(EntityKind::Task, "undated_ack", json!("2026-10-12"));
        bad(EntityKind::Task, "undated_ack", json!(true));
    }

    #[test]
    fn lists_links_phoenix() {
        ok(
            EntityKind::Task,
            "vault_links",
            json!(["Bereiche/Gesundheit.md", "obsidian://open?vault=x"]),
        );
        ok(EntityKind::Task, "vault_links", json!([]));
        bad(
            EntityKind::Task,
            "vault_links",
            json!("Bereiche/Gesundheit.md"),
        );
        bad(EntityKind::Task, "vault_links", json!([""]));
        bad(EntityKind::Task, "vault_links", json!([1]));
        ok(
            EntityKind::Task,
            "links",
            json!([{"url": "https://example.org"}, {"url": "https://example.org/a", "title": "A"}]),
        );
        bad(EntityKind::Task, "links", json!([{"url": "example.org"}]));
        bad(
            EntityKind::Task,
            "links",
            json!([{"url": "https://example.org", "extra": 1}]),
        );
        bad(EntityKind::Task, "links", json!([{"title": "x"}]));
        ok(EntityKind::Task, "blocked_by", json!([ID]));
        ok(EntityKind::Task, "blocked_by", json!([]));
        ok(EntityKind::Task, "blocked_by", json!(null));
        bad(EntityKind::Task, "blocked_by", json!(ID));
        bad(EntityKind::Task, "blocked_by", json!([ID, ID]));
        bad(EntityKind::Task, "blocked_by", json!(["ZUN-1"]));
        bad(EntityKind::Task, "blocked_by", json!([ID.to_uppercase()]));
        let many: Vec<String> = (0..=MAX_BLOCKED_BY)
            .map(|_| crate::ids::Id::new().to_string())
            .collect();
        bad(EntityKind::Task, "blocked_by", json!(many));
        ok(
            EntityKind::Task,
            "phoenix",
            json!({"role": "flamme", "fire": "2026-W45"}),
        );
        ok(EntityKind::Task, "phoenix", json!(null));
        bad(
            EntityKind::Task,
            "phoenix",
            json!({"role": "glut", "fire": "2026-W45"}),
        );
        bad(
            EntityKind::Task,
            "phoenix",
            json!({"role": "flamme", "fire": "2026-45"}),
        );
        bad(EntityKind::Task, "phoenix", json!({"role": "flamme"}));
    }

    #[test]
    fn repo_and_urls() {
        ok(EntityKind::Project, "repo", json!("rubeen/zunder"));
        ok(EntityKind::Project, "repo", json!("some-org/my.repo_name"));
        bad(EntityKind::Project, "repo", json!("zunder"));
        bad(EntityKind::Project, "repo", json!("a/b/c"));
        bad(EntityKind::Project, "repo", json!("-a/b"));
        bad(EntityKind::Project, "repo", json!("https://github.com/a/b"));
        ok(
            EntityKind::Task,
            "pr_url",
            json!("https://github.com/owner/repo/pull/1"),
        );
        bad(
            EntityKind::Task,
            "pr_url",
            json!("github.com/owner/repo/pull/1"),
        );
        bad(EntityKind::Task, "pr_url", json!("ftp://example.org/x"));
    }

    #[test]
    fn deleted_and_text() {
        ok(EntityKind::Task, "deleted", json!(true));
        ok(EntityKind::Task, "deleted", json!(false));
        ok(EntityKind::Area, "deleted", json!(null));
        bad(EntityKind::Task, "deleted", json!("true"));
        ok(EntityKind::Task, "description", json!("# Markdown"));
        bad(EntityKind::Task, "description", json!(["x"]));
        ok(EntityKind::Task, "rank", json!("a0"));
        ok(EntityKind::Task, "clickup_id", json!("86abc"));
    }
}
