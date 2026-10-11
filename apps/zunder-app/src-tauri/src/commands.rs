//! Commands for the web UI's `TauriTransport` (contract: app README and
//! implementation contract, section 5). Errors arrive in JavaScript as
//! `{"code", "message"}`.

use tauri::{AppHandle, State};
use zunder_core::hlc::system_time_ms;
use zunder_core::Op;
use zunder_local::sync::lock;
use zunder_local::{ApplyResult, ConfigUpdate, ConfigView, Error, LocalSnapshot, SyncReport};

use crate::{background, notify, AppState};

/// The visible state (confirmed + pending) in the `/api/snapshot` shape,
/// plus `pending`.
#[tauri::command]
pub fn local_snapshot(state: State<'_, AppState>) -> LocalSnapshot {
    lock(&state.store).snapshot()
}

/// Validates `ops` locally, applies the accepted ones optimistically and
/// queues them for push (which starts shortly after).
#[tauri::command]
pub fn local_apply(
    app: AppHandle,
    state: State<'_, AppState>,
    ops: Vec<Op>,
) -> Result<ApplyResult, Error> {
    let result = lock(&state.store).apply_local(&ops, system_time_ms())?;
    if result.pending > 0 {
        state.sync_wake.notify_one();
    }
    if !result.accepted.is_empty() {
        notify::refresh(&app);
    }
    Ok(result)
}

/// Pushes pending operations, then pulls (or loads the snapshot on first
/// sync). Also emits `zunder://sync` and, on changes, `zunder://changed`.
#[tauri::command]
pub async fn sync_now(app: AppHandle) -> Result<SyncReport, Error> {
    background::sync(&app).await
}

/// Server URL, whether a token is stored, node ID, cursor, pending count and
/// the last sync attempt. Never returns the token.
#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> Result<ConfigView, Error> {
    lock(&state.store).config()
}

/// Changes server URL and/or token (omitted = unchanged, `null` = clear).
/// Switching servers resets the local data; with pending operations that
/// needs `discard_pending: true`.
#[tauri::command]
pub fn set_config(
    app: AppHandle,
    state: State<'_, AppState>,
    update: ConfigUpdate,
) -> Result<ConfigView, Error> {
    let before = lock(&state.store).config()?;
    let after = lock(&state.store).set_config(&update)?;
    if after.server_url != before.server_url || after.has_api_token != before.has_api_token {
        state.sync_wake.notify_one();
        notify::refresh(&app);
    }
    Ok(after)
}
