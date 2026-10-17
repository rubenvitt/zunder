//! Domain core of Zunder: model, validation, hybrid logical clock and
//! last-writer-wins merge. No I/O, no async, no database.
//!
//! - [`ids`]: UUIDv7/UUIDv5 helpers
//! - [`hlc`]: hybrid logical clock and its wire format
//! - [`op`]: field operations (unit of change and sync)
//! - [`model`]: field names, enums, typed views and value validation
//! - [`numbering`]: project keys and task references (`ZUN-42`)
//! - [`state`]: materialized LWW state with derived data and snapshots
//! - [`validate`]: push validation with stable rejection codes
//! - [`builder`]: ergonomic construction of operation batches
//! - [`blocking`]: task status follows dependencies (`blocked_by`) and
//!   `waiting_on`
//! - [`defer`]: "Heute nicht" and the deferral counter
//! - [`recurrence`]: due date and reminder of the next instance of a series
//! - [`search`]: diacritic-tolerant full-text search
//! - [`balance`]: Wochenbilanz, done tasks of an ISO week by weight and
//!   for whom

pub mod balance;
pub mod blocking;
pub mod builder;
pub mod defer;
pub mod hlc;
pub mod ids;
pub mod model;
pub mod numbering;
pub mod op;
pub mod recurrence;
pub mod search;
pub mod state;
pub mod validate;

#[cfg(test)]
mod testutil;

pub use builder::{NewArea, NewProject, NewTask, OpBatch};
pub use hlc::{Clock, Hlc, NodeId};
pub use model::{
    Area, AreaState, Comment, Due, ForWhom, IsoWeek, Link, Location, MonthDay, Phoenix,
    PhoenixRole, Project, ProjectKind, ProjectStatus, ProjectView, Recurrence, RecurrenceFrom,
    RecurrenceUnit, Task, TaskStatus, TaskType, WeekdayCode, Weight,
};
pub use op::{EntityKind, Op};
pub use state::{Progress, SnapshotEntity, State};
pub use validate::{
    validate_push, validate_push_as, Author, PushOutcome, RejectCode, Rejection, Warning,
    WarningCode,
};

/// Compiles and runs the code examples of the crate README.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
