//! Self-update of the desktop app from GitHub Releases
//! (`tauri-plugin-updater`, endpoint and public key in `tauri.conf.json`).
//!
//! Checks at startup and then every few hours. A newer version is
//! downloaded and installed in the background; it takes effect on the next
//! start (the app is not restarted under the user). Debug builds never
//! update themselves.

use std::time::Duration;

use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::UpdaterExt;

const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

pub fn start(app: AppHandle) {
    if cfg!(debug_assertions) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        loop {
            match install(&app).await {
                // Installed: the next check would fetch the same version again.
                Ok(true) => return,
                Ok(false) => {}
                Err(e) => log::warn!("update check failed: {e}"),
            }
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

/// Installs a newer version if there is one; `true` when installed.
async fn install(app: &AppHandle) -> tauri_plugin_updater::Result<bool> {
    let Some(update) = app.updater()?.check().await? else {
        return Ok(false);
    };
    update.download_and_install(|_, _| {}, || {}).await?;
    let shown = app
        .notification()
        .builder()
        .title("Zunder aktualisiert")
        .body(format!(
            "Version {} ist installiert und wird beim nächsten Start aktiv.",
            update.version
        ))
        .show();
    if let Err(e) = shown {
        log::warn!("showing update notification failed: {e}");
    }
    Ok(true)
}
