//! `zunder-import`: one-time import from ClickUp into Zunder.
//!
//! - [`clickup`]: ClickUp API v2 client behind the [`clickup::ClickUpApi`] trait
//! - [`fetch`]: reading hierarchy and tasks
//! - [`mapping`]: the mapping file (`plan` proposes it, the user edits it)
//! - [`convert`]: ClickUp tasks → desired Zunder state + diagnostics
//! - [`ops`]: diff against Zunder, operations and push batches
//! - [`zunder`]: client for the Zunder sync API
//! - [`plan`], [`report`], [`run`], [`backup`]: the subcommands

pub mod backup;
pub mod clickup;
pub mod convert;
pub mod fetch;
pub mod mapping;
pub mod ops;
pub mod plan;
pub mod report;
pub mod run;
pub mod zunder;
