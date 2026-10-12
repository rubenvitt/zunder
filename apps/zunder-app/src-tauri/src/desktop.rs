//! Desktop shell: Zunder keeps running in the menu bar (tray) when its
//! window is closed, so reminders still fire; it starts at login if the
//! user enables that (`tauri-plugin-autostart`), then hidden in the menu bar.
//!
//! - Closing the window hides it; "Beenden" in the menu bar (or Cmd+Q on
//!   macOS) quits.
//! - The menu bar icon and, on macOS, the Dock icon bring the window back.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{App, AppHandle, Manager, WindowEvent};
use tauri_plugin_autostart::ManagerExt;
use zunder_local::Error;

/// Label of the window from `tauri.conf.json`.
pub const MAIN_WINDOW: &str = "main";

/// Passed by the autostart entry: start hidden in the menu bar.
const HIDDEN_ARG: &str = "--hidden";

pub fn autostart_plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    // On macOS the builder's default is a launch agent, which passes the
    // arguments (a login item would not).
    tauri_plugin_autostart::Builder::new()
        .args([HIDDEN_ARG])
        .build()
}

/// Whether this process was started by the autostart entry.
pub fn started_hidden() -> bool {
    std::env::args().any(|arg| arg == HIDDEN_ARG)
}

/// Installs the menu bar icon and turns closing the window into hiding it.
pub fn setup(app: &App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Zunder öffnen", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Beenden", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &PredefinedMenuItem::separator(app)?, &quit])?;
    let mut tray = TrayIconBuilder::with_id("zunder")
        .tooltip("Zunder")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main(app),
            "quit" => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;

    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let hide = window.clone();
        window.on_window_event(move |event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if let Err(e) = hide.hide() {
                    log::warn!("hiding the window failed: {e}");
                }
            }
        });
    }
    Ok(())
}

/// Shows and focuses the main window.
pub fn show_main(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };
    let shown = window
        .unminimize()
        .and_then(|()| window.show())
        .and_then(|()| window.set_focus());
    if let Err(e) = shown {
        log::warn!("showing the window failed: {e}");
    }
}

pub fn autostart_enabled(app: &AppHandle) -> Result<bool, Error> {
    app.autolaunch()
        .is_enabled()
        .map_err(|e| Error::Autostart(e.to_string()))
}

pub fn set_autostart(app: &AppHandle, enabled: bool) -> Result<bool, Error> {
    let autolaunch = app.autolaunch();
    let changed = if enabled {
        autolaunch.enable()
    } else {
        autolaunch.disable()
    };
    changed.map_err(|e| Error::Autostart(e.to_string()))?;
    autostart_enabled(app)
}
