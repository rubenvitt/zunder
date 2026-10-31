//! Sharing from other apps (iOS, macOS): the share extension
//! (`apps/zunder-app/share`) leaves each shared link or text in the App
//! Group container ([`zunder_local::share`]); the web UI turns them into
//! inbox tasks with `share_take` and `share_done`.
//!
//! The app tells the web UI with `zunder://share` when items wait: at
//! start, on return to the foreground and on a check every few seconds
//! while any wait (on the desktop the app keeps running while someone
//! shares in Safari; on iOS it only runs in the foreground).

use std::time::Duration;

use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};
use zunder_local::{Error, ShareInbox, SharedItem};

use crate::{events, AppState};

/// App Group shared with the extension (entitlements of both targets).
#[cfg(target_os = "ios")]
pub const APP_GROUP: &str = "group.dev.rubeen.zunder";
/// On macOS a group with the team ID as prefix needs no provisioning
/// profile (Developer ID builds).
#[cfg(target_os = "macos")]
pub const APP_GROUP: &str = "H95J852PKP.dev.rubeen.zunder";

const POLL: Duration = Duration::from_secs(3);

/// The inbox in the App Group container; `None` where there is no share
/// extension (Linux, Windows, Android) or the container is not available
/// (app not signed with the App Group entitlement, e.g. `tauri dev`).
pub fn inbox() -> Option<ShareInbox> {
    #[cfg(any(target_os = "ios", target_os = "macos"))]
    {
        use objc2_foundation::{NSFileManager, NSString};
        let manager = NSFileManager::defaultManager();
        let url = manager
            .containerURLForSecurityApplicationGroupIdentifier(&NSString::from_str(APP_GROUP))?;
        let path = url.path()?.to_string();
        Some(ShareInbox::in_container(std::path::Path::new(&path)))
    }
    #[cfg(not(any(target_os = "ios", target_os = "macos")))]
    None
}

/// The shared items waiting for their tasks, oldest first (empty without
/// a share extension).
#[tauri::command]
pub fn share_take(state: State<'_, AppState>) -> Result<Vec<SharedItem>, Error> {
    match &state.share {
        Some(inbox) => inbox.pending(),
        None => Ok(Vec::new()),
    }
}

/// Removes shared items whose tasks exist now; returns how many.
#[tauri::command]
pub fn share_done(state: State<'_, AppState>, ids: Vec<String>) -> Result<usize, Error> {
    match &state.share {
        Some(inbox) => inbox.remove(&ids),
        None => Ok(0),
    }
}

/// Announces waiting items (also when the web UI was told before: after
/// a return to the foreground it may not have seen the event).
pub fn announce(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let Some(inbox) = &state.share else {
        return;
    };
    match inbox.ids() {
        Ok(ids) if !ids.is_empty() => emit(app, ids.len()),
        Ok(_) => {}
        Err(e) => log::warn!("share inbox not readable: {e}"),
    }
}

fn emit(app: &AppHandle, count: usize) {
    if let Err(e) = app.emit(events::SHARE, json!({ "count": count })) {
        log::warn!("emitting {} failed: {e}", events::SHARE);
    }
}

/// Watches the inbox: an event on every check while items wait, so items
/// whose removal failed are retried (the web UI skips what it already
/// created or could not create in this session).
pub fn start(app: AppHandle) {
    let Some(inbox) = app.state::<AppState>().share.clone() else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        loop {
            match inbox.ids() {
                Ok(ids) if !ids.is_empty() => emit(&app, ids.len()),
                Ok(_) => {}
                Err(e) => log::warn!("share inbox not readable: {e}"),
            }
            tokio::time::sleep(POLL).await;
        }
    });
}
