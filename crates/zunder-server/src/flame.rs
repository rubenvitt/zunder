//! Flames (deep-work blocks) on tasks, bridged to Timing.
//!
//! Starting a flame always writes `flame_started_at` (and clears
//! `flame_ended_at`), stopping writes `flame_ended_at` — the fallback of the
//! spec, so flames work without Timing. With `ZUNDER_TIMING_TOKEN` the
//! service additionally starts/stops a Timing timer and stores the entry's
//! self link in `timing_ref`. A Timing failure never blocks the flame; the
//! result reports whether Timing was used and why not.
//!
//! Only one flame runs at a time: starting one ends every other running
//! flame (Timing itself stops its running timer when a new one starts).
//!
//! Operations carry the source `server` (Timing bridge), whatever entry
//! point (HTTP or MCP) triggered them.

use serde::Serialize;
use uuid::Uuid;
use zunder_core::hlc::system_time_ms;
use zunder_core::model::fields::task as f;
use zunder_core::{EntityKind, State, Warning};

use crate::app::{format_ms, App, Source};
use crate::error::{AppError, AppResult};
use crate::timing::StartTimer;
use crate::views::{flame_running, TaskView};

/// What happened with Timing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TimingUsage {
    /// Whether a Timing token is configured.
    pub configured: bool,
    /// Whether the Timing API was called successfully for this action.
    pub used: bool,
    /// Self link of the Timing entry (`timing_ref`).
    pub entry: Option<String>,
    /// German error message when Timing failed or was skipped.
    pub error: Option<String>,
}

impl TimingUsage {
    fn unconfigured() -> Self {
        Self {
            configured: false,
            used: false,
            entry: None,
            error: None,
        }
    }
}

/// Result of starting or stopping a flame.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FlameOutcome {
    pub task: TaskView,
    /// Whether the flame on `task` runs after the action.
    pub running: bool,
    /// Tasks whose running flame was ended by this start.
    pub stopped_task_ids: Vec<Uuid>,
    /// Starting an already running flame changes nothing.
    pub unchanged: bool,
    pub timing: TimingUsage,
    pub warnings: Vec<Warning>,
}

fn live_task(state: &State, id: Uuid) -> AppResult<zunder_core::Task> {
    state
        .task(id)
        .filter(|t| !t.deleted)
        .ok_or_else(|| AppError::NotFound(format!("Aufgabe {id} nicht gefunden.")))
}

fn view(app: &App, id: Uuid) -> AppResult<TaskView> {
    app.read(|s| TaskView::build(s, id, system_time_ms(), false))
        .ok_or_else(|| AppError::NotFound(format!("Aufgabe {id} nicht gefunden.")))
}

/// Starts a flame on a live task.
pub async fn start(app: &App, task_id: Uuid) -> AppResult<FlameOutcome> {
    let (title, already_running, timing_ref) = app.read(|s| {
        live_task(s, task_id).map(|t| (t.title.clone(), flame_running(&t), t.timing_ref))
    })?;
    if already_running {
        return Ok(FlameOutcome {
            task: view(app, task_id)?,
            running: true,
            stopped_task_ids: Vec::new(),
            unchanged: true,
            timing: TimingUsage {
                configured: app.timing().is_some(),
                used: timing_ref.is_some(),
                entry: timing_ref,
                error: None,
            },
            warnings: Vec::new(),
        });
    }

    let timing = match app.timing() {
        None => TimingUsage::unconfigured(),
        Some(api) => {
            let request = StartTimer {
                title,
                project: app.config().timing.as_ref().and_then(|t| t.project.clone()),
                replace_existing: false,
            };
            match api.start(request).await {
                Ok(entry) => TimingUsage {
                    configured: true,
                    used: true,
                    entry: Some(entry.self_ref),
                    error: None,
                },
                Err(e) => {
                    tracing::warn!(error = %e, "starting a Timing timer failed");
                    TimingUsage {
                        configured: true,
                        used: false,
                        entry: None,
                        error: Some(e.to_string()),
                    }
                }
            }
        }
    };

    let now = format_ms(system_time_ms() as i64);
    let entry = timing.entry.clone();
    let committed = app
        .mutate_atomic(Source::Server, move |state, batch| {
            live_task(state, task_id)?;
            let previous: Vec<Uuid> = state
                .live_tasks()
                .filter(|t| t.id != task_id && flame_running(t))
                .map(|t| t.id)
                .collect();
            for id in &previous {
                batch.set_task(*id, f::FLAME_ENDED_AT, now.clone());
            }
            batch.set_task(task_id, f::FLAME_STARTED_AT, now.clone());
            batch.clear(EntityKind::Task, task_id, f::FLAME_ENDED_AT);
            match &entry {
                Some(e) => batch.set_task(task_id, f::TIMING_REF, e.clone()),
                None => batch.clear(EntityKind::Task, task_id, f::TIMING_REF),
            };
            Ok(previous)
        })
        .await;
    let (stopped_task_ids, warnings) = match committed {
        Ok(v) => v,
        Err(e) => {
            // Do not leave an orphaned timer behind.
            if let (Some(api), Some(entry)) = (app.timing(), timing.entry.as_deref()) {
                if let Err(stop) = api.stop_if_running(entry).await {
                    tracing::warn!(error = %stop, "stopping an orphaned Timing timer failed");
                }
            }
            return Err(e);
        }
    };
    Ok(FlameOutcome {
        task: view(app, task_id)?,
        running: true,
        stopped_task_ids,
        unchanged: false,
        timing,
        warnings,
    })
}

/// Stops the running flame of a task.
pub async fn stop(app: &App, task_id: Uuid) -> AppResult<FlameOutcome> {
    let (running, timing_ref) =
        app.read(|s| live_task(s, task_id).map(|t| (flame_running(&t), t.timing_ref)))?;
    if !running {
        return Err(AppError::Conflict(
            "Für diese Aufgabe läuft keine Flamme.".into(),
        ));
    }

    let timing = match (app.timing(), timing_ref) {
        (None, entry) => TimingUsage {
            entry,
            ..TimingUsage::unconfigured()
        },
        (Some(_), None) => TimingUsage {
            configured: true,
            used: false,
            entry: None,
            error: Some("Die Flamme wurde ohne Timing-Eintrag gestartet.".into()),
        },
        (Some(api), Some(entry)) => match api.stop_if_running(&entry).await {
            Ok(_) => TimingUsage {
                configured: true,
                used: true,
                entry: Some(entry),
                error: None,
            },
            Err(e) => {
                tracing::warn!(error = %e, "stopping a Timing timer failed");
                TimingUsage {
                    configured: true,
                    used: false,
                    entry: Some(entry),
                    error: Some(e.to_string()),
                }
            }
        },
    };

    let now = format_ms(system_time_ms() as i64);
    let (_, warnings) = app
        .mutate_atomic(Source::Server, move |state, batch| {
            let task = live_task(state, task_id)?;
            if flame_running(&task) {
                batch.set_task(task_id, f::FLAME_ENDED_AT, now);
            }
            Ok(())
        })
        .await?;
    Ok(FlameOutcome {
        task: view(app, task_id)?,
        running: false,
        stopped_task_ids: vec![task_id],
        unchanged: false,
        timing,
        warnings,
    })
}
