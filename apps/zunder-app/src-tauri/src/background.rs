//! Background sync: every 30 s, shortly after local changes, when the app
//! returns to the foreground and when the webview reports `online`. After
//! network errors it retries sooner (5 s, 10 s, 20 s) so a regained
//! connection is used quickly.

use std::time::Duration;

use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};
use zunder_core::hlc::system_time_ms;
use zunder_local::sync::{lock, retry_delay, SYNC_INTERVAL};
use zunder_local::{Error, SyncReport};

use crate::{events, notify, AppState};

/// Collects further local changes before pushing.
const DEBOUNCE: Duration = Duration::from_millis(500);

/// Runs one sync round and tells the webview about the outcome.
pub async fn sync(app: &AppHandle) -> Result<SyncReport, Error> {
    let state = app.state::<AppState>();
    let now_ms = i64::try_from(system_time_ms()).unwrap_or(i64::MAX);
    let result = state.syncer.sync(&state.store, now_ms).await;

    let payload = match &result {
        Ok(report) => json!({ "ok": true, "report": report }),
        Err(error) => json!({ "ok": false, "error": error }),
    };
    emit(app, events::SYNC, payload);
    if let Ok(report) = &result {
        if report.changed {
            emit(
                app,
                events::CHANGED,
                json!({ "cursor": report.cursor, "pending": report.pending }),
            );
            notify::refresh(app);
        }
    }
    result
}

fn emit(app: &AppHandle, event: &str, payload: serde_json::Value) {
    if let Err(e) = app.emit(event, payload) {
        log::warn!("emitting {event} failed: {e}");
    }
}

/// Starts the sync loop. The first round runs right away.
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut delay = Duration::ZERO;
        let mut failures: u32 = 0;
        loop {
            let state = app.state::<AppState>();
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = state.sync_wake.notified() => tokio::time::sleep(DEBOUNCE).await,
            }
            let configured = matches!(lock(&state.store).remote(), Ok(Some(_)));
            if !configured {
                failures = 0;
                delay = SYNC_INTERVAL;
                continue;
            }
            match sync(&app).await {
                Ok(_) => failures = 0,
                Err(e) if e.is_network() => failures = failures.saturating_add(1),
                Err(e) => {
                    log::warn!("sync failed: {e}");
                    failures = 0;
                }
            }
            delay = retry_delay(failures);
        }
    });
}
