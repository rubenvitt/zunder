//! Local reminders via `tauri-plugin-notification`.
//!
//! - **Mobile (iOS/Android):** the OS schedules the notifications, so they
//!   fire even when the app is not running. After every state change the
//!   app reschedules (`zunder_local::schedule::reschedule`): at most 64
//!   pending notifications (iOS limit), changed ones are cancelled and
//!   scheduled again.
//! - **Desktop:** the plugin cannot schedule (it shows notifications
//!   immediately), so a timer inside the app fires due reminders while the
//!   app runs. Reminders that fall due while the app is closed are not
//!   shown afterwards.

use std::sync::Mutex;

use tauri::{AppHandle, Manager};
use zunder_core::hlc::system_time_ms;

use crate::AppState;

/// Reminder bookkeeping of the running app.
#[derive(Default)]
pub struct Reminders {
    /// Serializes reschedules (plan, OS calls, save).
    #[cfg_attr(desktop, allow(dead_code))]
    reschedule_lock: Mutex<()>,
    /// Wakes the desktop timer after state changes.
    #[cfg_attr(mobile, allow(dead_code))]
    wake: tokio::sync::Notify,
}

fn now_ms() -> i64 {
    i64::try_from(system_time_ms()).unwrap_or(i64::MAX)
}

/// Starts reminder delivery (permission request on mobile, timer on
/// desktop) and plans the current reminders.
pub fn start(app: AppHandle) {
    #[cfg(mobile)]
    mobile::start(app);
    #[cfg(desktop)]
    desktop::start(app);
}

/// Re-plans reminders after a state change. Returns immediately.
pub fn refresh(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    #[cfg(desktop)]
    state.reminders.wake.notify_one();
    #[cfg(mobile)]
    {
        let _ = state;
        mobile::refresh(app.clone());
    }
}

#[cfg(desktop)]
mod desktop {
    use std::time::Duration;

    use tauri::{AppHandle, Manager};
    use tauri_plugin_notification::NotificationExt;
    use zunder_local::reminders::{due_in_window, next_reminder_at, Reminder};
    use zunder_local::sync::lock;

    use super::now_ms;
    use crate::AppState;

    /// Longest sleep between checks; also bounds how far back a reminder
    /// set to a moment in the past can still fire.
    const MAX_IDLE: Duration = Duration::from_secs(30);

    pub fn start(app: AppHandle) {
        tauri::async_runtime::spawn(async move {
            let mut last_check = now_ms();
            loop {
                let state = app.state::<AppState>();
                let next = next_reminder_at(lock(&state.store).state(), last_check);
                let wait = next
                    .map(|at| Duration::from_millis(u64::try_from(at - now_ms()).unwrap_or(0)))
                    .unwrap_or(MAX_IDLE)
                    .min(MAX_IDLE);
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = state.reminders.wake.notified() => {}
                }
                let now = now_ms();
                if now > last_check {
                    let due = due_in_window(lock(&state.store).state(), last_check, now);
                    for reminder in &due {
                        show(&app, reminder);
                    }
                }
                last_check = now;
            }
        });
    }

    fn show(app: &AppHandle, reminder: &Reminder) {
        let result = app
            .notification()
            .builder()
            .title(&reminder.title)
            .body(&reminder.body)
            .show();
        if let Err(e) = result {
            log::warn!("showing reminder failed: {e}");
        }
    }
}

#[cfg(mobile)]
mod mobile {
    use tauri::{AppHandle, Manager};
    use tauri_plugin_notification::{NotificationExt, PermissionState, Schedule};
    use zunder_local::schedule::{plugin_time, reschedule, scheduler_error, NotificationScheduler};
    use zunder_local::ScheduledReminder;

    use std::sync::PoisonError;

    use super::now_ms;
    use crate::AppState;

    struct PluginScheduler<'a>(&'a AppHandle);

    impl NotificationScheduler for PluginScheduler<'_> {
        fn cancel(&self, ids: &[i32]) -> zunder_local::Result<()> {
            self.0
                .notification()
                .cancel(ids.to_vec())
                .map_err(scheduler_error)
        }

        fn schedule(&self, s: &ScheduledReminder) -> zunder_local::Result<()> {
            let at_ms = s.reminder.at_ms;
            let date = plugin_time(at_ms, local_offset_secs(at_ms))?;
            self.0
                .notification()
                .builder()
                .id(s.id)
                .title(&s.reminder.title)
                .body(&s.reminder.body)
                .schedule(Schedule::At {
                    date,
                    repeating: false,
                    allow_while_idle: true,
                })
                .show()
                .map_err(scheduler_error)
        }
    }

    /// The device's UTC offset at `at_ms` (iOS reads the plugin's date as
    /// local wall-clock time, see `plugin_time`). `time`'s own local offset
    /// lookup refuses to work in a multi-threaded process, so chrono reads
    /// the time zone database; without one it falls back to UTC.
    #[cfg(target_os = "ios")]
    fn local_offset_secs(at_ms: i64) -> Option<i32> {
        use chrono::{Local, Offset, TimeZone};
        let local = Local.timestamp_millis_opt(at_ms).single();
        Some(local.map_or(0, |t| t.offset().fix().local_minus_utc()))
    }

    /// Android parses the plugin's date as UTC.
    #[cfg(not(target_os = "ios"))]
    fn local_offset_secs(_at_ms: i64) -> Option<i32> {
        None
    }

    pub fn start(app: AppHandle) {
        tauri::async_runtime::spawn_blocking(move || {
            let notification = app.notification();
            let granted = match notification.permission_state() {
                Ok(PermissionState::Granted) => true,
                _ => matches!(
                    notification.request_permission(),
                    Ok(PermissionState::Granted)
                ),
            };
            if !granted {
                log::warn!("notification permission not granted; reminders stay silent");
            }
            run(&app);
        });
    }

    /// Plugin calls block on the native side, so they run off the main
    /// thread (sync commands run on it).
    pub fn refresh(app: AppHandle) {
        tauri::async_runtime::spawn_blocking(move || run(&app));
    }

    fn run(app: &AppHandle) {
        let state = app.state::<AppState>();
        let _serial = state
            .reminders
            .reschedule_lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Err(e) = reschedule(&state.store, &PluginScheduler(app), now_ms()) {
            log::warn!("rescheduling reminders failed: {e}");
        }
    }
}
