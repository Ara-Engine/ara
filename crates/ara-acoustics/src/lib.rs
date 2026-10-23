//! Physical game acoustics for Ara, layered on `ara-audio`.
//!
//! - [`geometry`]: the narrow [`RayScene`] seam (physics adapter behind `physics`) and a
//!   BVH [`MeshScene`] for baking and tools.
//! - [`occlusion`]: volumetric occlusion and per-band material transmission.
//! - [`propagation`]: distance curves, air absorption and Doppler ([`AcousticSource`]).
//! - [`room`]: [`ReverbZone`]s, [`AcousticPortal`]s and the Eyring room model.
//! - [`reverb`]: the listener's room reverb (early reflections + FDN) as an aux effect.
//! - [`bake`]: Steam Audio or geometric probe baking into per-cell cooked data.
//! - [`Acoustics`]: the per-update pass tying them together.
//!
//! Everything runs on control/worker threads except [`RoomReverb`], which renders inside
//! the audio graph without allocating. All results are deterministic for golden captures.

mod acoustics;
pub mod bake;
pub mod geometry;
mod material;
mod math;
pub mod occlusion;
#[cfg(feature = "physics")]
pub mod physics;
pub mod propagation;
pub mod reverb;
pub mod room;

pub use acoustics::{Acoustics, Emitter, Evaluation, PROBE_RADIUS};
pub use bake::{
    AcousticCell, CellKey, GeometricBaker, ProbeField, ReverbBaker, ReverbProbe, SteamBaker,
};
pub use geometry::{MeshScene, RayHit, RayHits, RayScene, Segment};
pub use material::{AcousticMaterial, AcousticSurface, MaterialPreset};
pub use propagation::{AcousticSource, AcousticsSettings};
pub use reverb::{EarlyTap, ReverbParams, RoomReverb, RoomReverbHandle, room_reverb};
pub use room::{AcousticPortal, ReverbZone, RoomAcoustics, ZoneSet, ZoneShape};

use ara_schema::SchemaDb;

/// Seconds over which the room reverb crossfades to a new room's network.
pub(crate) const ROOM_CROSSFADE_SECONDS: f32 = 0.25;

/// Registers the acoustics components and value types in the Schema DB.
pub fn register(db: &mut SchemaDb) {
    db.register::<AcousticSource>()
        .register::<AcousticSurface>()
        .register::<ReverbZone>()
        .register::<AcousticPortal>()
        .register::<AcousticsSettings>()
        .register::<AcousticMaterial>();
}
