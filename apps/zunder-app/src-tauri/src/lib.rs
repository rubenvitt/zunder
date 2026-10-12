//! Tauri shell of Zunder for desktop, iOS and Android.
//!
//! The web UI (`web/`) runs inside the webview and talks to the local store
//! through the commands in [`commands`]. Store, sync and reminder planning
//! live in the webview-free crate `zunder-local`; this crate only wires them
//! into Tauri:
//!
//! - [`commands`]: `local_snapshot`, `local_apply`, `sync_now`,
//!   `get_config`, `set_config`, and the server calls `task_history`,
//!   `flame_start`, `flame_stop`
//! - [`background`]: sync loop (every 30 s, after local changes, on resume
//!   and when the webview reports the network is back)
//! - [`notify`]: local reminders (OS scheduler on mobile, in-process timer
//!   on desktop)
//! - [`update`]: self-update from GitHub Releases (desktop only)

use std::sync::Mutex;

use tauri::{Manager, RunEvent};
use tokio::sync::Notify;
use zunder_local::{LocalStore, Syncer};

pub mod background;
pub mod commands;
pub mod notify;
#[cfg(desktop)]
pub mod update;

/// Events sent to the webview.
pub mod events {
    /// The visible state changed through a sync; payload
    /// `{"cursor": N, "pending": N}`. Reload with `local_snapshot`.
    pub const CHANGED: &str = "zunder://changed";
    /// A sync attempt finished; payload `{"ok": true, "report": SyncReport}`
    /// or `{"ok": false, "error": {"code", "message"}, "report": SyncReport}`
    /// (what the failed round committed before the error). Not sent for a
    /// round cut short by a server switch (`reconfigured`).
    pub const SYNC: &str = "zunder://sync";
}

/// File name of the local database in the app data directory.
const DB_FILE: &str = "zunder.db";

/// Shared state of the running app.
pub struct AppState {
    pub store: Mutex<LocalStore>,
    pub syncer: Syncer,
    /// Wakes the background sync loop early (after local changes etc.).
    pub sync_wake: Notify,
    pub reminders: notify::Reminders,
}

/// Re-runs the sync whenever the webview regains network access.
const ONLINE_SCRIPT: &str = r#"
window.addEventListener('online', function () {
  window.__TAURI_INTERNALS__.invoke('sync_now').catch(function () {});
});
"#;

fn online_plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("zunder-online")
        .js_init_script(ONLINE_SCRIPT)
        .build()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(online_plugin());
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    let app = builder
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            // The database holds the API token: owner only (also for
            // directories created by older versions).
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
            }
            let store = LocalStore::open(dir.join(DB_FILE))?;
            app.manage(AppState {
                store: Mutex::new(store),
                syncer: Syncer::new(),
                sync_wake: Notify::new(),
                reminders: notify::Reminders::default(),
            });
            notify::start(app.handle().clone());
            background::start(app.handle().clone());
            #[cfg(desktop)]
            update::start(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::local_snapshot,
            commands::local_apply,
            commands::sync_now,
            commands::get_config,
            commands::set_config,
            commands::task_history,
            commands::flame_start,
            commands::flame_stop,
        ])
        .build(tauri::generate_context!())
        .expect("Zunder konnte nicht starten");

    app.run(|handle, event| {
        if let RunEvent::Resumed = event {
            // Back in the foreground (mobile): catch up right away.
            if let Some(state) = handle.try_state::<AppState>() {
                state.sync_wake.notify_one();
            }
            notify::refresh(handle);
        }
    });
}
