//! Proposals for unsorted tasks from a decision model (ZUN-52, contract
//! 4.5): weight, for whom and, for inbox tasks, the place (project or
//! area), asked in one request to `POST /v1/systemone` (TypeSafe Jev; other
//! providers serve the same contract). Nothing here changes a task: the UI
//! offers each proposal with a tap and sets the ones marked [`Suggestion::auto`]
//! (empty field, probability ≥ [`AUTO_APPLY_MIN_PROBABILITY`]) by itself.
//!
//! - Settings rows `jev.*` (web UI only, no environment variable): the API
//!   token, a pinned model version (aliases such as `jev-latest` shift
//!   answers without notice and are refused) and the private areas, whose
//!   names and projects are never sent and never proposed.
//! - Asked by the UI (`POST /api/tasks/{id}/suggestion`), which does so on
//!   its own when an open root task's weight, for whom or place is still
//!   open (ZUN-56). Never for tasks in private areas (also via their
//!   project) or in dev projects. Only the task's title, description
//!   (shortened) and type go out, for inbox tasks also the names of the
//!   active, non-private projects and areas as options.
//! - Every question has an "unklar"/"nichts" option, because a model with a
//!   fixed schema otherwise reads missing information as a no.
//! - Stored with the full distributions and the model version (`suggestions`
//!   table). Whether the owner followed a proposal is derived from the
//!   task's values ([`SuggestionStats`]), so nothing extra is clicked;
//!   values the UI set by itself are counted apart (`auto_set`,
//!   `auto_changed`) and left out of eval runs, so Jev never grades itself.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use url::Url;
use zunder_core::ids::Id;
use zunder_core::model::{AreaState, ForWhom, Location, ProjectStatus, Task, Weight};
use zunder_core::State;

use crate::error::{AppError, AppResult};

/// Key prefix of the Jev settings.
pub const JEV_PREFIX: &str = "jev.";
pub const JEV_TOKEN_KEY: &str = "jev.token";
pub const JEV_MODEL_KEY: &str = "jev.model";
/// JSON array of area IDs.
pub const JEV_PRIVATE_AREAS_KEY: &str = "jev.private_areas";
/// Pinned default version.
pub const DEFAULT_MODEL: &str = "jev-1.14.0";
/// Endpoint of TypeSafe; tests point [`crate::config::Config::jev_base_url`] elsewhere.
pub const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai/v1/systemone";
/// Aliases that move without a code change.
const ALIASES: &[&str] = &["jev-latest", "jev-preview"];
const MAX_TOKEN_CHARS: usize = 2048;
const MAX_MODEL_CHARS: usize = 64;
/// Characters of the description that are sent.
const MAX_DESCRIPTION_CHARS: usize = 2000;
/// Choice questions allow at most 255 options; one is "nichts".
const MAX_PLACES: usize = 254;
const UNCLEAR: &str = "unklar";
const NOTHING: &str = "nichts";
const TOKEN_HINT_CHARS: usize = 4;
/// From this probability on, a proposal for an empty weight or for-whom is
/// set without a tap (marked in [`Suggestion::auto`]). Deliberately high
/// until the pilot's follow rate (ZUN-55) says otherwise; the place is
/// never set automatically.
pub const AUTO_APPLY_MIN_PROBABILITY: f64 = 0.9;
/// [`Suggestion::auto`] entries.
pub const AUTO_WEIGHT: &str = "weight";
pub const AUTO_FOR_WHOM: &str = "for_whom";

/// Jev settings stored in the database.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JevStored {
    pub token: Option<String>,
    pub model: Option<String>,
    pub private_areas: Vec<Id>,
}

impl JevStored {
    pub fn from_rows(rows: &BTreeMap<String, String>) -> Self {
        Self {
            token: rows.get(JEV_TOKEN_KEY).filter(|t| !t.is_empty()).cloned(),
            model: rows.get(JEV_MODEL_KEY).filter(|m| !m.is_empty()).cloned(),
            private_areas: rows
                .get(JEV_PRIVATE_AREAS_KEY)
                .and_then(|v| serde_json::from_str::<Vec<String>>(v).ok())
                .unwrap_or_default()
                .iter()
                .filter_map(|s| Id::parse(s))
                .collect(),
        }
    }

    /// The model in effect.
    pub fn model(&self) -> &str {
        self.model.as_deref().unwrap_or(DEFAULT_MODEL)
    }
}

/// Probability thresholds of the follow rate: "would a proposal only from
/// p ≥ x have been better?" (ZUN-53). The first row is every proposal.
pub const THRESHOLDS: [f64; 6] = [0.0, 0.5, 0.6, 0.7, 0.8, 0.9];

/// Proposals at or above one threshold.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct ThresholdTally {
    pub min_probability: f64,
    pub decided: u64,
    pub followed: u64,
}

/// How often a proposal was followed, per question.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Tally {
    /// Proposals the owner has decided on since (the task has a value now).
    pub decided: u64,
    /// Of those, how many equal the proposal.
    pub followed: u64,
    /// The same, counted only for proposals with at least the threshold's
    /// probability; one row per [`THRESHOLDS`] entry.
    pub by_threshold: Vec<ThresholdTally>,
    /// Proposals the UI set by itself (not in `decided`).
    pub auto_set: u64,
    /// Of those, how many the owner has changed since.
    pub auto_changed: u64,
}

impl Default for Tally {
    fn default() -> Self {
        Self {
            decided: 0,
            followed: 0,
            by_threshold: THRESHOLDS
                .iter()
                .map(|&min_probability| ThresholdTally {
                    min_probability,
                    ..ThresholdTally::default()
                })
                .collect(),
            auto_set: 0,
            auto_changed: 0,
        }
    }
}

impl Tally {
    /// Counts one decided proposal with its probability (`None` counts only
    /// in the first row).
    pub fn add(&mut self, probability: Option<f64>, followed: bool) {
        self.decided += 1;
        self.followed += u64::from(followed);
        let p = probability.unwrap_or(0.0);
        for row in &mut self.by_threshold {
            if p >= row.min_probability {
                row.decided += 1;
                row.followed += u64::from(followed);
            }
        }
    }

    /// Counts one proposal the UI set by itself; `changed`: the task's
    /// value differs from it now (also when it was cleared).
    pub fn add_auto(&mut self, changed: bool) {
        self.auto_set += 1;
        self.auto_changed += u64::from(changed);
    }
}

/// Follow rate of the stored proposals (`GET /api/settings/jev`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SuggestionStats {
    /// Tasks with a stored proposal.
    pub asked: u64,
    pub weight: Tally,
    pub for_whom: Tally,
    pub place: Tally,
    pub input_tokens: u64,
}

/// `GET /api/jev`: what every UI (browser and app) needs to know.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct JevStatus {
    pub configured: bool,
    /// [`AUTO_APPLY_MIN_PROBABILITY`], for the UI's wording.
    pub auto_apply_min_probability: f64,
}

/// `GET /api/settings/jev`. Never contains the token.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JevSettings {
    pub configured: bool,
    pub token_hint: Option<String>,
    pub model: String,
    pub default_model: &'static str,
    pub private_area_ids: Vec<Id>,
    pub stats: SuggestionStats,
}

impl JevSettings {
    pub fn build(stored: &JevStored, stats: SuggestionStats) -> Self {
        Self {
            configured: stored.token.is_some(),
            token_hint: stored.token.as_deref().map(token_hint),
            model: stored.model().to_owned(),
            default_model: DEFAULT_MODEL,
            private_area_ids: stored.private_areas.clone(),
            stats,
        }
    }
}

fn token_hint(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() < TOKEN_HINT_CHARS * 3 {
        return "…".into();
    }
    let tail: String = chars[chars.len() - TOKEN_HINT_CHARS..].iter().collect();
    format!("…{tail}")
}

/// `PUT /api/settings/jev`: a missing or empty `token` keeps the stored one;
/// `model` empty means the default; `private_area_ids` replaces the list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JevUpdate {
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub private_area_ids: Option<Vec<String>>,
}

/// A validated [`JevUpdate`] as settings rows (`None` deletes the row).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JevRows(pub Vec<(&'static str, Option<String>)>);

impl JevUpdate {
    pub fn validated(self) -> AppResult<JevRows> {
        let mut rows = Vec::new();
        match self.token.as_deref().map(str::trim) {
            None | Some("") => {}
            Some(t) if t.chars().count() > MAX_TOKEN_CHARS => {
                return Err(AppError::bad_request(format!(
                    "Der Jev-Key darf höchstens {MAX_TOKEN_CHARS} Zeichen lang sein"
                )))
            }
            Some(t) if t.chars().any(|c| c.is_whitespace() || c.is_control()) => {
                return Err(AppError::bad_request(
                    "Der Jev-Key darf keine Leer- oder Steuerzeichen enthalten",
                ))
            }
            Some(t) => rows.push((JEV_TOKEN_KEY, Some(t.to_owned()))),
        }
        match self.model.as_deref().map(str::trim) {
            None => {}
            Some("") => rows.push((JEV_MODEL_KEY, None)),
            Some(m)
                if m.len() > MAX_MODEL_CHARS
                    || !m
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')) =>
            {
                return Err(AppError::bad_request(
                    "Die Jev-Version: Buchstaben, Ziffern, Punkt und Bindestrich, z. B. jev-1.14.0",
                ))
            }
            Some(m) if ALIASES.contains(&m.to_ascii_lowercase().as_str()) => {
                return Err(AppError::bad_request(format!(
                    "Bitte eine feste Version wie {DEFAULT_MODEL} statt {m}: Aliase verschieben Antworten ohne Ankündigung"
                )))
            }
            Some(m) => rows.push((JEV_MODEL_KEY, Some(m.to_owned()))),
        }
        if let Some(ids) = self.private_area_ids {
            let mut parsed = Vec::new();
            for raw in &ids {
                let id = Id::parse(raw.trim()).ok_or_else(|| {
                    AppError::bad_request(format!("Unbekannte Bereichs-ID „{raw}“"))
                })?;
                if !parsed.contains(&id) {
                    parsed.push(id);
                }
            }
            let list: Vec<String> = parsed.iter().map(ToString::to_string).collect();
            rows.push((
                JEV_PRIVATE_AREAS_KEY,
                Some(serde_json::to_string(&list).expect("strings serialize")),
            ));
        }
        Ok(JevRows(rows))
    }
}

/// A proposed place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaceProposal {
    /// `area` or `project`.
    pub kind: String,
    pub id: Id,
    pub name: String,
}

impl PlaceProposal {
    fn location(&self) -> Location {
        if self.kind == "project" {
            Location::Project { id: self.id }
        } else {
            Location::Area { id: self.id }
        }
    }
}

/// A stored proposal (`GET`/`POST /api/tasks/{id}/suggestion`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Suggestion {
    pub task_id: Id,
    pub model: String,
    /// RFC 3339.
    pub created_at: String,
    /// `None`: the model found it unclear.
    pub weight: Option<Weight>,
    pub weight_probabilities: BTreeMap<String, f64>,
    pub for_whom: Option<ForWhom>,
    pub for_whom_probabilities: BTreeMap<String, f64>,
    /// `None`: nothing fits.
    pub place: Option<PlaceProposal>,
    /// By option key (an ID or `nichts`).
    pub place_probabilities: BTreeMap<String, f64>,
    /// Probability of each proposal, for the UI.
    pub weight_probability: Option<f64>,
    pub for_whom_probability: Option<f64>,
    pub place_probability: Option<f64>,
    pub input_tokens: u64,
    /// Proposals the UI sets without a tap ([`AUTO_WEIGHT`],
    /// [`AUTO_FOR_WHOM`]): the field was empty when asked and the
    /// probability is at least [`AUTO_APPLY_MIN_PROBABILITY`].
    #[serde(default)]
    pub auto: Vec<String>,
}

impl Suggestion {
    /// Marks the proposals for fields that are still empty on the task
    /// (its current `weight` and `for_whom`) and sure enough to be set
    /// without a tap.
    pub fn mark_auto(&mut self, weight: Option<Weight>, for_whom: Option<ForWhom>) {
        let sure = |p: Option<f64>| p.is_some_and(|p| p >= AUTO_APPLY_MIN_PROBABILITY);
        self.auto.clear();
        if weight.is_none() && self.weight.is_some() && sure(self.weight_probability) {
            self.auto.push(AUTO_WEIGHT.into());
        }
        if for_whom.is_none() && self.for_whom.is_some() && sure(self.for_whom_probability) {
            self.auto.push(AUTO_FOR_WHOM.into());
        }
    }

    /// The weight the UI set by itself, if any.
    pub fn auto_weight(&self) -> Option<Weight> {
        self.weight
            .filter(|_| self.auto.iter().any(|a| a == AUTO_WEIGHT))
    }

    /// The for-whom the UI set by itself, if any.
    pub fn auto_for_whom(&self) -> Option<ForWhom> {
        self.for_whom
            .filter(|_| self.auto.iter().any(|a| a == AUTO_FOR_WHOM))
    }
}

/// One option of the place question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceOption {
    pub proposal: PlaceProposal,
    pub description: String,
}

/// Active, non-private projects and areas as options, projects first.
pub fn place_options(state: &State, private: &[Id]) -> Vec<PlaceOption> {
    let areas: BTreeMap<Id, String> = state
        .areas()
        .filter(|a| !a.deleted && a.state == AreaState::Active && !private.contains(&a.id))
        .map(|a| (a.id, a.name))
        .collect();
    let mut out: Vec<PlaceOption> = state
        .projects()
        .filter(|p| !p.deleted && p.status == ProjectStatus::Active)
        .filter_map(|p| {
            let area = areas.get(&p.area_id)?;
            Some(PlaceOption {
                description: format!("Projekt „{}“ im Bereich „{area}“", p.name),
                proposal: PlaceProposal {
                    kind: "project".into(),
                    id: p.id,
                    name: p.name,
                },
            })
        })
        .collect();
    out.extend(areas.iter().map(|(id, name)| PlaceOption {
        description: format!("Bereich „{name}“ direkt, ohne Projekt"),
        proposal: PlaceProposal {
            kind: "area".into(),
            id: *id,
            name: name.clone(),
        },
    }));
    out.truncate(MAX_PLACES);
    out
}

/// The request body for `POST /v1/systemone`.
/// The request for `task`; `places: None` leaves the place question out
/// (tasks that already have a place).
pub fn request_body(model: &str, task: &Task, places: Option<&[PlaceOption]>) -> Value {
    request_body_for(
        model,
        &task.title,
        task.description.as_deref(),
        task.task_type.as_str(),
        places,
    )
}

fn request_body_for(
    model: &str,
    title: &str,
    description: Option<&str>,
    task_type: &str,
    places: Option<&[PlaceOption]>,
) -> Value {
    let description: Option<String> =
        description.map(|d| d.chars().take(MAX_DESCRIPTION_CHARS).collect());
    let mut body = json!({
        "state": {
            "titel": title,
            "beschreibung": description,
            "typ": task_type,
        },
        "model": model,
        "questions": {
            "gewicht": {
                "type": "choice",
                "instructions": "Eine neue Aufgabe einer einzelnen Person, eingeteilt nach Covey: Was ist sie für diese Person? Wichtig heißt: dient ihren eigenen Zielen (Selbstverwirklichung, Partnerschaft, Finanzen, Gesundheit vor Arbeit). Dringlich nur fürs System heißt: andere oder ein Arbeitgeber machen Druck, für sie selbst zählt es wenig.",
                "criteria": {
                    "brand": "Wichtig und dringlich: heute tun",
                    "glut": "Wichtig, nicht dringlich: einplanen, darf rutschen, nicht verlieren",
                    "rauch": "Dringlich nur fürs System oder für andere, für sie selbst nicht wichtig",
                    "asche": "Weder wichtig noch dringlich: darf liegen bleiben",
                    UNCLEAR: "Aus der Aufgabe nicht zu entscheiden",
                },
            },
            "fuer_wen": {
                "type": "choice",
                "instructions": "Wer wartet, wenn sich die Aufgabe verschiebt? Nicht: wem sie nützt.",
                "criteria": {
                    "mich": "Niemand: nur die Person selbst",
                    "uns": "Partnerschaft oder Haushalt",
                    "andere": "Jemand hat eine Zusage und wartet darauf",
                    UNCLEAR: "Aus der Aufgabe nicht zu erkennen",
                },
            },
        },
    });
    if let Some(places) = places {
        let mut place_criteria = serde_json::Map::new();
        for p in places {
            place_criteria.insert(
                p.proposal.id.to_string(),
                Value::String(p.description.clone()),
            );
        }
        place_criteria.insert(
            NOTHING.into(),
            Value::String(
                "Nichts davon passt, oder es ist aus der Aufgabe nicht zu erkennen".into(),
            ),
        );
        body["questions"]["ort"] = json!({
            "type": "choice",
            "instructions": "Wohin gehört die Aufgabe?",
            "criteria": place_criteria,
        });
    }
    body
}

#[derive(Deserialize)]
struct Answer {
    #[serde(default)]
    choice: Option<String>,
    #[serde(default)]
    probabilities: BTreeMap<String, f64>,
}

#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    model: Option<String>,
    answers: BTreeMap<String, Answer>,
    #[serde(default)]
    usage: Usage,
}

#[derive(Deserialize, Default)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
}

/// The chosen option: `choice` if it is one of `keys`, else the most
/// probable key.
fn chosen(answer: &Answer, keys: &[String]) -> Option<String> {
    if let Some(c) = &answer.choice {
        if keys.contains(c) {
            return Some(c.clone());
        }
    }
    keys.iter()
        .filter_map(|k| answer.probabilities.get(k).map(|p| (k, *p)))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(k, _)| k.clone())
}

fn known(probabilities: &BTreeMap<String, f64>, keys: &[String]) -> BTreeMap<String, f64> {
    keys.iter()
        .map(|k| (k.clone(), probabilities.get(k).copied().unwrap_or(0.0)))
        .collect()
}

/// Reads the provider's answer into a [`Suggestion`].
pub fn parse_response(
    body: &[u8],
    task_id: Id,
    requested_model: &str,
    places: Option<&[PlaceOption]>,
    created_at: String,
) -> AppResult<Suggestion> {
    let r: Response = serde_json::from_slice(body)
        .map_err(|e| AppError::Unavailable(format!("Jev antwortet unverständlich: {e}")))?;
    let answer = |key: &str| {
        r.answers
            .get(key)
            .ok_or_else(|| AppError::Unavailable(format!("Jev antwortet ohne „{key}“")))
    };
    let codes = |all: Vec<&str>| -> Vec<String> {
        all.into_iter()
            .map(str::to_owned)
            .chain([UNCLEAR.to_owned()])
            .collect()
    };
    let weight_keys = codes(Weight::codes());
    let for_whom_keys = codes(ForWhom::codes());
    let w = answer("gewicht")?;
    let f = answer("fuer_wen")?;
    let weight_choice = chosen(w, &weight_keys);
    let for_whom_choice = chosen(f, &for_whom_keys);
    let weight_probabilities = known(&w.probabilities, &weight_keys);
    let for_whom_probabilities = known(&f.probabilities, &for_whom_keys);
    // Without the place question: no place, no distribution.
    let (place_choice, place_probabilities) = match places {
        Some(places) => {
            let mut place_keys: Vec<String> =
                places.iter().map(|p| p.proposal.id.to_string()).collect();
            place_keys.push(NOTHING.into());
            let o = answer("ort")?;
            (chosen(o, &place_keys), known(&o.probabilities, &place_keys))
        }
        None => (None, BTreeMap::new()),
    };
    let places = places.unwrap_or_default();
    let weight = weight_choice
        .as_deref()
        .and_then(|c| c.parse::<Weight>().ok());
    let for_whom = for_whom_choice
        .as_deref()
        .and_then(|c| c.parse::<ForWhom>().ok());
    let place = place_choice.as_deref().and_then(|c| {
        places
            .iter()
            .find(|p| p.proposal.id.to_string() == c)
            .map(|p| p.proposal.clone())
    });
    Ok(Suggestion {
        task_id,
        model: r.model.unwrap_or_else(|| requested_model.to_owned()),
        created_at,
        weight_probability: weight.map(|v| weight_probabilities[v.as_str()]),
        for_whom_probability: for_whom.map(|v| for_whom_probabilities[v.as_str()]),
        place_probability: place
            .as_ref()
            .map(|p| place_probabilities[&p.id.to_string()]),
        weight,
        weight_probabilities,
        for_whom,
        for_whom_probabilities,
        place,
        place_probabilities,
        input_tokens: r.usage.input_tokens,
        auto: Vec::new(),
    })
}

/// Sends the request. Errors become German messages (`Unavailable`).
pub async fn ask(
    http: &reqwest::Client,
    endpoint: &Url,
    token: &str,
    body: &Value,
) -> AppResult<Vec<u8>> {
    let response = http
        .post(endpoint.clone())
        .bearer_auth(token)
        .header(reqwest::header::ACCEPT, "application/json")
        .json(body)
        .send()
        .await
        .map_err(|e| AppError::Unavailable(format!("Jev ist nicht erreichbar: {e}")))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|e| AppError::Unavailable(format!("Jev-Antwort abgebrochen: {e}")))?;
    if status.is_success() {
        return Ok(bytes.to_vec());
    }
    Err(AppError::Unavailable(match status.as_u16() {
        401 | 403 => "Jev lehnt den Key ab".to_owned(),
        429 => "Jev ist gerade ausgelastet, bitte später noch einmal".to_owned(),
        code => format!(
            "Jev antwortet {code}: {}",
            String::from_utf8_lossy(&bytes)
                .chars()
                .take(200)
                .collect::<String>()
        ),
    }))
}

/// Follow rate: a proposal counts once the task has a value for it (a
/// weight, a for-whom, a place outside the inbox); "unklar" and "nichts"
/// do not count.
pub fn stats(state: &State, suggestions: &[Suggestion]) -> SuggestionStats {
    let mut s = SuggestionStats::default();
    for sug in suggestions {
        s.asked += 1;
        s.input_tokens += sug.input_tokens;
        let Some(task) = state.task(sug.task_id) else {
            continue;
        };
        if let Some(p) = sug.auto_weight() {
            s.weight.add_auto(task.weight != Some(p));
        } else if let (Some(p), Some(v)) = (sug.weight, task.weight) {
            s.weight.add(sug.weight_probability, p == v);
        }
        if let Some(p) = sug.auto_for_whom() {
            s.for_whom.add_auto(task.for_whom != Some(p));
        } else if let (Some(p), Some(v)) = (sug.for_whom, task.for_whom) {
            s.for_whom.add(sug.for_whom_probability, p == v);
        }
        let location = state.effective_location(task.id).unwrap_or(task.location);
        if let Some(p) = &sug.place {
            if location != Location::Inbox {
                s.place.add(sug.place_probability, p.location() == location);
            }
        }
    }
    s
}

// ----- eval run (ZUN-53) -------------------------------------------------------------

/// Tasks per eval run without `limit`.
pub const EVAL_DEFAULT_LIMIT: usize = 25;
/// Upper bound of `limit`: every case costs tokens at TypeSafe.
pub const EVAL_MAX_LIMIT: usize = 100;

/// `POST /api/settings/jev/eval`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalRequest {
    #[serde(default)]
    pub limit: Option<usize>,
}

impl EvalRequest {
    pub fn limit(&self) -> AppResult<usize> {
        match self.limit {
            None => Ok(EVAL_DEFAULT_LIMIT),
            Some(n) if (1..=EVAL_MAX_LIMIT).contains(&n) => Ok(n),
            Some(_) => Err(AppError::bad_request(format!(
                "Ein Probelauf umfasst 1 bis {EVAL_MAX_LIMIT} Aufgaben"
            ))),
        }
    }
}

/// One already sorted task with its known answers.
#[derive(Debug, Clone, PartialEq)]
pub struct EvalCase {
    pub task_id: Id,
    pub title: String,
    pub weight: Option<Weight>,
    pub for_whom: Option<ForWhom>,
    /// The task's place, if it is one of the options.
    pub place: Option<PlaceProposal>,
    pub body: Value,
}

/// The eval set: live root tasks outside the inbox with at least one known
/// answer (weight, for whom, or a place among the options), most known
/// answers first, then most recently changed. Tasks in private areas (also
/// via their project) never go out. A weight or for-whom the UI set from a
/// proposal by itself (`auto`, by task) and still unchanged is no known
/// answer: Jev would grade itself.
pub fn eval_cases(
    state: &State,
    model: &str,
    private: &[Id],
    places: &[PlaceOption],
    auto: &BTreeMap<Id, (Option<Weight>, Option<ForWhom>)>,
    limit: usize,
) -> Vec<EvalCase> {
    // Request bodies (with up to 254 places each) only for the cases kept.
    let mut found: Vec<(usize, zunder_core::Hlc, Task, Option<PlaceProposal>)> = state
        .live_tasks()
        .filter(|t| t.parent_id.is_none())
        .filter_map(|mut t| {
            let location = state.effective_location(t.id)?;
            let area = match location {
                Location::Inbox => return None,
                Location::Loose => None,
                Location::Area { id } => Some(id),
                Location::Project { id } => Some(state.project(id)?.area_id),
            };
            if area.is_some_and(|a| private.contains(&a)) {
                return None;
            }
            if let Some((weight, for_whom)) = auto.get(&t.id) {
                if weight.is_some() && t.weight == *weight {
                    t.weight = None;
                }
                if for_whom.is_some() && t.for_whom == *for_whom {
                    t.for_whom = None;
                }
            }
            let place = places
                .iter()
                .find(|p| p.proposal.location() == location)
                .map(|p| p.proposal.clone());
            let known = usize::from(t.weight.is_some())
                + usize::from(t.for_whom.is_some())
                + usize::from(place.is_some());
            if known == 0 {
                return None;
            }
            let updated = state
                .updated_at(zunder_core::EntityKind::Task, t.id)?
                .clone();
            Some((known, updated, t, place))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    found
        .into_iter()
        .take(limit)
        .map(|(_, _, t, place)| EvalCase {
            task_id: t.id,
            body: request_body(model, &t, Some(places)),
            title: t.title,
            weight: t.weight,
            for_whom: t.for_whom,
            place,
        })
        .collect()
}

/// One question of the eval run.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct EvalQuestion {
    /// Answered cases whose answer is known.
    pub known: u64,
    /// Of those, how often the model answered "unklar"/"nichts".
    pub unclear: u64,
    /// Proposals (`decided`) and hits (`followed`), also per threshold.
    pub hits: Tally,
}

/// Expected and proposed answer of one question (codes, place names).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct EvalAnswer {
    pub expected: Option<String>,
    pub proposed: Option<String>,
    pub probability: Option<f64>,
}

/// One answered case.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvalRow {
    pub task_id: Id,
    pub title: String,
    pub weight: EvalAnswer,
    pub for_whom: EvalAnswer,
    pub place: EvalAnswer,
}

/// Result of `POST /api/settings/jev/eval`. Nothing is stored.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct JevEval {
    pub model: String,
    /// Cases sent.
    pub asked: u64,
    /// Of those, without a usable answer.
    pub failed: u64,
    /// Distinct error messages of the failed cases (at most 5).
    pub errors: Vec<String>,
    pub input_tokens: u64,
    pub weight: EvalQuestion,
    pub for_whom: EvalQuestion,
    pub place: EvalQuestion,
    /// Answered cases in eval-set order.
    pub cases: Vec<EvalRow>,
}

fn score<T: PartialEq>(
    q: &mut EvalQuestion,
    expected: Option<T>,
    proposed: Option<T>,
    probability: Option<f64>,
) {
    let Some(expected) = expected else { return };
    q.known += 1;
    match proposed {
        None => q.unclear += 1,
        Some(p) => q.hits.add(probability, p == expected),
    }
}

impl JevEval {
    pub fn new(model: &str) -> Self {
        Self {
            model: model.to_owned(),
            ..Self::default()
        }
    }

    /// Counts one case: its answer or its error.
    pub fn record(&mut self, case: &EvalCase, answer: Result<Suggestion, String>) {
        self.asked += 1;
        let s = match answer {
            Ok(s) => s,
            Err(e) => {
                self.failed += 1;
                if self.errors.len() < 5 && !self.errors.contains(&e) {
                    self.errors.push(e);
                }
                return;
            }
        };
        self.model.clone_from(&s.model);
        self.input_tokens += s.input_tokens;
        score(
            &mut self.weight,
            case.weight,
            s.weight,
            s.weight_probability,
        );
        score(
            &mut self.for_whom,
            case.for_whom,
            s.for_whom,
            s.for_whom_probability,
        );
        score(
            &mut self.place,
            case.place.as_ref().map(|p| p.id),
            s.place.as_ref().map(|p| p.id),
            s.place_probability,
        );
        let code = |v: Option<&str>| v.map(str::to_owned);
        self.cases.push(EvalRow {
            task_id: case.task_id,
            title: case.title.clone(),
            weight: EvalAnswer {
                expected: code(case.weight.map(Weight::as_str)),
                proposed: code(s.weight.map(Weight::as_str)),
                probability: s.weight_probability,
            },
            for_whom: EvalAnswer {
                expected: code(case.for_whom.map(ForWhom::as_str)),
                proposed: code(s.for_whom.map(ForWhom::as_str)),
                probability: s.for_whom_probability,
            },
            place: EvalAnswer {
                expected: case.place.as_ref().map(|p| p.name.clone()),
                proposed: s.place.as_ref().map(|p| p.name.clone()),
                probability: s.place_probability,
            },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn stored_defaults_and_private_areas() {
        let s = JevStored::from_rows(&BTreeMap::new());
        assert_eq!(s.token, None);
        assert_eq!(s.model(), DEFAULT_MODEL);
        let id = Id::new();
        let s = JevStored::from_rows(&rows(&[
            (JEV_TOKEN_KEY, "tsk_0123456789"),
            (JEV_MODEL_KEY, "jev-1.13.0"),
            (JEV_PRIVATE_AREAS_KEY, &format!("[\"{id}\", \"kaputt\"]")),
        ]));
        assert_eq!(s.model(), "jev-1.13.0");
        assert_eq!(s.private_areas, vec![id]);
        let settings = JevSettings::build(&s, SuggestionStats::default());
        assert!(settings.configured);
        assert_eq!(settings.token_hint.as_deref(), Some("…6789"));
    }

    #[test]
    fn update_refuses_aliases_and_bad_ids() {
        for model in ["jev-latest", "JEV-Preview"] {
            assert!(JevUpdate {
                model: Some(model.into()),
                ..Default::default()
            }
            .validated()
            .is_err());
        }
        assert!(JevUpdate {
            model: Some("jev 1".into()),
            ..Default::default()
        }
        .validated()
        .is_err());
        assert!(JevUpdate {
            token: Some("a b".into()),
            ..Default::default()
        }
        .validated()
        .is_err());
        assert!(JevUpdate {
            private_area_ids: Some(vec!["nope".into()]),
            ..Default::default()
        }
        .validated()
        .is_err());
        let id = Id::new();
        let r = JevUpdate {
            token: Some("  ".into()),
            model: Some(String::new()),
            private_area_ids: Some(vec![id.to_string(), id.to_string()]),
        }
        .validated()
        .unwrap();
        assert_eq!(
            r.0,
            vec![
                (JEV_MODEL_KEY, None),
                (JEV_PRIVATE_AREAS_KEY, Some(format!("[\"{id}\"]")))
            ]
        );
    }

    fn place(kind: &str, name: &str) -> PlaceOption {
        PlaceOption {
            proposal: PlaceProposal {
                kind: kind.into(),
                id: Id::new(),
                name: name.into(),
            },
            description: name.into(),
        }
    }

    #[test]
    fn parses_choices_with_unclear_and_fallback_to_the_maximum() {
        let places = vec![place("project", "Zunder"), place("area", "Finanzen")];
        let pid = places[0].proposal.id.to_string();
        let body = json!({
            "model": "jev-1.14.0",
            "answers": {
                "gewicht": {"type": "choice", "choice": "glut", "probabilities": {"brand": 0.1, "glut": 0.7, "rauch": 0.1, "asche": 0.05, "unklar": 0.05}, "confidence": 0.6},
                "fuer_wen": {"type": "choice", "choice": "unklar", "probabilities": {"mich": 0.2, "uns": 0.1, "andere": 0.1, "unklar": 0.6}},
                "ort": {"type": "choice", "probabilities": {pid.clone(): 0.8, "nichts": 0.2}}
            },
            "usage": {"input_tokens": 512, "output_tokens": 9}
        });
        let id = Id::new();
        let s = parse_response(
            body.to_string().as_bytes(),
            id,
            "x",
            Some(&places),
            "t".into(),
        )
        .unwrap();
        assert_eq!(s.weight, Some(Weight::Glut));
        assert_eq!(s.weight_probability, Some(0.7));
        assert_eq!(s.for_whom, None, "unklar is no proposal");
        assert_eq!(s.place.as_ref().map(|p| p.name.as_str()), Some("Zunder"));
        assert_eq!(s.place_probability, Some(0.8));
        assert_eq!(s.place_probabilities.len(), 3);
        assert_eq!(s.model, "jev-1.14.0");
        assert_eq!(s.input_tokens, 512);

        assert!(s.auto.is_empty());

        let missing = json!({"answers": {"gewicht": {"choice": "glut"}}});
        assert!(parse_response(
            missing.to_string().as_bytes(),
            id,
            "x",
            Some(&places),
            "t".into()
        )
        .is_err());

        // Without the place question no "ort" is needed and no place comes back.
        let mut no_place = body.clone();
        no_place["answers"].as_object_mut().unwrap().remove("ort");
        let s = parse_response(no_place.to_string().as_bytes(), id, "x", None, "t".into()).unwrap();
        assert_eq!(s.weight, Some(Weight::Glut));
        assert_eq!(s.place, None);
        assert!(s.place_probabilities.is_empty());
    }

    fn suggestion(weight: Option<(Weight, f64)>, for_whom: Option<(ForWhom, f64)>) -> Suggestion {
        Suggestion {
            task_id: Id::new(),
            model: "jev-1.14.0".into(),
            created_at: "t".into(),
            weight: weight.map(|w| w.0),
            weight_probabilities: BTreeMap::new(),
            for_whom: for_whom.map(|f| f.0),
            for_whom_probabilities: BTreeMap::new(),
            place: None,
            place_probabilities: BTreeMap::new(),
            weight_probability: weight.map(|w| w.1),
            for_whom_probability: for_whom.map(|f| f.1),
            place_probability: None,
            input_tokens: 0,
            auto: Vec::new(),
        }
    }

    #[test]
    fn only_sure_proposals_for_empty_fields_are_set_by_themselves() {
        let mut s = suggestion(
            Some((Weight::Glut, AUTO_APPLY_MIN_PROBABILITY)),
            Some((ForWhom::Mich, 0.89)),
        );
        s.mark_auto(None, None);
        assert_eq!(s.auto, vec![AUTO_WEIGHT.to_owned()]);
        assert_eq!(s.auto_weight(), Some(Weight::Glut));
        assert_eq!(s.auto_for_whom(), None);

        // A field that is already set is never overwritten.
        let mut s = suggestion(Some((Weight::Glut, 0.99)), Some((ForWhom::Mich, 0.95)));
        s.mark_auto(Some(Weight::Brand), None);
        assert_eq!(s.auto, vec![AUTO_FOR_WHOM.to_owned()]);

        // Stored proposals from before ZUN-56 read without `auto`.
        let mut old = serde_json::to_value(suggestion(Some((Weight::Glut, 0.99)), None)).unwrap();
        old.as_object_mut().unwrap().remove("auto");
        let old: Suggestion = serde_json::from_value(old).unwrap();
        assert!(old.auto.is_empty());
    }

    #[test]
    fn request_contains_only_title_description_type_and_places() {
        let places = vec![place("project", "Zunder")];
        let long = "x".repeat(5000);
        let body = request_body_for(
            "jev-1.14.0",
            "Steuer",
            Some(&long),
            "aufgabe",
            Some(&places),
        );
        let state = body["state"].as_object().unwrap();
        assert_eq!(
            state.keys().collect::<Vec<_>>(),
            vec!["beschreibung", "titel", "typ"]
        );
        assert_eq!(
            state["beschreibung"].as_str().unwrap().chars().count(),
            MAX_DESCRIPTION_CHARS
        );
        assert_eq!(body["model"], "jev-1.14.0");
        let ort = body["questions"]["ort"]["criteria"].as_object().unwrap();
        assert_eq!(ort.len(), 2);
        assert!(ort.contains_key(NOTHING));
        assert!(body["questions"]["gewicht"]["criteria"]
            .get(UNCLEAR)
            .is_some());
        assert!(body["questions"]["fuer_wen"]["criteria"]
            .get(UNCLEAR)
            .is_some());

        let body = request_body_for("jev-1.14.0", "Steuer", None, "aufgabe", None);
        assert!(body["questions"].get("ort").is_none());
        assert!(body["questions"].get("gewicht").is_some());
    }
}
