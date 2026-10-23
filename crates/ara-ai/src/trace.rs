//! Debugger stream: what every brain decided this tick, plus a per-brain view
//! (active path, timers, blackboard, scores, perception) for inspectors and agents.
use crate::components::{BrainState, TaskStatus};
use crate::eval::EqsItem;
use crate::program::{Program, Value};
use ara_perception::{Perception, SightOutcome};
use serde::{Deserialize, Serialize};

/// One debugger event. Events of a tick are ordered by brain ID.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum TraceEvent {
    /// Leaf state changed.
    Transition {
        /// Brain stable ID.
        brain: u64,
        /// Previous leaf state.
        from: Option<String>,
        /// New leaf state.
        to: String,
        /// `start`, `done`, `failed`, `when` or `select`.
        cause: String,
    },
    /// Utility scores evaluated under a selector.
    Scores {
        /// Brain stable ID.
        brain: u64,
        /// Selecting state.
        state: String,
        /// (child, score) for every qualifying child.
        scores: Vec<(String, f32)>,
    },
    /// A query ran.
    Eqs {
        /// Brain stable ID.
        brain: u64,
        /// Query name.
        query: String,
        /// Chosen point.
        best: Option<[f32; 3]>,
        /// Every candidate with its score.
        items: Vec<EqsItem>,
    },
    /// A task finished or failed.
    Task {
        /// Brain stable ID.
        brain: u64,
        /// Owning state.
        state: String,
        /// Task name.
        task: String,
        /// New status.
        status: TaskStatus,
    },
    /// An `act` task requested a gameplay action.
    Act {
        /// Brain stable ID.
        brain: u64,
        /// Action name.
        name: String,
        /// Target entity.
        target: Option<u64>,
    },
    /// An `emit` action fired.
    Emit {
        /// Brain stable ID.
        brain: u64,
        /// Event name.
        event: String,
    },
}

/// Inspector/agent view of one brain, with names instead of indices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrainView {
    /// Behaviour name.
    pub behavior: String,
    /// Active states, root to leaf.
    pub path: Vec<String>,
    /// Seconds in each active state.
    pub seconds: Vec<f32>,
    /// Active tasks: (state, task, status).
    pub tasks: Vec<(String, String, TaskStatus)>,
    /// Blackboard: (name, value).
    pub vars: Vec<(String, Value)>,
    /// Latest utility scores: (state, score).
    pub scores: Vec<(String, f32)>,
    /// Latest change: (from, to, cause, seconds ago).
    pub last: Option<(Option<String>, String, String, f32)>,
    /// Remembered stimuli: (id, visible, heard, seconds since sensed).
    pub memory: Vec<(u64, bool, bool, f32)>,
    /// Latest sight checks: (id, outcome).
    pub checks: Vec<(u64, SightOutcome)>,
}

impl BrainView {
    /// Builds a view of `state` running `program`.
    pub fn new(
        program: &Program,
        state: &BrainState,
        perception: Option<&Perception>,
        dt: f32,
    ) -> Self {
        let name = |i: u16| {
            program
                .states
                .get(i as usize)
                .map_or_else(String::new, |s| s.name.clone())
        };
        let mut tasks = Vec::new();
        let mut index = 0;
        for &s in &state.active {
            let node = &program.states[s as usize];
            for t in &node.tasks {
                if let Some(ts) = state.tasks.get(index) {
                    tasks.push((node.name.clone(), t.name().to_owned(), ts.status));
                }
                index += 1;
            }
        }
        Self {
            behavior: program.name.clone(),
            path: state.active.iter().map(|s| name(*s)).collect(),
            seconds: state.ticks.iter().map(|t| *t as f32 * dt).collect(),
            tasks,
            vars: program
                .slots
                .iter()
                .zip(&state.vars)
                .map(|(s, v)| (s.name.clone(), v.clone()))
                .collect(),
            scores: state.scores.iter().map(|(s, v)| (name(*s), *v)).collect(),
            last: state.last.as_ref().map(|c| {
                (
                    c.from.map(name),
                    name(c.to),
                    c.cause.clone(),
                    state.age.saturating_sub(c.at) as f32 * dt,
                )
            }),
            memory: perception.map_or_else(Vec::new, |p| {
                p.known
                    .iter()
                    .map(|k| (k.id, k.visible, k.heard, k.since as f32 * dt))
                    .collect()
            }),
            checks: perception.map_or_else(Vec::new, |p| p.checks.clone()),
        }
    }
}
