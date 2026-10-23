use crate::Stage;
use ara_material::Domain;
use serde::{Deserialize, Serialize};

/// Scalar curve key, interpolated linearly with endpoint clamping.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Key {
    /// Normalized particle lifetime in [0,1].
    pub t: f32,
    /// Value at this key.
    pub value: f32,
}
/// Scene-linear color gradient key.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColorKey {
    /// Normalized particle lifetime in [0,1].
    pub t: f32,
    /// Straight linear RGBA; premultiplication happens at output.
    pub value: [f32; 4],
}
/// Visual sort policy. Stable slot/spawn order is always the tie breaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sorting {
    /// Stable compacted slot order, ideal for additive effects.
    None,
    /// Back-to-front camera depth with stable slot-ID ties.
    Depth,
    /// Oldest particles first.
    Age,
}
/// Event provenance; GPU events never drive authoritative gameplay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// Particle expires at its configured lifetime.
    Death,
    /// Particle hits a plane or scene depth.
    Collision,
}
/// Typed built-in VFX modules. Each variant lowers to Slang and CPU math.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Module {
    /// Textured output, blending and velocity shutter. Does not change simulation.
    Appearance {
        /// Offline procedural sprite sheet, blended between adjacent frames.
        texture: crate::SpriteTexture,
        /// Premultiplied alpha or additive radiance.
        blend: crate::Blend,
        /// Camera, velocity or ground-plane alignment.
        alignment: crate::Alignment,
        /// Minimum length-to-width ratio, at least one.
        stretch: f32,
        /// Exposed fraction of a fixed tick, from zero through one.
        shutter: f32,
    },
    /// Emit particles per second using fixed-tick accumulated demand.
    Rate {
        /// Particles per second.
        per_second: f32,
    },
    /// Emit an exact count once at a fixed tick.
    Burst {
        /// Fixed tick of the burst.
        tick: u32,
        /// Requested particles.
        count: u32,
    },
    /// Set lifetime, initial diameter and color.
    Lifetime {
        /// Seconds until death.
        seconds: f32,
        /// Initial world diameter.
        size: f32,
        /// Initial straight linear color.
        color: [f32; 4],
    },
    /// Place particles at one position.
    Point {
        /// World-space position.
        position: [f32; 3],
    },
    /// Uniform volume sampling inside a sphere.
    Sphere {
        /// World-space centre.
        center: [f32; 3],
        /// Sphere radius in metres.
        radius: f32,
    },
    /// Uniform volume sampling inside an axis-aligned box.
    Box {
        /// World-space centre.
        center: [f32; 3],
        /// Positive box half extents.
        extent: [f32; 3],
    },
    /// Uniform volume sampling in an upward cone.
    Cone {
        /// Cone tip.
        origin: [f32; 3],
        /// Height in metres.
        height: f32,
        /// Base radius in metres.
        radius: f32,
    },
    /// Area-weighted triangle surface sampling; asset workers supply cooked triangles.
    MeshSurface {
        /// World-space triangles, bounded to 4096 per module.
        triangles: Vec<[[f32; 3]; 3]>,
    },
    /// Set velocity with independent symmetric random variation per axis.
    Velocity {
        /// Initial metres per second.
        value: [f32; 3],
        /// Symmetric per-axis half range.
        spread: [f32; 3],
    },
    /// Constant acceleration for the next tick's displacement.
    Gravity {
        /// Metres per second squared.
        acceleration: [f32; 3],
    },
    /// Stable rational drag: velocity /= 1 + coefficient * dt.
    Drag {
        /// Nonnegative inverse seconds.
        coefficient: f32,
    },
    /// Analytic divergence-free curl of a sinusoidal vector potential.
    CurlNoise {
        /// Inverse spatial wavelength.
        frequency: f32,
        /// Acceleration amplitude.
        strength: f32,
    },
    /// Accelerate around a normalized axis through the centre.
    Vortex {
        /// Rotation centre.
        center: [f32; 3],
        /// Nonzero rotation axis.
        axis: [f32; 3],
        /// Tangential acceleration multiplier.
        strength: f32,
    },
    /// Softened inverse-square attraction, avoiding singularities.
    Attractor {
        /// World target position.
        position: [f32; 3],
        /// Attraction strength.
        strength: f32,
        /// Positive minimum squared distance.
        softening: f32,
    },
    /// Replace color with a lifetime gradient.
    ColorOverLife {
        /// At least two keys, ordered by t.
        keys: Vec<ColorKey>,
    },
    /// Replace world diameter with a lifetime curve.
    SizeOverLife {
        /// At least two keys, ordered by t.
        keys: Vec<Key>,
    },
    /// Project out of a plane and reflect inward velocity.
    PlaneCollision {
        /// Nonzero outward plane normal.
        normal: [f32; 3],
        /// Plane equation dot(normalized normal,p) = offset.
        offset: f32,
        /// Bounce coefficient in [0,1].
        restitution: f32,
    },
    /// Collide with provided scene depth; CPU hosts must supply the same query.
    DepthCollision {
        /// Bounce coefficient in [0,1].
        restitution: f32,
        /// World-space contact band.
        thickness: f32,
    },
    /// Camera-facing billboards, soft intersections and a supported material domain.
    Sprite {
        /// Shared material contract: surface, unlit or ui.
        domain: Domain,
        /// Fade distance at depth intersections, zero disables.
        soft_distance: f32,
        /// Transparent sorting policy.
        sorting: Sorting,
    },
    /// Animated atlas addressing; coordinates are available to material evaluation.
    Flipbook {
        /// Atlas grid dimensions.
        grid: [u32; 2],
        /// Frames per second.
        fps: f32,
    },
    /// Join consecutive live slots into camera-facing ribbon segments.
    Ribbon {
        /// World-space ribbon width.
        width: f32,
        /// Shared material domain.
        domain: Domain,
    },
    /// Draw a cooked triangle mesh per particle, oriented to velocity.
    Mesh {
        /// Local-space triangles, maximum 4096.
        triangles: Vec<[[f32; 3]; 3]>,
        /// Shared material domain.
        domain: Domain,
        /// Transparent sorting policy.
        sorting: Sorting,
    },
    /// Export a bounded emitter light, independent of particle count.
    Light {
        /// Scene-linear RGB.
        color: [f32; 3],
        /// Luminous flux in lumens.
        lumens: f32,
        /// World-space attenuation radius.
        radius: f32,
    },
    /// Spawn cosmetic particles at event positions on the next tick.
    EventSpawn {
        /// Event to observe.
        trigger: Trigger,
        /// Destination emitter name.
        target: String,
        /// Children per event, capped by quality and capacity.
        count: u32,
    },
}
impl Module {
    /// Whether a namespaced parameter has the exact type of a bindable setting.
    /// Geometry, curves, domains and event destinations remain authored structure.
    pub fn accepts_binding(&self, field: &str, value: &crate::Value) -> bool {
        use crate::Value;
        matches!(
            (self, field, value),
            (Self::Rate { .. }, "per_second", Value::Float(_))
                | (Self::Burst { .. }, "tick" | "count", Value::Uint(_))
                | (Self::Lifetime { .. }, "seconds" | "size", Value::Float(_))
                | (Self::Lifetime { .. }, "color", Value::Color(_))
                | (Self::Point { .. }, "position", Value::Vec3(_))
                | (Self::Sphere { .. }, "center", Value::Vec3(_))
                | (Self::Sphere { .. }, "radius", Value::Float(_))
                | (Self::Box { .. }, "center" | "extent", Value::Vec3(_))
                | (Self::Cone { .. }, "origin", Value::Vec3(_))
                | (Self::Cone { .. }, "height" | "radius", Value::Float(_))
                | (Self::Velocity { .. }, "value" | "spread", Value::Vec3(_))
                | (Self::Gravity { .. }, "acceleration", Value::Vec3(_))
                | (Self::Drag { .. }, "coefficient", Value::Float(_))
                | (
                    Self::CurlNoise { .. },
                    "frequency" | "strength",
                    Value::Float(_)
                )
                | (Self::Vortex { .. }, "center" | "axis", Value::Vec3(_))
                | (Self::Vortex { .. }, "strength", Value::Float(_))
                | (Self::Attractor { .. }, "position", Value::Vec3(_))
                | (
                    Self::Attractor { .. },
                    "strength" | "softening",
                    Value::Float(_)
                )
                | (Self::PlaneCollision { .. }, "normal", Value::Vec3(_))
                | (
                    Self::PlaneCollision { .. },
                    "offset" | "restitution",
                    Value::Float(_)
                )
                | (
                    Self::DepthCollision { .. },
                    "restitution" | "thickness",
                    Value::Float(_)
                )
                | (Self::Sprite { .. }, "soft_distance", Value::Float(_))
                | (
                    Self::Appearance { .. },
                    "stretch" | "shutter",
                    Value::Float(_)
                )
                | (Self::Flipbook { .. }, "fps", Value::Float(_))
                | (Self::Ribbon { .. }, "width", Value::Float(_))
                | (Self::Light { .. }, "color", Value::Vec3(_))
                | (Self::Light { .. }, "lumens" | "radius", Value::Float(_))
                | (Self::EventSpawn { .. }, "count", Value::Uint(_))
        )
    }
    /// Required stack for this module, exposed to stack inspectors and agents.
    pub fn stage(&self) -> Stage {
        match self {
            Self::Rate { .. } | Self::Burst { .. } => Stage::Spawn,
            Self::Lifetime { .. }
            | Self::Point { .. }
            | Self::Sphere { .. }
            | Self::Box { .. }
            | Self::Cone { .. }
            | Self::MeshSurface { .. }
            | Self::Velocity { .. } => Stage::Initialize,
            Self::Gravity { .. }
            | Self::Drag { .. }
            | Self::CurlNoise { .. }
            | Self::Vortex { .. }
            | Self::Attractor { .. }
            | Self::ColorOverLife { .. }
            | Self::SizeOverLife { .. }
            | Self::PlaneCollision { .. }
            | Self::DepthCollision { .. } => Stage::Update,
            _ => Stage::Render,
        }
    }
    /// Stable module registry key.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Appearance { .. } => "appearance",
            Self::Rate { .. } => "rate",
            Self::Burst { .. } => "burst",
            Self::Lifetime { .. } => "lifetime",
            Self::Point { .. } => "point",
            Self::Sphere { .. } => "sphere",
            Self::Box { .. } => "box",
            Self::Cone { .. } => "cone",
            Self::MeshSurface { .. } => "mesh_surface",
            Self::Velocity { .. } => "velocity",
            Self::Gravity { .. } => "gravity",
            Self::Drag { .. } => "drag",
            Self::CurlNoise { .. } => "curl_noise",
            Self::Vortex { .. } => "vortex",
            Self::Attractor { .. } => "attractor",
            Self::ColorOverLife { .. } => "color_over_life",
            Self::SizeOverLife { .. } => "size_over_life",
            Self::PlaneCollision { .. } => "plane_collision",
            Self::DepthCollision { .. } => "depth_collision",
            Self::Sprite { .. } => "sprite",
            Self::Flipbook { .. } => "flipbook",
            Self::Ribbon { .. } => "ribbon",
            Self::Mesh { .. } => "mesh",
            Self::Light { .. } => "light",
            Self::EventSpawn { .. } => "event_spawn",
        }
    }
}
