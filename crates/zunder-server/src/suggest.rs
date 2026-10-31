//! Proposals for inbox tasks from a decision model (ZUN-52, contract 4.5):
//! weight, for whom and place (project or area), asked in one request to
//! `POST /v1/systemone` (TypeSafe Jev; other providers serve the same
//! contract). Only proposals: nothing here changes a task, the web UI offers
//! each one to take over with a click.
//!
//! - Settings rows `jev.*` (web UI only, no environment variable): the API
//!   token, a pinned model version (aliases such as `jev-latest` shift
//!   answers without notice and are refused) and the private areas, whose
//!   names and projects are never sent and never proposed.
//! - Asked on request only (`POST /api/tasks/{id}/suggestion`), for tasks in
//!   the inbox: the owner decides per task what leaves the server. Only the
//!   task's title, description (shortened) and type go out, plus the names
//!   of the active, non-private projects and areas as options.
//! - Every question has an "unklar"/"nichts" option, because a model with a
//!   fixed schema otherwise reads missing information as a no.
//! - Stored with the full distributions and the model version (`suggestions`
//!   table). Whether the owner followed a proposal is derived from the
//!   task's values ([`SuggestionStats`]), so nothing extra is clicked.

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

/// How often a proposal was followed, per question.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Tally {
    /// Proposals the owner has decided on since (the task has a value now).
    pub decided: u64,
    /// Of those, how many equal the proposal.
    pub followed: u64,
}

/// Follow rate of the stored proposals (`GET /api/settings/jev`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct SuggestionStats {
    /// Tasks with a stored proposal.
    pub asked: u64,
    pub weight: Tally,
    pub for_whom: Tally,
    pub place: Tally,
    pub input_tokens: u64,
}

/// `GET /api/settings/jev`. Never contains the token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
pub fn request_body(model: &str, task: &Task, places: &[PlaceOption]) -> Value {
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
    places: &[PlaceOption],
) -> Value {
    let description: Option<String> =
        description.map(|d| d.chars().take(MAX_DESCRIPTION_CHARS).collect());
    let mut place_criteria = serde_json::Map::new();
    for p in places {
        place_criteria.insert(
            p.proposal.id.to_string(),
            Value::String(p.description.clone()),
        );
    }
    place_criteria.insert(
        NOTHING.into(),
        Value::String("Nichts davon passt, oder es ist aus der Aufgabe nicht zu erkennen".into()),
    );
    json!({
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
            "ort": {
                "type": "choice",
                "instructions": "Wohin gehört die Aufgabe?",
                "criteria": place_criteria,
            },
        },
    })
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
    places: &[PlaceOption],
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
    let mut place_keys: Vec<String> = places.iter().map(|p| p.proposal.id.to_string()).collect();
    place_keys.push(NOTHING.into());

    let w = answer("gewicht")?;
    let f = answer("fuer_wen")?;
    let o = answer("ort")?;
    let weight_choice = chosen(w, &weight_keys);
    let for_whom_choice = chosen(f, &for_whom_keys);
    let place_choice = chosen(o, &place_keys);
    let weight_probabilities = known(&w.probabilities, &weight_keys);
    let for_whom_probabilities = known(&f.probabilities, &for_whom_keys);
    let place_probabilities = known(&o.probabilities, &place_keys);
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
        if let (Some(p), Some(v)) = (sug.weight, task.weight) {
            s.weight.decided += 1;
            s.weight.followed += u64::from(p == v);
        }
        if let (Some(p), Some(v)) = (sug.for_whom, task.for_whom) {
            s.for_whom.decided += 1;
            s.for_whom.followed += u64::from(p == v);
        }
        let location = state.effective_location(task.id).unwrap_or(task.location);
        if let Some(p) = &sug.place {
            if location != Location::Inbox {
                s.place.decided += 1;
                s.place.followed += u64::from(p.location() == location);
            }
        }
    }
    s
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
        let s = parse_response(body.to_string().as_bytes(), id, "x", &places, "t".into()).unwrap();
        assert_eq!(s.weight, Some(Weight::Glut));
        assert_eq!(s.weight_probability, Some(0.7));
        assert_eq!(s.for_whom, None, "unklar is no proposal");
        assert_eq!(s.place.as_ref().map(|p| p.name.as_str()), Some("Zunder"));
        assert_eq!(s.place_probability, Some(0.8));
        assert_eq!(s.place_probabilities.len(), 3);
        assert_eq!(s.model, "jev-1.14.0");
        assert_eq!(s.input_tokens, 512);

        let missing = json!({"answers": {"gewicht": {"choice": "glut"}}});
        assert!(
            parse_response(missing.to_string().as_bytes(), id, "x", &places, "t".into()).is_err()
        );
    }

    #[test]
    fn request_contains_only_title_description_type_and_places() {
        let places = vec![place("project", "Zunder")];
        let long = "x".repeat(5000);
        let body = request_body_for("jev-1.14.0", "Steuer", Some(&long), "aufgabe", &places);
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
    }
}
