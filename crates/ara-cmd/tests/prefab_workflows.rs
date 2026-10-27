//! Prefab source/instance transactions, inheritance, typed references and recovery.
#![allow(clippy::unwrap_used)]
#[path = "prefab_workflows/apply.rs"]
mod apply;
#[path = "prefab_workflows/common.rs"]
mod common;
#[path = "prefab_workflows/create.rs"]
mod create;
#[path = "prefab_workflows/failures.rs"]
mod failures;
#[path = "prefab_workflows/recovery.rs"]
mod recovery;
#[path = "prefab_workflows/undo.rs"]
mod undo;
