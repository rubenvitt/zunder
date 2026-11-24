//! Timing hours per goal and ISO week (design ZUN-66, 5 and 6).
//!
//! A goal names Timing projects (`timing_projects`). The server asks
//! Timing's report once per week for the union of all projects of the
//! active goals, sums the seconds per goal and keeps the raw answer for a
//! few minutes. Only time entries count (see [`crate::timing`]); the flame
//! project never counts, so flames are not counted twice.
//!
//! The answer never fails: without a token it is `not_configured`, when
//! Timing does not answer in time or with an error it is `unavailable`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use time::{OffsetDateTime, PrimitiveDateTime, Time};
use zunder_core::ids::Id;
use zunder_core::recurrence::LocalZone;
use zunder_core::IsoWeek;

use crate::app::TimingBridge;
use crate::timing::{project_path, ProjectTime};

/// How long a report of the running (or a future) week is reused.
pub const TTL_CURRENT_WEEK: Duration = Duration::from_secs(5 * 60);
/// How long a report of a past week is reused.
pub const TTL_PAST_WEEK: Duration = Duration::from_secs(60 * 60);
/// Longest wait for Timing; afterwards the answer is `unavailable`.
pub const TIMING_TIMEOUT: Duration = Duration::from_secs(5);
/// Cached reports before old ones are dropped.
const MAX_CACHE_ENTRIES: usize = 200;

/// Whether the hours could be asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalHoursStatus {
    Ok,
    /// No Timing token in effect.
    NotConfigured,
    /// Timing did not answer (in time) or answered with an error.
    Unavailable,
}

/// Answer of `GET /api/goal-hours`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GoalHours {
    /// ISO week `JJJJ-Www`.
    pub week: String,
    pub status: GoalHoursStatus,
    /// Minutes per goal; goals without time (or without projects) are missing.
    pub goals: BTreeMap<Id, u32>,
    /// Why Timing is unavailable (German).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl GoalHours {
    pub fn not_configured(week: IsoWeek) -> Self {
        Self::with(week, GoalHoursStatus::NotConfigured, BTreeMap::new(), None)
    }

    pub fn unavailable(week: IsoWeek, message: String) -> Self {
        Self::with(
            week,
            GoalHoursStatus::Unavailable,
            BTreeMap::new(),
            Some(message),
        )
    }

    pub fn ok(week: IsoWeek, goals: BTreeMap<Id, u32>) -> Self {
        Self::with(week, GoalHoursStatus::Ok, goals, None)
    }

    fn with(
        week: IsoWeek,
        status: GoalHoursStatus,
        goals: BTreeMap<Id, u32>,
        message: Option<String>,
    ) -> Self {
        Self {
            week: week.to_string(),
            status,
            goals,
            message,
        }
    }
}

/// First and last second of `week` (Monday 00:00:00 to Sunday 23:59:59
/// local time in `zone`).
pub fn week_bounds(week: IsoWeek, zone: &impl LocalZone) -> (OffsetDateTime, OffsetDateTime) {
    let monday = week.monday();
    let sunday = monday.saturating_add(time::Duration::days(6));
    let at = |naive: PrimitiveDateTime| {
        // The offset at the guessed instant, checked once more at the
        // instant it yields (a change of offset right at midnight).
        let first = naive.assume_offset(zone.offset_at(naive.assume_utc()));
        naive.assume_offset(zone.offset_at(first))
    };
    (
        at(PrimitiveDateTime::new(monday, Time::MIDNIGHT)),
        at(PrimitiveDateTime::new(
            sunday,
            Time::from_hms(23, 59, 59).unwrap_or(Time::MIDNIGHT),
        )),
    )
}

/// Whole minutes per goal: the seconds of all its projects summed, then
/// rounded down. A project named by several goals counts for each of them;
/// `exclude` (the flame project) counts for none. Goals with less than one
/// minute are left out.
pub fn distribute(
    goals: &[(Id, Vec<String>)],
    times: &[ProjectTime],
    exclude: Option<&str>,
) -> BTreeMap<Id, u32> {
    let mut seconds: HashMap<&str, f64> = HashMap::new();
    for t in times {
        if Some(t.project.as_str()) != exclude {
            *seconds.entry(t.project.as_str()).or_default() += t.seconds.max(0.0);
        }
    }
    goals
        .iter()
        .filter_map(|(id, projects)| {
            let distinct: BTreeSet<&str> = projects.iter().map(String::as_str).collect();
            let total: f64 = distinct.into_iter().filter_map(|p| seconds.get(p)).sum();
            let minutes = (total / 60.0).floor();
            (minutes >= 1.0).then(|| (*id, minutes.min(f64::from(u32::MAX)) as u32))
        })
        .collect()
}

/// The flame project of the Timing settings as self link: a self link (or
/// API URL) directly, a title through Timing's project list. `None` when
/// there is none or the title cannot be resolved.
pub async fn flame_project_ref(bridge: &TimingBridge) -> Option<String> {
    let setting = bridge.project.as_deref()?.trim();
    if setting.is_empty() {
        return None;
    }
    if setting.contains("/projects/") {
        return project_path(setting);
    }
    // Timing reads anything else (even a bare number) as a title.
    let projects = bridge.api.projects().await.ok()?;
    projects
        .into_iter()
        .find(|p| p.title == setting)
        .and_then(|p| project_path(&p.self_ref))
}

#[derive(Debug, Clone)]
struct CacheEntry {
    fetched_at: Instant,
    ttl: Duration,
    times: Vec<ProjectTime>,
    exclude: Option<String>,
}

type CacheKey = (String, Vec<String>);

#[derive(Debug, Default)]
struct CacheState {
    entries: HashMap<CacheKey, CacheEntry>,
    /// Raised by [`HoursCache::clear`]: an answer asked before is not stored.
    generation: u64,
}

/// Reports per week and project set, kept in memory.
#[derive(Debug, Default)]
pub struct HoursCache {
    state: Mutex<CacheState>,
}

impl HoursCache {
    fn lock(&self) -> std::sync::MutexGuard<'_, CacheState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Forgets every report (e.g. after the Timing settings changed); a
    /// report still being asked with the old settings is not stored.
    pub fn clear(&self) {
        let mut state = self.lock();
        state.entries.clear();
        state.generation += 1;
    }

    fn generation(&self) -> u64 {
        self.lock().generation
    }

    fn get(&self, key: &CacheKey, now: Instant) -> Option<CacheEntry> {
        self.lock()
            .entries
            .get(key)
            .filter(|e| now.duration_since(e.fetched_at) < e.ttl)
            .cloned()
    }

    /// Stores `entry` unless the cache was cleared since `generation`.
    fn put(&self, key: CacheKey, entry: CacheEntry, generation: u64) {
        let mut state = self.lock();
        if state.generation != generation {
            return;
        }
        let entries = &mut state.entries;
        if entries.len() >= MAX_CACHE_ENTRIES {
            let now = Instant::now();
            entries.retain(|_, e| now.duration_since(e.fetched_at) < e.ttl);
            if entries.len() >= MAX_CACHE_ENTRIES {
                entries.clear();
            }
        }
        entries.insert(key, entry);
    }
}

/// Hours of `week` for `goals` (active goals with their projects), asking
/// Timing through `bridge` unless the cache has a fresh report.
pub async fn goal_hours(
    bridge: &TimingBridge,
    cache: &HoursCache,
    week: IsoWeek,
    current_week: IsoWeek,
    zone: &impl LocalZone,
    goals: Vec<(Id, Vec<String>)>,
) -> GoalHours {
    let projects: Vec<String> = goals
        .iter()
        .flat_map(|(_, p)| p.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if projects.is_empty() {
        return GoalHours::ok(week, BTreeMap::new());
    }
    let key = (week.to_string(), projects);
    let entry = match cache.get(&key, Instant::now()) {
        Some(entry) => entry,
        None => {
            let generation = cache.generation();
            let (from, to) = week_bounds(week, zone);
            let ask = async {
                let times = bridge.api.report(from, to, &key.1).await?;
                Ok::<_, crate::timing::TimingError>((times, flame_project_ref(bridge).await))
            };
            let (times, exclude) = match tokio::time::timeout(TIMING_TIMEOUT, ask).await {
                Ok(Ok(answer)) => answer,
                Ok(Err(e)) => return GoalHours::unavailable(week, e.to_string()),
                Err(_) => {
                    return GoalHours::unavailable(
                        week,
                        "Timing hat nicht rechtzeitig geantwortet.".to_owned(),
                    )
                }
            };
            let entry = CacheEntry {
                fetched_at: Instant::now(),
                ttl: if week < current_week {
                    TTL_PAST_WEEK
                } else {
                    TTL_CURRENT_WEEK
                },
                times,
                exclude,
            };
            cache.put(key, entry.clone(), generation);
            entry
        }
    };
    GoalHours::ok(
        week,
        distribute(&goals, &entry.times, entry.exclude.as_deref()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::{datetime, offset};
    use zunder_core::recurrence::FixedZone;

    fn time(project: &str, seconds: f64) -> ProjectTime {
        ProjectTime {
            project: project.into(),
            seconds,
        }
    }

    #[test]
    fn minutes_per_goal_sum_its_projects_and_skip_the_flame_project() {
        let (a, b, c) = (Id::new(), Id::new(), Id::new());
        let goals = vec![
            (a, vec!["/projects/1".into(), "/projects/2".into()]),
            (b, vec!["/projects/2".into(), "/projects/9".into()]),
            (c, vec!["/projects/3".into()]),
        ];
        let times = [
            time("/projects/1", 1800.5),
            time("/projects/2", 1830.0),
            time("/projects/9", 600.0),
            time("/projects/3", 59.0),
        ];
        let got = distribute(&goals, &times, Some("/projects/9"));
        // a: 3630.5 s = 60 min; b: project 2 counts for it too, 9 not at all.
        assert_eq!(got.get(&a), Some(&60));
        assert_eq!(got.get(&b), Some(&30));
        // Under a minute is left out.
        assert_eq!(got.get(&c), None);
    }

    #[test]
    fn a_project_named_twice_by_one_goal_counts_once() {
        let a = Id::new();
        let goals = vec![(a, vec!["/projects/1".into(), "/projects/1".into()])];
        let got = distribute(&goals, &[time("/projects/1", 600.0)], None);
        assert_eq!(got.get(&a), Some(&10));
    }

    #[test]
    fn a_report_asked_before_a_clear_is_not_stored() {
        let cache = HoursCache::default();
        let key: CacheKey = ("2026-W48".into(), vec!["/projects/1".into()]);
        let entry = CacheEntry {
            fetched_at: Instant::now(),
            ttl: TTL_CURRENT_WEEK,
            times: vec![time("/projects/1", 60.0)],
            exclude: None,
        };
        let before = cache.generation();
        cache.clear();
        cache.put(key.clone(), entry.clone(), before);
        assert!(cache.get(&key, Instant::now()).is_none());
        cache.put(key.clone(), entry, cache.generation());
        assert!(cache.get(&key, Instant::now()).is_some());
    }

    #[test]
    fn week_bounds_in_winter_and_summer_time() {
        let winter = FixedZone(offset!(+1));
        let week = IsoWeek::parse("2026-W48").unwrap();
        assert_eq!(
            week_bounds(week, &winter),
            (
                datetime!(2026-11-23 00:00 +1),
                datetime!(2026-11-29 23:59:59 +1)
            )
        );
        let summer = FixedZone(offset!(+2));
        let week = IsoWeek::parse("2026-W27").unwrap();
        assert_eq!(
            week_bounds(week, &summer),
            (
                datetime!(2026-06-29 00:00 +2),
                datetime!(2026-07-05 23:59:59 +2)
            )
        );
    }
}
