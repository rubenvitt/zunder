//! Commands for the web UI's `TauriTransport` (contract: app README and
//! implementation contract, section 5). Errors arrive in JavaScript as
//! `{"code", "message"}`.

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;
use zunder_core::hlc::system_time_ms;
use zunder_core::ids::Id;
use zunder_core::Op;
use zunder_local::sync::lock;
use zunder_local::{
    login, ApplyResult, ConfigUpdate, ConfigView, Error, FlameAction, HistoryEntry, LocalSnapshot,
    SyncReport,
};

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
pub fn set_config(app: AppHandle, update: ConfigUpdate) -> Result<ConfigView, Error> {
    apply_config(&app, &update)
}

/// `set_config`, also used when a login stores its device token: a new
/// server or token starts a sync and re-plans reminders.
pub fn apply_config(app: &AppHandle, update: &ConfigUpdate) -> Result<ConfigView, Error> {
    let state = app.state::<AppState>();
    let before = lock(&state.store).config()?;
    let after = lock(&state.store).set_config(update)?;
    let token_changed = update.api_token.is_some();
    if after.server_url != before.server_url
        || after.has_api_token != before.has_api_token
        || token_changed
    {
        state.sync_wake.notify_one();
        notify::refresh(app);
    }
    Ok(after)
}

/// Starts a login in the system browser (server login and consent). The
/// outcome arrives as `zunder://login` once the browser returns to the app.
/// A server switch that would drop pending operations fails right away with
/// `pending_ops` unless `discard_pending` is set.
#[tauri::command(rename_all = "snake_case")]
pub async fn login_start(
    app: AppHandle,
    server_url: String,
    device_name: String,
    discard_pending: bool,
) -> Result<(), Error> {
    let state = app.state::<AppState>();
    lock(&state.store).check_server_switch(&server_url, discard_pending)?;
    let now = i64::try_from(system_time_ms()).unwrap_or(i64::MAX);
    let (pending, url) = login::start(&server_url, &device_name, discard_pending, now).await?;
    lock(&state.store).set_pending_login(Some(&pending))?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| Error::Login(format!("Browser lässt sich nicht öffnen: {e}")))
}

/// `GET /api/tasks/{id}/history` with the stored API token (the webview may
/// not talk to the server itself). Operations that are still pending are
/// not in the server's history yet.
#[tauri::command(rename_all = "snake_case")]
pub async fn task_history(
    state: State<'_, AppState>,
    task_id: Id,
) -> Result<Vec<HistoryEntry>, Error> {
    let client = state.syncer.client(&state.store)?;
    client.task_history(task_id).await
}

/// Starts the task's flame on the server (Timing bridge) and syncs, so the
/// flame fields the server wrote arrive right away (`zunder://changed`).
#[tauri::command(rename_all = "snake_case")]
pub async fn flame_start(app: AppHandle, task_id: Id) -> Result<serde_json::Value, Error> {
    flame(&app, task_id, FlameAction::Start).await
}

/// Stops the task's flame on the server and syncs. Without a running flame
/// the server answers 409 (`server` error).
#[tauri::command(rename_all = "snake_case")]
pub async fn flame_stop(app: AppHandle, task_id: Id) -> Result<serde_json::Value, Error> {
    flame(&app, task_id, FlameAction::Stop).await
}

async fn flame(
    app: &AppHandle,
    task_id: Id,
    action: FlameAction,
) -> Result<serde_json::Value, Error> {
    let state = app.state::<AppState>();
    // The server only knows tasks it has received: push first. A failure
    // here shows up again in the flame request itself.
    if lock(&state.store).pending_count() > 0 {
        let _ = background::sync(app).await;
    }
    let client = state.syncer.client(&state.store)?;
    let outcome = client.flame(task_id, action).await?;
    // Fetch the flame fields; a failed sync is reported via `zunder://sync`
    // and retried by the background loop, the flame itself succeeded.
    let _ = background::sync(app).await;
    Ok(outcome)
}

/// `GET /api/settings/focus` with the stored token ("Erst für dich": day
/// clicked away, task picked for a day), answer unchanged. Offline it fails
/// with `network` and the web UI keeps its local value; with an API token
/// instead of a device token it is `unsupported`.
#[tauri::command]
pub async fn focus_get(state: State<'_, AppState>) -> Result<serde_json::Value, Error> {
    let client = state.syncer.client(&state.store)?;
    client.focus_settings().await
}

/// `PUT /api/settings/focus` with the web UI's update unchanged (a missing
/// field stays, `null` removes it); returns the server's new value.
#[tauri::command]
pub async fn focus_set(
    state: State<'_, AppState>,
    update: serde_json::Value,
) -> Result<serde_json::Value, Error> {
    let client = state.syncer.client(&state.store)?;
    client.update_focus_settings(&update).await
}

// ----- app settings ----------------------------------------------------------

#[cfg(mobile)]
fn desktop_only() -> Error {
    Error::Unsupported("Nur in der Desktop-App verfügbar.".into())
}

/// What the settings show about the app itself.
#[derive(Debug, Serialize)]
pub struct AppInfo {
    /// Version of the running app.
    version: String,
    /// Desktop (menu bar, autostart, self-update) or mobile.
    desktop: bool,
    /// Whether the app starts at login; `null` on mobile or when unknown.
    autostart: Option<bool>,
    /// Self-update state; `null` on mobile.
    update: Option<UpdateStatus>,
}

#[cfg(mobile)]
#[derive(Debug, Serialize)]
pub struct UpdateStatus {}

#[cfg(desktop)]
use crate::update::UpdateStatus;

#[tauri::command]
pub fn app_info(app: AppHandle) -> AppInfo {
    let version = app.package_info().version.to_string();
    #[cfg(desktop)]
    {
        let autostart = crate::desktop::autostart_enabled(&app)
            .inspect_err(|e| log::warn!("reading autostart failed: {e}"))
            .ok();
        AppInfo {
            version,
            desktop: true,
            autostart,
            update: Some(app.state::<crate::update::Updates>().status()),
        }
    }
    #[cfg(mobile)]
    AppInfo {
        version,
        desktop: false,
        autostart: None,
        update: None,
    }
}

/// Starts the app at login (hidden in the menu bar) or not. Returns the new
/// state.
#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> Result<bool, Error> {
    #[cfg(desktop)]
    return crate::desktop::set_autostart(&app, enabled);
    #[cfg(mobile)]
    {
        let _ = (app, enabled);
        Err(desktop_only())
    }
}

/// Looks for a newer version now and installs it (active after
/// `restart_app`).
#[tauri::command]
pub async fn check_update(app: AppHandle) -> Result<UpdateStatus, Error> {
    #[cfg(desktop)]
    return crate::update::check(&app).await;
    #[cfg(mobile)]
    {
        let _ = app;
        Err(desktop_only())
    }
}

/// Restarts the app, e.g. to activate an installed update.
#[tauri::command]
pub fn restart_app(app: AppHandle) {
    app.request_restart();
}

/// Shows a notification right away, to check that the system lets them
/// through.
#[tauri::command]
pub fn test_notification(app: AppHandle) -> Result<(), Error> {
    use tauri_plugin_notification::NotificationExt;
    app.notification()
        .builder()
        .title("Zunder")
        .body("Benachrichtigungen kommen an.")
        .show()
        .map_err(|e| Error::Notification(e.to_string()))
}
