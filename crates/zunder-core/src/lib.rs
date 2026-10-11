//! Domain core of Zunder: model, validation, hybrid logical clock and
//! last-writer-wins merge. No I/O, no async, no database.
//!
//! - [`ids`]: UUIDv7/UUIDv5 helpers
//! - [`hlc`]: hybrid logical clock and its wire format
//! - [`op`]: field operations (unit of change and sync)
//! - [`model`]: field names, enums, typed views and value validation
//! - [`state`]: materialized LWW state with derived data and snapshots
//! - [`validate`]: push validation with stable rejection codes
//! - [`builder`]: ergonomic construction of operation batches
//! - [`search`]: diacritic-tolerant full-text search

pub mod builder;
pub mod hlc;
pub mod ids;
pub mod model;
pub mod op;
pub mod search;
pub mod state;
pub mod validate;

#[cfg(test)]
mod testutil;

pub use builder::{NewArea, NewProject, NewTask, OpBatch};
pub use hlc::{Clock, Hlc, NodeId};
pub use model::{
    Area, AreaState, Due, IsoWeek, Link, Location, Phoenix, PhoenixRole, Project, ProjectKind,
    ProjectStatus, ProjectView, Task, TaskStatus, TaskType,
};
pub use op::{EntityKind, Op};
pub use state::{Progress, SnapshotEntity, State};
pub use validate::{validate_push, PushOutcome, RejectCode, Rejection, Warning, WarningCode};
