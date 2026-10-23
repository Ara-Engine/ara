//! Deterministic simulation and edit-world projection (same binary and platform).
//!
//! Gameplay state belongs in registered components. Systems must not retain mutable
//! future-affecting state in closures, `Local`, globals or external services. Use
//! stable [`Identity`] / schema entity references, not raw Bevy handles, in saved data.
//! Presentation reads the world and [`FrameAdvance::alpha`] without mutating it.

mod command;
mod context;
mod dynamic;
mod feature;
mod fixed;
mod persistent;
mod play;
mod projection;
mod provider;
mod registry;
mod session;
mod snapshot;
mod source;
mod streaming;

pub use ara_input as input;
pub use bevy_ecs;

/// Initializes the process-wide compute pool before creating sessions. Returns the
/// actual worker count (an existing host pool is reused and never silently resized).
pub fn initialize_compute_pool(workers: std::num::NonZeroUsize) -> usize {
    bevy_tasks::ComputeTaskPool::get_or_init(|| {
        bevy_tasks::TaskPoolBuilder::new()
            .num_threads(workers.get())
            .build()
    })
    .thread_num()
}
pub use command::{CommandContext, SimCommand};
pub use context::{SimContext, StreamRng};
pub use dynamic::{DynamicLayout, LayoutField};
pub use feature::{PredictionKey, SimulationFeature};
pub use fixed::{Clock, FixedTick, FrameAdvance, TickConfig};
pub use persistent::{PersistenceSelection, PersistentCursor, PersistentEntity, PersistentState};
pub use play::{PlayControl, PlayState, Sample, StepInput, StepReport, Watch};
pub use projection::{EditWorld, Identity, SceneMetadata};
pub use provider::{StateProvider, TrustedState};
pub use registry::SimRegistry;
pub use session::{BoundaryCommand, Session, SessionAdapter, SessionMode, TickRecord};
pub use snapshot::{Replay, Snapshot, StateHash};
pub use streaming::{PreparedStreamEntity, StreamChange, StreamingBoundary};

/// A rejected simulation operation, with a stable readable description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimError(
    /// Description of the rejected operation or failed boundary.
    pub String,
);

impl std::fmt::Display for SimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for SimError {}
impl From<String> for SimError {
    fn from(value: String) -> Self {
        Self(value)
    }
}
impl From<serde_json::Error> for SimError {
    fn from(value: serde_json::Error) -> Self {
        Self(value.to_string())
    }
}

type Result<T> = std::result::Result<T, SimError>;
