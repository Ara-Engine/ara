//! Reflected senses and stimuli, plus snapshot-friendly memory components.
use ara_schema::{Schema, SchemaDb};
use bevy_ecs::prelude::{Component, Resource};
use serde::{Deserialize, Serialize};

/// An AI's senses: a sight cone with peripheral vision, hearing and memory.
#[derive(Debug, Clone, Copy, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "ai")]
#[serde(default, rename_all = "camelCase")]
pub struct Perceiver {
    /// Sight range in metres inside the cone.
    #[ara(default=25,unit=Meters,range(min=0,max=10000))]
    pub sight_range: f32,
    /// Full field of view in degrees.
    #[ara(default=120,unit=Degrees,range(min=0,max=360))]
    pub fov: f32,
    /// Range in metres outside the cone (peripheral awareness); 0 disables.
    #[ara(default=2,unit=Meters,range(min=0,max=1000))]
    pub peripheral_range: f32,
    /// Eye height above the feet in metres.
    #[ara(default=1.7,unit=Meters,range(min=0,max=100))]
    pub eye_height: f32,
    /// Hearing multiplier applied to every noise radius.
    #[ara(default = 1, range(min = 0, max = 100))]
    pub hearing: f32,
    /// Noise radius multiplier when the sound path is occluded.
    #[ara(default = 0.5, range(min = 0, max = 1))]
    pub occlusion: f32,
    /// Seconds a memory survives after the last stimulus.
    #[ara(default=8,unit=Seconds,range(min=0,max=3600))]
    pub memory: f32,
}
impl Default for Perceiver {
    fn default() -> Self {
        Self {
            sight_range: 25.0,
            fov: 120.0,
            peripheral_range: 2.0,
            eye_height: 1.7,
            hearing: 1.0,
            occlusion: 0.5,
            memory: 8.0,
        }
    }
}

/// Something perceivers can see and remember (players, corpses, thrown objects).
#[derive(Debug, Clone, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "ai")]
#[serde(default, rename_all = "camelCase")]
pub struct Stimulus {
    /// Category name behaviours query (`player`, `corpse`).
    pub kind: String,
    /// Height of the sight target above the feet in metres.
    #[ara(default=1.2,unit=Meters,range(min=0,max=100))]
    pub height: f32,
    /// Detection range multiplier (0 = invisible, 0.5 = crouched in shadow).
    #[ara(default = 1, range(min = 0, max = 10))]
    pub visibility: f32,
}
impl Default for Stimulus {
    fn default() -> Self {
        Self {
            kind: String::new(),
            height: 1.2,
            visibility: 1.0,
        }
    }
}

/// Stable identity and pose used by perception; the host (or `ara-ai`) syncs it
/// from transforms each tick. Use the same stable ID as physics/navigation.
#[derive(Debug, Clone, Copy, PartialEq, Component, Serialize, Deserialize)]
pub struct PerceptionPose {
    /// Stable simulation ID (never an Entity index).
    pub id: u64,
    /// Feet position in metres.
    pub position: [f32; 3],
    /// Facing direction (XZ is used; need not be normalised).
    pub forward: [f32; 3],
}

/// How the latest sense used for a memory was received.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sense {
    /// Seen inside the cone or peripheral range with clear line of sight.
    Sight,
    /// Heard a noise it made.
    Hearing,
}

/// Outcome of the latest sight check against one candidate target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SightOutcome {
    /// Within range and cone (or peripheral range) with clear line of sight.
    Seen,
    /// Within range but outside the field of view.
    OutsideCone,
    /// Within range and cone, but line of sight is blocked.
    Occluded,
    /// Farther than the (visibility-scaled) sight range.
    OutOfRange,
}

/// One remembered stimulus or noise source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Known {
    /// Stimulus stable ID; 0 for anonymous noises.
    pub id: u64,
    /// FNV-1a hash of the stimulus kind (see [`kind_hash`]).
    pub kind: u32,
    /// Last perceived position (feet).
    pub position: [f32; 3],
    /// Sense that last refreshed the memory.
    pub sense: Sense,
    /// Seen this tick.
    pub visible: bool,
    /// Heard this tick.
    pub heard: bool,
    /// Ticks since last sensed (0 = this tick).
    pub since: u32,
    /// 1 when sensed, decaying linearly to 0 over the perceiver's memory.
    pub strength: f32,
}

/// Perception memory and the latest sight checks; complete snapshot state.
#[derive(Debug, Clone, Default, PartialEq, Component, Serialize, Deserialize)]
pub struct Perception {
    /// Remembered stimuli sorted by ID (anonymous noises by position after them).
    pub known: Vec<Known>,
    /// Latest sight outcomes for candidates within twice the sight range, by ID.
    pub checks: Vec<(u64, SightOutcome)>,
}
impl Perception {
    /// Memory of a stimulus ID.
    pub fn get(&self, id: u64) -> Option<&Known> {
        self.known
            .binary_search_by_key(&id, |k| k.id)
            .ok()
            .map(|i| &self.known[i])
    }
    /// Latest sight outcome for a stimulus ID, if it was a candidate this tick.
    pub fn check(&self, id: u64) -> Option<SightOutcome> {
        self.checks
            .binary_search_by_key(&id, |c| c.0)
            .ok()
            .map(|i| self.checks[i].1)
    }
}

/// A noise lasting one tick. Gameplay pushes it (gunshot, footstep, thrown
/// bottle) before the perception system runs; perception clears the queue.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Noise {
    /// Emitting stimulus ID, or 0 when anonymous.
    pub source: u64,
    /// Sound position in metres.
    pub position: [f32; 3],
    /// Audible radius in metres for hearing 1.
    pub radius: f32,
    /// Kind hash of the source (see [`kind_hash`]).
    pub kind: u32,
}

/// Noises pushed during the current tick, consumed (and cleared) by `sense`.
/// Empty at every tick boundary, so it never carries state across a snapshot.
#[derive(Debug, Default, Resource)]
pub struct NoiseEvents {
    pub(crate) events: Vec<Noise>,
}
impl NoiseEvents {
    /// Queues a noise for this tick's perception update.
    pub fn push(&mut self, noise: Noise) {
        self.events.push(noise);
    }
    /// Noises queued so far this tick.
    pub fn pending(&self) -> &[Noise] {
        &self.events
    }
}

/// Stable 32-bit FNV-1a hash of a stimulus kind name.
pub fn kind_hash(kind: &str) -> u32 {
    kind.bytes().fold(0x811c_9dc5u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    })
}

/// Registers the reflected perception components with the Schema DB.
pub fn register(db: &mut SchemaDb) {
    db.register::<Perceiver>().register::<Stimulus>();
}
