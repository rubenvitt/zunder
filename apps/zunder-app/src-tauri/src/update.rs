//! Self-update of the desktop app from GitHub Releases
//! (`tauri-plugin-updater`, endpoint and public key in `tauri.conf.json`).
//!
//! Checks at startup and then every few hours, and on demand from the
//! settings (`check_update`). A newer version is downloaded and installed in
//! the background; it takes effect on the next start (`restart_app`, the app
//! is never restarted under the user). Debug builds never update themselves.

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::UpdaterExt;
use zunder_core::hlc::system_time_ms;
use zunder_local::Error;

use crate::events;

const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// What the settings show about updates.
#[derive(Debug, Clone, Default, Serialize)]
pub struct UpdateStatus {
    /// Version installed and waiting for a restart.
    pub installed: Option<String>,
    /// The last check, if any ran since the start.
    pub last_check: Option<LastCheck>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LastCheck {
    pub at_ms: i64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Update bookkeeping of the running app.
#[derive(Default)]
pub struct Updates {
    /// Serializes checks (background loop and `check_update`).
    running: tokio::sync::Mutex<()>,
    status: Mutex<UpdateStatus>,
}

impl Updates {
    pub fn status(&self) -> UpdateStatus {
        self.lock_status().clone()
    }

    fn lock_status(&self) -> MutexGuard<'_, UpdateStatus> {
        self.status.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub fn start(app: AppHandle) {
    if cfg!(debug_assertions) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        loop {
            match check(&app).await {
                // Installed: the next check would fetch the same version again.
                Ok(status) if status.installed.is_some() => return,
                Ok(_) => {}
                Err(e) => log::warn!("update check failed: {e}"),
            }
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

/// Checks for a newer version and installs it. Once a version is installed,
/// further checks return right away (it is active after the restart).
pub async fn check(app: &AppHandle) -> Result<UpdateStatus, Error> {
    if cfg!(debug_assertions) {
        return Err(Error::Unsupported(
            "Eine Entwicklungsversion aktualisiert sich nicht selbst.".into(),
        ));
    }
    let updates = app.state::<Updates>();
    let _serial = updates.running.lock().await;
    if updates.status().installed.is_some() {
        return Ok(updates.status());
    }
    let result = install(app).await;
    let at_ms = i64::try_from(system_time_ms()).unwrap_or(i64::MAX);
    let status = {
        let mut status = updates.lock_status();
        status.last_check = Some(LastCheck {
            at_ms,
            ok: result.is_ok(),
            error: result.as_ref().err().map(ToString::to_string),
        });
        if let Ok(Some(version)) = &result {
            status.installed = Some(version.clone());
        }
        status.clone()
    };
    match result {
        Ok(Some(version)) => {
            notify_installed(app, &version);
            if let Err(e) = app.emit(events::UPDATE, &status) {
                log::warn!("emitting {} failed: {e}", events::UPDATE);
            }
            Ok(status)
        }
        Ok(None) => Ok(status),
        Err(e) => Err(Error::Update(e.to_string())),
    }
}

/// Installs a newer version if there is one and returns its version.
async fn install(app: &AppHandle) -> tauri_plugin_updater::Result<Option<String>> {
    let Some(update) = app.updater()?.check().await? else {
        return Ok(None);
    };
    update.download_and_install(|_, _| {}, || {}).await?;
    Ok(Some(update.version))
}

fn notify_installed(app: &AppHandle, version: &str) {
    let shown = app
        .notification()
        .builder()
        .title("Zunder aktualisiert")
        .body(format!(
            "Version {version} ist installiert und wird beim nächsten Start aktiv."
        ))
        .show();
    if let Err(e) = shown {
        log::warn!("showing update notification failed: {e}");
    }
}
