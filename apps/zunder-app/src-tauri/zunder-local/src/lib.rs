//! Local side of the Zunder app, free of any webview dependency:
//!
//! - [`store`]: SQLite store with the operation log (confirmed + pending),
//!   the materialized state, configuration and scheduled reminders
//! - [`client`]: HTTP client for the server's sync API, plus task history
//!   and flames, which the app calls on behalf of the web UI
//! - [`sync`]: one sync round (push, then snapshot or pull) and [`Syncer`]
//! - [`login`]: login through the server's OAuth authorization server
//!   (device token instead of a pasted API token)
//! - [`reminders`]: which local notifications should be pending and how to
//!   get there from what is scheduled
//! - [`schedule`]: executes such a plan against an OS scheduler (mobile)
//! - [`wire`]: request and response bodies of the sync API
//!
//! The Tauri shell (`zunder-app`) wraps these in commands; see the app README
//! for the command contract.

pub mod client;
pub mod error;
pub mod login;
pub mod reminders;
pub mod schedule;
pub mod store;
pub mod sync;
pub mod wire;

#[cfg(test)]
mod testutil;

pub use client::{FlameAction, HttpClient, SyncApi};
pub use error::{Error, Result};
pub use login::PendingLogin;
pub use reminders::{
    catch_up_notices, missed_reminders, plan_schedule, upcoming_reminders, Notice, Reminder,
    SchedulePlan, ScheduledReminder,
};
pub use store::{
    ApplyResult, ConfigUpdate, ConfigView, LastSync, LocalSnapshot, LocalStore, Remote,
};
pub use sync::{sync_once, SyncFailure, SyncReport, Syncer};
pub use wire::HistoryEntry;
