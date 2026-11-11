//! Deterministic render-v3 change feed, independent of GPU objects and gameplay state.
//!
//! Call [`ChangeFeed::begin_frame`], submit changed snapshots/removals, then [`ChangeFeed::finish`].
//! IDs are caller-owned u64 identities (native upload uses publication-local rows). Generations
//! start at one, advance on effective changes, and survive removal/reinsertion. A frame coalesces
//! changes per identity, retains its first previous state, and sorts by ID. Intermediate motion
//! remains in swept unions and summed displacement bounds. Even A -> B -> A produces an event.
//! Existing identities and warmed event capacities allocate nothing. No wall clock or randomness.
//!
//! **A1:** `feed.instance(id)` returns the current stamped transform; `finish().instance(id)`
//! returns this frame's transform, generation, swept bounds and maximum point displacement.
//! Geometry and material stamps are independent. `TransportChanges::reset` invalidates all
//! publication-local identities before the records. Physics-active bodies never own links;
//! static/sleeping bodies do. Kinematic bodies remain receivers while animated. Size is the
//! largest current world AABB dimension, with >= 0.5 m classified large.
//!
//! **Host obligations:** supply actual physics motion/sleep state, declared world-space wind
//! amplitude, stable light IDs and portal openness where available. Motion alone is never proof
//! of sleep. Bounds support the paths documented in [`motion_bounds`]; nonlinear root motion
//! requires subdivision. Vertex revisions need a conservative local displacement. Wind is an
//! absolute amplitude, so one frame can move by 2A. Sun events include angular delta and elevation;
//! height * angle is NOT a receiver-projected shadow bound. Consumers must include receiver
//! height, azimuth and the low-elevation 1/sin² factor, accumulate since page draw, and retain debt.
//!
//! # Replay file v1 (stable, little-endian, no native struct dumps)
//!
//! [`ReplayWriter`] / [`ReplayReader`] accept `Write` / `Read`; use buffered files for runs.
//! Header: 8 bytes `ARACHG01`, u32 version=1, u32 reserved=0. Repeated frames: u64 frame,
//! u64 payload byte length, then payload: u8 reset, five u32 counts (instances, geometries,
//! materials, lights, portals), u8 sun-present. Arrays follow in that order, then optional sun.
//! Each change: u64 id, u64 previous-generation, u64 generation, u32 flags, u8 presence
//! (bit 0 previous, bit 1 current), then previous/current snapshots when present. Instance
//! changes append swept AABB (6 f32) and maximum displacement (f32). Sun uses the same change
//! header with id=0 and appends angular delta and current elevation (2 f32).
//!
//! Snapshot field order (packed, no padding; bool/enums u8, floats IEEE-754 f32):
//! - instance: u64 entity, u64 geometry, u64 material, 16 f32 world columns, AABB (min xyz,
//!   max xyz), u8 body (0 static, 1 kinematic, 2 physics-active, 3 sleeping), u8 casts-shadows,
//!   f32 wind amplitude. Size and ownership derive from this snapshot.
//! - geometry: 32-byte content key, u64 vertex revision, AABB, f32 local vertex displacement.
//! - material: 80 u32 words in the native 320-byte material ABI (float words retain their bits).
//! - light: 28 u32 words in the native 112-byte light ABI (last four are table indices).
//! - portal: f32 openness [0,1]. Sun: direction xyz, lux xyz (6 f32).
//!
//! Flags: added=1, removed=2, moved=4, shape=8, recoloured=16, intensity/flicker=32,
//! deformation=64, body=128, sleep=256, wake=512, paint=1024, wind=2048, portal=4096,
//! material=8192, decals=16384. Unknown flags/enums, noncanonical order, invalid lengths
//! and truncation are rejected. EOF only between frames is clean. Caller-chosen byte limits
//! bound reader memory. Frames are strictly increasing.
//!
//! Replay reproduces the event source with [`ChangeFeed::apply_replay`] without recomputing
//! bounds/stamps. The first frame must be a reset/full snapshot. Mesh/texture payloads are external:
//! retain the scene asset set and geometry content-key/vertex-revision payloads to reproduce
//! raster/BLAS geometry too. This is a feed recording, not a scene archive. Capped and uncapped
//! consumers receive identical events; the feed never drops events to fit a cost cap.

mod bounds;
mod feed;
mod replay;
#[cfg(test)]
mod tests;
pub use bounds::{Aabb, MotionBounds, motion_bounds};
pub use feed::ChangeFeed;
use glam::{Mat4, Vec3};
pub use replay::{ReplayReader, ReplayWriter};

/// Cause bits can be combined; radiometric-only changes do not invalidate visibility.
pub mod flags {
    /// Identity appeared.
    pub const ADDED: u32 = 1;
    /// Identity disappeared; generation is a tombstone.
    pub const REMOVED: u32 = 2;
    /// Position/orientation changed.
    pub const MOVED: u32 = 4;
    /// Shape, range, topology, profile or sampling policy changed.
    pub const SHAPE: u32 = 8;
    /// Linear colour changed.
    pub const RECOLOURED: u32 = 16;
    /// Intensity changed, including flicker/switch-off.
    pub const INTENSITY: u32 = 32;
    /// Vertex revision changed.
    pub const DEFORMED: u32 = 64;
    /// Body ownership class changed.
    pub const BODY: u32 = 128;
    /// Body entered sleep and can acquire links.
    pub const SLEEP: u32 = 256;
    /// Sleeping body woke and must relinquish links.
    pub const WAKE: u32 = 512;
    /// Per-instance paint changed.
    pub const PAINT: u32 = 1024;
    /// Wind can displace vertices this frame.
    pub const WIND: u32 = 2048;
    /// Portal openness changed.
    pub const PORTAL: u32 = 4096;
    /// Material parameters changed.
    pub const MATERIAL: u32 = 8192;
    /// Decals changed; conservatively invalidate appearance across the scene.
    pub const DECALS: u32 = 16384;
    pub(crate) const ALL: u32 = 32767;
}

/// Derived render ownership, not authoritative physics state.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BodyClass {
    /// Immobile authored geometry owns links.
    #[default]
    Static = 0,
    /// Kinematic or skinned geometry receives light while animated.
    Kinematic = 1,
    /// Simulating bodies own no links.
    PhysicsActive = 2,
    /// Sleeping bodies can own links until wake.
    Sleeping = 3,
}
impl BodyClass {
    /// Whether persistent transport links can belong to this body.
    pub fn owns_links(self) -> bool {
        matches!(self, Self::Static | Self::Sleeping)
    }
}

/// One instance's complete cache-relevant snapshot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InstanceState {
    /// Stable external entity/primitive identity.
    pub entity: u64,
    /// Feed geometry identity.
    pub geometry: u64,
    /// Feed material identity.
    pub material: u64,
    /// Affine local-to-world transform.
    pub world: Mat4,
    /// Current local geometry bounds.
    pub bounds: Aabb,
    /// Actual host ownership class.
    pub body: BodyClass,
    /// Whether it occludes light.
    pub casts_shadows: bool,
    /// Declared absolute world-space wind displacement, metres; zero when absent.
    pub wind: f32,
}
impl InstanceState {
    /// At least two 25 cm patches across in its largest world dimension.
    pub fn is_large(self) -> bool {
        self.world_bounds().extent().max_element() >= 0.5
    }
    /// Current world bounds including wind amplitude.
    pub fn world_bounds(self) -> Aabb {
        self.bounds.transformed(self.world).expanded(self.wind)
    }
}

/// Geometry/BLAS identity and bound measured at a vertex revision.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeometryState {
    /// Source mesh key; pose payloads pair this with `revision`.
    pub key: [u8; 32],
    /// Host vertex revision, independent of feed generation.
    pub revision: u64,
    /// Model-space bounds.
    pub bounds: Aabb,
    /// Maximum local point displacement from the preceding revision.
    pub displacement: f32,
}
/// Native material bits retained verbatim, without colour quantization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaterialState(pub [u32; 80]);
impl From<&crate::instanced::Material> for MaterialState {
    fn from(value: &crate::instanced::Material) -> Self {
        let words: &[u32] = bytemuck::cast_slice(std::slice::from_ref(value));
        Self(std::array::from_fn(|i| words[i]))
    }
}
/// Native light bits; integer table offsets remain integers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LightState(pub [u32; 28]);
// Changing a native ABI requires a deliberate replay version/encoding decision.
const _: () = assert!(std::mem::size_of::<crate::instanced::Material>() == 320);
const _: () = assert!(std::mem::size_of::<crate::scene::Light>() == 112);
impl From<&crate::scene::Light> for LightState {
    fn from(value: &crate::scene::Light) -> Self {
        let words: &[u32] = bytemuck::cast_slice(std::slice::from_ref(value));
        Self(std::array::from_fn(|i| words[i]))
    }
}
/// Optical portal state; acoustic portals are not implicitly optical portals.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PortalState {
    /// Zero closed, one fully open.
    pub openness: f32,
}
/// Sun direction and linear illuminance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SunState {
    /// Unit direction toward the sun, Y up.
    pub direction: Vec3,
    /// Linear RGB lux.
    pub lux: Vec3,
}
/// Current snapshot and cache validation stamp.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stamped<T> {
    /// Zero is never live.
    pub generation: u64,
    /// Cache-relevant value.
    pub value: T,
}
/// One coalesced identity change, including tombstones.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Change<T> {
    /// Stable feed identity until a reset.
    pub id: u64,
    /// Stamp before this frame (zero if never seen).
    pub previous_generation: u64,
    /// Stamp after this frame's changes.
    pub generation: u64,
    /// Combined [`flags`], including intermediate changes.
    pub flags: u32,
    /// Frame-start value, absent for a new/previously removed identity.
    pub previous: Option<T>,
    /// Frame-end value, absent after removal.
    pub current: Option<T>,
}
/// Instance change and conservative motion over all updates this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InstanceChange {
    /// Identity, generations and endpoints.
    pub change: Change<InstanceState>,
    /// Swept world box, including skin and wind.
    pub swept: Aabb,
    /// Bound on maximum point displacement over endpoint updates.
    pub displacement: f32,
}
/// Sun change with receiver-invalidation inputs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SunChange {
    /// Direction/illuminance endpoints and stamp.
    pub change: Change<SunState>,
    /// Sum of angular movement over updates, radians.
    pub angular_delta: f32,
    /// Current elevation, radians; negative below horizon.
    pub elevation: f32,
}
/// One deterministic frame with reusable event storage.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TransportChanges {
    /// Strictly increasing render frame; simulation ticks may repeat.
    pub frame: u64,
    /// Invalidate every publication-local identity before these records.
    pub reset: bool,
    /// Sorted instance records.
    pub instances: Vec<InstanceChange>,
    /// Sorted geometry records.
    pub geometries: Vec<Change<GeometryState>>,
    /// Sorted material records.
    pub materials: Vec<Change<MaterialState>>,
    /// Sorted light records.
    pub lights: Vec<Change<LightState>>,
    /// Sorted portal records.
    pub portals: Vec<Change<PortalState>>,
    /// At most one sun record.
    pub sun: Option<SunChange>,
}
impl TransportChanges {
    /// Copy a frame into retained storage. Derived `Clone::clone_from` would replace all vectors.
    pub fn copy_from(&mut self, source: &Self) {
        self.frame = source.frame;
        self.reset = source.reset;
        self.instances.clone_from(&source.instances);
        self.geometries.clone_from(&source.geometries);
        self.materials.clone_from(&source.materials);
        self.lights.clone_from(&source.lights);
        self.portals.clone_from(&source.portals);
        self.sun = source.sun;
    }
    /// Clear events, retaining capacity.
    pub fn clear(&mut self, frame: u64) {
        self.frame = frame;
        self.reset = false;
        self.instances.clear();
        self.geometries.clear();
        self.materials.clear();
        self.lights.clear();
        self.portals.clear();
        self.sun = None;
    }
    /// A1's sorted frame lookup, valid after `ChangeFeed::finish`.
    pub fn instance(&self, id: u64) -> Option<&InstanceChange> {
        self.instances
            .binary_search_by_key(&id, |c| c.change.id)
            .ok()
            .map(|i| &self.instances[i])
    }
}
