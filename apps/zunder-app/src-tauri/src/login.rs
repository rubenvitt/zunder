//! Login through the system browser (`zunder_local::login`): `login_start`
//! opens the server's login and consent page, the OS hands the redirect to
//! `dev.rubeen.zunder:/oauth` back to the app (deep link), and [`finish`]
//! stores the device token like `set_config` would. The outcome goes to the
//! web UI as `zunder://login`.
//!
//! shortcut: deep links need an installed bundle on macOS (not `tauri dev`)
//! and single-instance on Linux/Windows (not set up, no releases there);
//! the token form stays as a fallback.

use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};
use url::Url;
use zunder_core::hlc::system_time_ms;
use zunder_local::sync::lock;
use zunder_local::{login, ConfigUpdate, Error};

use crate::{commands, events, AppState};

fn now() -> i64 {
    i64::try_from(system_time_ms()).unwrap_or(i64::MAX)
}

/// Handles a URL the OS opened the app with; ignores everything that is not
/// the login callback.
pub fn handle_url(app: &AppHandle, url: Url) {
    if !login::is_callback(&url) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let payload = match finish(&app, &url).await {
            Ok(()) => json!({"ok": true}),
            Err(e) => json!({"ok": false, "error": e}),
        };
        let _ = app.emit(events::LOGIN, payload);
    });
}

async fn finish(app: &AppHandle, url: &Url) -> Result<(), Error> {
    let state = app.state::<AppState>();
    // Single use: whatever happens next, this login is over.
    let pending = {
        let store = lock(&state.store);
        let pending = store.pending_login()?;
        store.set_pending_login(None)?;
        pending
    };
    let pending = pending.filter(|p| !p.expired(now())).ok_or_else(|| {
        Error::Login("Die Anmeldung ist abgelaufen; bitte erneut anmelden".into())
    })?;
    let code = login::code_from_callback(&pending, url)?;
    let token = login::exchange(&pending, &code).await?;
    let previous = lock(&state.store).remote()?;
    let update = ConfigUpdate {
        server_url: Some(Some(pending.server_url.clone())),
        api_token: Some(Some(token.clone())),
        discard_pending: pending.discard_pending,
    };
    if let Err(e) = commands::apply_config(app, &update) {
        // Changes made during the login block the switch: drop the new token.
        let _ = login::revoke(&pending.server_url, &token).await;
        return Err(e);
    }
    // The device token this login replaces (re-login) is no longer needed.
    if let Some(old) = previous.filter(|r| r.api_token.starts_with("zd_") && r.api_token != token) {
        tauri::async_runtime::spawn(async move {
            let _ = login::revoke(&old.server_url, &old.api_token).await;
        });
    }
    Ok(())
}
