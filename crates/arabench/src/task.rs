//! What a benchmark task is: a fixture project, a prompt written like a real user
//! request, budgets, a deterministic verifier and a reference solution.
//!
//! The verifier reads only the finished project (documents, history, files) and the
//! agent's normalized tool calls, never the agent's prose. The reference solution
//! drives the Engine Protocol in-process; `arabench selftest` checks that each
//! verifier fails on the untouched fixture and passes on its solution.

use std::path::Path;

use serde::Serialize;

use crate::solve::Solver;
use crate::world::World;

/// The capability a task exercises (the report groups by it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Area {
    /// Creating, naming, parenting and placing entities.
    Assembly,
    /// Adding, removing and setting components and fields.
    Components,
    /// Prefab instances, overrides and reverts.
    Prefabs,
    /// Reparenting and sibling order.
    Hierarchy,
    /// `@path#id` references.
    References,
    /// Resolving an `ara merge` conflict.
    Merge,
    /// Discovering fields through the schema.
    Schema,
    /// Undoing or reverting agent changes.
    Undo,
    /// Applying a diagnostic's fix.
    Diagnostics,
    /// Finding answers in the generated docs.
    Docs,
    /// One batch with symbolic IDs.
    Batch,
    /// A dry run before a destructive change.
    DryRun,
    /// Several of the above in one request.
    MultiStep,
    /// Finding out why something is broken with the tools, then fixing it.
    Diagnose,
}

/// The project a task starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Fixture {
    /// A fresh `ara init` project (empty `levels/main.scene`).
    Empty,
    /// `ara init` plus `fixtures/outpost/` (types, prefabs, meshes, the outpost level).
    Outpost,
}

/// Limits for one attempt. Exceeding either stops the agent and fails the task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Budget {
    /// Maximum tool-using steps.
    pub turns: u32,
    /// Maximum wall time in seconds.
    pub seconds: u32,
}

impl Budget {
    /// A budget of `turns` steps and `minutes` minutes.
    pub const fn new(turns: u32, minutes: u32) -> Self {
        Budget {
            turns,
            seconds: minutes * 60,
        }
    }
}

/// Prepares the project after the fixture is copied (git state, prior history).
pub type Setup = fn(&Path) -> Result<(), String>;
/// Checks the finished project.
pub type Verify = fn(&World, &mut Checks);
/// Solves the task the way an ideal agent would, through the Engine Protocol.
pub type Solve = fn(&mut Solver) -> Result<(), String>;

/// One benchmark task.
pub struct Task {
    /// Stable ID (`scene-floor-crate`); results are keyed by it.
    pub id: &'static str,
    /// What it exercises.
    pub area: Area,
    /// The request, as a user would write it.
    pub prompt: &'static str,
    /// Starting project.
    pub fixture: Fixture,
    /// Extra preparation, run after the fixture is in place.
    pub setup: Option<Setup>,
    /// Limits.
    pub budget: Budget,
    /// The deterministic verifier.
    pub verify: Verify,
    /// The reference solution.
    pub solve: Solve,
}

/// One verified condition.
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct Check {
    /// What must hold, in a few words.
    pub what: String,
    /// Whether it holds.
    pub ok: bool,
    /// What was found (always set when `ok` is false).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

/// The checks of one verification. Helpers record a check and return what they found
/// so later checks can build on it.
#[derive(Debug, Default)]
pub struct Checks {
    /// Every check, in order.
    pub list: Vec<Check>,
}

impl Checks {
    /// Records a check; returns `ok`.
    pub fn check(&mut self, what: impl Into<String>, ok: bool, detail: impl Into<String>) -> bool {
        self.list.push(Check {
            what: what.into(),
            ok,
            detail: detail.into(),
        });
        ok
    }

    /// Records that `value` is present.
    pub fn some<T>(&mut self, what: impl Into<String>, value: Option<T>) -> Option<T> {
        let ok = value.is_some();
        self.check(what, ok, if ok { "" } else { "missing" });
        value
    }

    /// True if every check passed (and there is at least one).
    pub fn passed(&self) -> bool {
        !self.list.is_empty() && self.list.iter().all(|c| c.ok)
    }
}
