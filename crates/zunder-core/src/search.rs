//! Simple full-text search over tasks, projects and areas.
//!
//! Matching is case-insensitive and tolerant towards diacritics: `ä` matches
//! both `a` and `ae` (likewise `ö`, `ü`), `ß` matches `ss`, and common accents
//! are stripped. All query words must occur (as substrings) in the searched
//! fields of a hit. Tasks are searched in title, description and plan,
//! projects and areas by name. Closed tasks are included; tombstoned
//! entities are not.

use crate::ids::Id;
use serde::{Deserialize, Serialize};

use crate::model::fields;
use crate::op::EntityKind;
use crate::state::{Entity, State};

/// One search hit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub entity: EntityKind,
    pub entity_id: Id,
    /// Title of a task or name of a project/area.
    pub title: String,
    /// Higher is better.
    pub score: u32,
}

const WEIGHT_TITLE: u32 = 10;
const WEIGHT_DESCRIPTION: u32 = 3;
const WEIGHT_PLAN: u32 = 2;
const BONUS_WORD_START: u32 = 4;
const BONUS_EXACT_TITLE: u32 = 50;
/// Exact ClickUp ID or task reference (`ZUN-42`, any case).
const SCORE_CLICKUP_ID: u32 = 1000;

/// Searches all live entities. Results are ordered by score (descending),
/// then by kind and title. `limit = 0` means no limit.
pub fn search(state: &State, query: &str, limit: usize) -> Vec<SearchHit> {
    let terms: Vec<Folded> = query.split_whitespace().map(Folded::new).collect();
    if terms.is_empty() {
        return Vec::new();
    }
    let exact_query = Folded::new(query.trim());
    let mut hits: Vec<SearchHit> = state
        .entities()
        .filter(|e| !e.is_deleted())
        .filter_map(|e| score_entity(e, query.trim(), &terms, &exact_query))
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.entity.cmp(&b.entity))
            .then_with(|| a.title.cmp(&b.title))
            .then_with(|| a.entity_id.cmp(&b.entity_id))
    });
    if limit > 0 {
        hits.truncate(limit);
    }
    hits
}

fn score_entity(
    entity: &Entity,
    raw_query: &str,
    terms: &[Folded],
    exact: &Folded,
) -> Option<SearchHit> {
    let (title_field, extra): (&str, &[(&str, u32)]) = match entity.kind() {
        EntityKind::Task => (
            fields::task::TITLE,
            &[
                (fields::task::DESCRIPTION, WEIGHT_DESCRIPTION),
                (fields::task::PLAN, WEIGHT_PLAN),
            ],
        ),
        EntityKind::Project => (fields::project::NAME, &[]),
        EntityKind::Area => (fields::area::NAME, &[]),
    };
    let title = entity.str_value(title_field)?.to_owned();

    let is_ref = || {
        entity
            .str_value(fields::task::REF)
            .is_some_and(|r| r.eq_ignore_ascii_case(raw_query))
    };
    if entity.kind() == EntityKind::Task
        && (entity.str_value(fields::task::CLICKUP_ID) == Some(raw_query) || is_ref())
    {
        return Some(SearchHit {
            entity: entity.kind(),
            entity_id: entity.id(),
            title,
            score: SCORE_CLICKUP_ID,
        });
    }

    let mut searched = vec![(Folded::new(&title), WEIGHT_TITLE)];
    for (field, weight) in extra {
        if let Some(text) = entity.str_value(field) {
            searched.push((Folded::new(text), *weight));
        }
    }

    let mut score = 0;
    for term in terms {
        let mut term_score = 0;
        for (text, weight) in &searched {
            if let Some(word_start) = text.find(term) {
                term_score += weight + if word_start { BONUS_WORD_START } else { 0 };
            }
        }
        if term_score == 0 {
            return None;
        }
        score += term_score;
    }
    if searched[0].0.equals(exact) {
        score += BONUS_EXACT_TITLE;
    }
    Some(SearchHit {
        entity: entity.kind(),
        entity_id: entity.id(),
        title,
        score,
    })
}

/// A text folded two ways: umlauts as base letter (`ä` → `a`) and expanded
/// (`ä` → `ae`). Everything else is lowercased and stripped of accents
/// identically in both variants.
#[derive(Debug, Clone)]
struct Folded {
    short: String,
    long: String,
}

impl Folded {
    fn new(text: &str) -> Self {
        let mut short = String::with_capacity(text.len());
        let mut long = String::with_capacity(text.len());
        for c in text.chars().flat_map(char::to_lowercase) {
            match c {
                'ä' => {
                    short.push('a');
                    long.push_str("ae");
                }
                'ö' => {
                    short.push('o');
                    long.push_str("oe");
                }
                'ü' => {
                    short.push('u');
                    long.push_str("ue");
                }
                'ß' => {
                    short.push_str("ss");
                    long.push_str("ss");
                }
                other => {
                    let base = strip_accent(other);
                    short.push(base);
                    long.push(base);
                }
            }
        }
        Self { short, long }
    }

    /// Whether `term` occurs; `Some(true)` if it occurs at a word start.
    fn find(&self, term: &Folded) -> Option<bool> {
        let mut found = None;
        for hay in [&self.short, &self.long] {
            for needle in [&term.short, &term.long] {
                for (pos, _) in hay.match_indices(needle.as_str()) {
                    let word_start = hay[..pos]
                        .chars()
                        .next_back()
                        .is_none_or(|c| !c.is_alphanumeric());
                    if word_start {
                        return Some(true);
                    }
                    found = Some(false);
                }
            }
        }
        found
    }

    fn equals(&self, other: &Folded) -> bool {
        self.short == other.short || self.long == other.long
    }
}

fn strip_accent(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'å' | 'ā' => 'a',
        'ç' | 'č' | 'ć' => 'c',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' => 'e',
        'ì' | 'í' | 'î' | 'ï' | 'ī' => 'i',
        'ñ' | 'ń' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ø' | 'ō' => 'o',
        'ù' | 'ú' | 'û' | 'ū' => 'u',
        'ý' | 'ÿ' => 'y',
        'š' | 'ś' => 's',
        'ž' | 'ź' | 'ż' => 'z',
        'ł' => 'l',
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::{NewArea, NewProject, NewTask, OpBatch};
    use crate::hlc::{Clock, NodeId};
    use crate::model::TaskStatus;

    fn fixture() -> (State, [Id; 6]) {
        let mut clock = Clock::new(NodeId::new("test").unwrap());
        let mut b = OpBatch::new(&mut clock, 1_760_000_000_000);
        let area = b.create_area(NewArea::new("Gesundheit"));
        let project = b.create_project(NewProject::new("Ärzte-Termine", area));
        let dentist = b.create_task(NewTask::new("Zahnarzt anrufen"));
        let done = b.create_task(NewTask::new("Rezept abholen").with_status(TaskStatus::Done));
        let deleted = b.create_task(NewTask::new("Zahnbürste kaufen"));
        b.delete(EntityKind::Task, deleted);
        let plan = b.create_task(NewTask {
            plan: Some("Erst den Arzt fragen, dann Straße prüfen".into()),
            clickup_id: Some("86abc".into()),
            ..NewTask::new("Überweisung")
        });
        (
            State::from_ops(b.ops()),
            [area, project, dentist, done, deleted, plan],
        )
    }

    fn ids(hits: &[SearchHit]) -> Vec<Id> {
        hits.iter().map(|h| h.entity_id).collect()
    }

    #[test]
    fn case_insensitive_and_word_start_ranking() {
        let (state, [_, _, dentist, ..]) = fixture();
        let hits = search(&state, "ZAHNARZT", 0);
        assert_eq!(ids(&hits), vec![dentist]);
        assert!(search(&state, "", 0).is_empty());
        assert!(search(&state, "   ", 0).is_empty());
    }

    #[test]
    fn umlaut_variants_match() {
        let (state, [_, project, _, _, _, plan]) = fixture();
        for q in ["ärzte", "Aerzte", "arzte"] {
            assert!(ids(&search(&state, q, 0)).contains(&project), "{q}");
        }
        for q in ["ueberweisung", "uberweisung", "Überweisung"] {
            assert_eq!(ids(&search(&state, q, 0)), vec![plan], "{q}");
        }
        assert_eq!(ids(&search(&state, "strasse", 0)), vec![plan]);
    }

    #[test]
    fn closed_included_deleted_excluded() {
        let (state, [_, _, _, done, deleted, _]) = fixture();
        assert_eq!(ids(&search(&state, "rezept", 0)), vec![done]);
        assert!(!ids(&search(&state, "zahnbürste", 0)).contains(&deleted));
    }

    #[test]
    fn all_terms_must_match_and_title_outranks_plan() {
        let (state, [area, project, dentist, _, _, plan]) = fixture();
        assert_eq!(ids(&search(&state, "zahnarzt anrufen", 0)), vec![dentist]);
        assert!(search(&state, "zahnarzt kaufen", 0).is_empty());
        // "arzt" in the title of the dentist task (inside a word), the
        // project name and the plan of another task.
        let hits = search(&state, "arzt", 0);
        assert!(hits.iter().any(|h| h.entity_id == plan));
        assert!(hits.iter().any(|h| h.entity_id == project));
        assert!(!hits.iter().any(|h| h.entity_id == area));
        let plan_score = hits.iter().find(|h| h.entity_id == plan).unwrap().score;
        let project_score = hits.iter().find(|h| h.entity_id == project).unwrap().score;
        assert!(project_score > plan_score);
        assert_eq!(search(&state, "arzt", 1).len(), 1);
    }

    #[test]
    fn exact_title_and_clickup_id() {
        let (state, [area, _, _, _, _, plan]) = fixture();
        let hits = search(&state, "gesundheit", 0);
        assert_eq!(hits[0].entity_id, area);
        assert_eq!(hits[0].entity, EntityKind::Area);
        assert_eq!(hits[0].title, "Gesundheit");
        assert_eq!(search(&state, "86abc", 0)[0].entity_id, plan);
    }
}
