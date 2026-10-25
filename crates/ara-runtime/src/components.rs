//! Reflected runtime components: presentation (meshes, lights, cameras, sky),
//! gameplay drivers (player control, wandering agents) and animation. Every field
//! is authoring data or explicit simulation state; nothing here holds handles.
use ara_schema::{AssetRef, Color, EntityRef, Schema, SchemaDb};
use bevy_ecs::prelude::Component;
use serde::{Deserialize, Serialize};

/// Built-in procedural geometry for gray-box levels.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(module = "render")]
#[serde(rename_all = "camelCase")]
pub enum MeshShape {
    /// Axis-aligned box with `size` extents.
    #[default]
    Box,
    /// Sphere with diameter `size.x`.
    Sphere,
    /// Upright capsule with diameter `size.x` and total height `size.y`.
    Capsule,
    /// Upward-facing quad spanning `size.x` by `size.z`.
    Plane,
}

/// One native material graph/instance replacing a selected model primitive's material.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "render")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrimitiveMaterialOverride {
    /// Zero-based primitive within the selected subtree, before `primitive` filtering.
    // The canonical writer omits the default slot 0, so reading must default it.
    #[serde(default)]
    pub primitive: u32,
    /// Authoritative material graph or material-instance asset (`@path#id`).
    pub material: AssetRef,
}

/// Draws geometry at the entity's interpolated world transform.
#[derive(Debug, Clone, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "render")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct MeshRenderer {
    /// Procedural shape, drawn when `model` is empty.
    #[ara(editor(show_if(field = "model", empty)))]
    pub shape: MeshShape,
    /// Shape size: box extents, sphere/capsule diameter in x and capsule height in y,
    /// plane extents in x and z.
    #[ara(default = [1, 1, 1], unit = Meters, range(min = 0.0001), editor(show_if(field = "model", empty)))]
    pub size: [f32; 3],
    /// Imported model (`@path#id` of a glTF/FBX source). Its meshes are drawn instead
    /// of the shape; skinned meshes follow this entity's Animator.
    // Migration: early editor documents wrote `mesh`; reading renames it.
    #[ara(alias = "mesh")]
    pub model: Option<AssetRef>,
    /// Source node name or numeric glTF index. `name:3` selects a numeric name.
    /// The drawable subtree retains source rotation/scale and restores its pivot
    /// to the entity origin. Absent selects the complete default scene.
    #[ara(editor(show_if(field = "model", set)))]
    pub node: Option<String>,
    /// Zero-based primitive within the selected subtree (or whole model).
    #[ara(editor(show_if(field = "model", set)))]
    pub primitive: Option<u32>,
    /// Native material assets by subtree-local primitive slot; omitted slots use the model.
    pub material_overrides: Vec<PrimitiveMaterialOverride>,
    /// Linear base color multiplier and opacity.
    // Migration: early editor documents wrote `tint`; reading renames it.
    #[ara(alias = "tint")]
    pub color: Color,
    /// Perceptual roughness.
    #[ara(default = 0.6, range(min = 0, max = 1))]
    pub roughness: f32,
    /// Metalness.
    #[ara(range(min = 0, max = 1))]
    pub metallic: f32,
    /// Default procedural radiance, or an RGB emission multiplier with a model/material asset.
    /// Models treat zero RGB as neutral; use `emissiveEnabled` to switch them off.
    #[ara(editor(color))]
    pub emissive: [f32; 3],
    /// Enables emission without modifying the source model or its materials.
    #[ara(default = true)]
    pub emissive_enabled: bool,
    /// Emission multiplier, applied once after the source or overridden factor.
    #[ara(default = 1, range(min = 0))]
    pub emissive_strength: f32,
    /// Optional linear RGB replacement for the source emissive factor, useful for lamps.
    #[ara(range(min = 0), editor(color))]
    pub emissive_factor: Option<[f32; 3]>,
    /// Source material names or numeric indices affected by `emissiveFactor`.
    /// Empty targets all; `name:3` disambiguates a numeric material name.
    pub emissive_materials: Vec<String>,
}
impl Default for MeshRenderer {
    fn default() -> Self {
        Self {
            shape: MeshShape::Box,
            size: [1.0; 3],
            model: None,
            node: None,
            primitive: None,
            material_overrides: Vec::new(),
            color: Color([1.0; 4]),
            roughness: 0.6,
            metallic: 0.0,
            emissive: [0.0; 3],
            emissive_enabled: true,
            emissive_strength: 1.0,
            emissive_factor: None,
            emissive_materials: Vec::new(),
        }
    }
}

impl MeshRenderer {
    /// Whether a source material receives this renderer's optional emission factor.
    pub fn targets_emissive_material(&self, index: usize, name: &str) -> bool {
        self.emissive_materials.is_empty()
            || self
                .emissive_materials
                .iter()
                .any(|selector| matches_emissive_material(selector, index, name))
    }
}

pub(crate) fn matches_emissive_material(selector: &str, index: usize, name: &str) -> bool {
    if let Some(name_selector) = selector.strip_prefix("name:") {
        name == name_selector
    } else if let Ok(selected) = selector.parse::<usize>() {
        index == selected
    } else {
        name == selector
    }
}

/// How a light emits.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(module = "render")]
#[serde(rename_all = "camelCase")]
pub enum LightKind {
    /// Omnidirectional point light.
    #[default]
    Point,
    /// Cone light aimed along the entity's local -Z axis.
    Spot,
    /// Sun-like light shining along the entity's local -Z axis; casts cascaded shadows.
    Directional,
}

/// A light at the entity's world position. Edits apply live in play.
#[derive(Debug, Clone, Copy, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "render")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Light {
    /// Point, spot or directional.
    pub kind: LightKind,
    /// Linear RGB tint (alpha is ignored).
    pub color: Color,
    /// Luminous flux in lumens (point/spot) or illuminance in lux (directional).
    #[ara(default = 800, range(min = 0))]
    pub intensity: f32,
    /// Influence radius of point and spot lights.
    #[ara(default = 10, unit = Meters, range(min = 0.01), editor(show_if(field = "kind", one_of = ["point", "spot"])))]
    pub range: f32,
    /// Spot inner cone half-angle.
    #[ara(default = 20, unit = Degrees, range(min = 0, max = 89), editor(show_if(field = "kind", one_of = ["spot"])))]
    pub inner_angle: f32,
    /// Spot outer cone half-angle.
    #[ara(default = 30, unit = Degrees, range(min = 0.1, max = 89), editor(show_if(field = "kind", one_of = ["spot"])))]
    pub outer_angle: f32,
}
impl Default for Light {
    fn default() -> Self {
        Self {
            kind: LightKind::Point,
            color: Color([1.0; 4]),
            intensity: 800.0,
            range: 10.0,
            inner_angle: 20.0,
            outer_angle: 30.0,
        }
    }
}

/// Procedural sky radiance and camera exposure. The first in stable-ID order wins.
#[derive(Debug, Clone, Copy, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "render")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Environment {
    /// Zenith sky color (linear).
    #[ara(default = [0.22, 0.32, 0.6, 1])]
    pub sky: Color,
    /// Horizon sky color (linear).
    #[ara(default = [0.95, 0.62, 0.4, 1])]
    pub horizon: Color,
    /// Ground color seen below the horizon (linear).
    #[ara(default = [0.18, 0.16, 0.14, 1])]
    pub ground: Color,
    /// Sky radiance scale in candela per square metre.
    #[ara(default = 900, unit = Candela, range(min = 0))]
    pub intensity: f32,
    /// Linear camera exposure multiplier, applied before tone mapping.
    #[ara(default = 0.0015, range(min = 0.0000001))]
    pub exposure: f32,
}
impl Default for Environment {
    fn default() -> Self {
        Self {
            sky: Color([0.22, 0.32, 0.6, 1.0]),
            horizon: Color([0.95, 0.62, 0.4, 1.0]),
            ground: Color([0.18, 0.16, 0.14, 1.0]),
            intensity: 900.0,
            exposure: 0.0015,
        }
    }
}

/// A viewpoint looking along the entity's local -Z axis. The lowest `priority` wins.
#[derive(Debug, Clone, Copy, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "render")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Camera {
    /// Vertical field of view.
    #[ara(default = 55, unit = Degrees, range(min = 1, max = 170))]
    pub fov: f32,
    /// Selection order when several cameras exist; lower is preferred.
    pub priority: i32,
}
impl Default for Camera {
    fn default() -> Self {
        Self {
            fov: 55.0,
            priority: 0,
        }
    }
}

/// Third-person orbit rig: places this entity behind `target`, steered by `Look`.
#[derive(Debug, Clone, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "gameplay")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct FollowCamera {
    /// Followed entity.
    pub target: Option<EntityRef>,
    /// Orbit distance from the aim point; the boom stops in front of static box colliders.
    #[ara(default = 5, unit = Meters, range(min = 0.1))]
    pub distance: f32,
    /// Aim point height above the target's origin.
    #[ara(default = 0.8, unit = Meters)]
    pub height: f32,
    /// Orbit yaw; 0 looks along +Z. Simulation state driven by `Look`.
    #[ara(unit = Degrees)]
    pub yaw: f32,
    /// Orbit pitch above the horizon; simulation state driven by `Look`.
    #[ara(default = 20, unit = Degrees, range(min = -80, max = 80))]
    pub pitch: f32,
    /// Look sensitivity: degrees per tick at full axis deflection.
    #[ara(default = 2.5, unit = Degrees, range(min = 0))]
    pub sensitivity: f32,
    /// Input owner whose `Look` action steers the rig.
    #[ara(default = 1)]
    pub player: u64,
}
impl Default for FollowCamera {
    fn default() -> Self {
        Self {
            target: None,
            distance: 5.0,
            height: 0.8,
            yaw: 0.0,
            pitch: 20.0,
            sensitivity: 2.5,
            player: 1,
        }
    }
}

/// Drives this entity's CharacterController from `Move`, `Sprint` and `Jump`,
/// relative to its camera rig's yaw.
#[derive(Debug, Clone, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "gameplay")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct PlayerControl {
    /// Input owner (`PlayerId`).
    #[ara(default = 1)]
    pub player: u64,
    /// Walking speed.
    #[ara(default = 1.6, unit = MetersPerSecond, range(min = 0))]
    pub walk_speed: f32,
    /// Sprinting speed.
    #[ara(default = 4.5, unit = MetersPerSecond, range(min = 0))]
    pub run_speed: f32,
    /// Jump launch speed.
    #[ara(default = 4.5, unit = MetersPerSecond, range(min = 0))]
    pub jump_speed: f32,
    /// Maximum turn rate toward the movement direction.
    #[ara(default = 540, unit = DegreesPerSecond, range(min = 0))]
    pub turn_rate: f32,
    /// Facing yaw; 0 faces +Z. Simulation state.
    #[ara(unit = Degrees)]
    pub yaw: f32,
    /// Camera rig whose yaw orients `Move`; world axes when empty.
    pub camera: Option<EntityRef>,
}
impl Default for PlayerControl {
    fn default() -> Self {
        Self {
            player: 1,
            walk_speed: 1.6,
            run_speed: 4.5,
            jump_speed: 4.5,
            turn_rate: 540.0,
            yaw: 0.0,
            camera: None,
        }
    }
}

/// Server-side AI: walks the navmesh to random points inside a box, pausing between.
#[derive(Debug, Clone, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "gameplay")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Wanderer {
    /// Minimum corner of the roaming box (world space).
    #[ara(unit = Meters)]
    pub min: [f32; 3],
    /// Maximum corner of the roaming box (world space).
    #[ara(unit = Meters)]
    pub max: [f32; 3],
    /// Ticks to wait after arriving before choosing the next goal.
    #[ara(default = 45)]
    pub pause_ticks: u32,
    /// Tick at which the next goal is chosen. Simulation state.
    pub resume_tick: u64,
    /// Goals chosen so far. Simulation state.
    pub goals: u32,
}
impl Default for Wanderer {
    fn default() -> Self {
        Self {
            min: [0.0; 3],
            max: [0.0; 3],
            pause_ticks: 45,
            resume_tick: 0,
            goals: 0,
        }
    }
}

/// One clip of a locomotion blend and the ground speed it represents.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct LocomotionClip {
    /// Animation name inside the model (the glTF animation name).
    pub clip: String,
    /// Ground speed at which this clip has full weight.
    #[ara(unit = MetersPerSecond, range(min = 0))]
    pub speed: f32,
}
impl Default for LocomotionClip {
    fn default() -> Self {
        Self {
            clip: String::new(),
            speed: 0.0,
        }
    }
}

/// Skeletal animation for this entity's `MeshRenderer.model`, compiled to an ara-anim
/// program: the `controller` document when set, else an `idle` state and a `move`
/// state whose 1D blend tree is driven by the entity's ground speed, cross-faded at
/// `moveThreshold`.
#[derive(Debug, Clone, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Animator {
    /// Controller authored in an `.animation` document (`@anim/fox.animation#an_fox`):
    /// its state machine, blend trees and motion matching replace the built-in
    /// idle/locomotion graph below. A float `speed` parameter follows ground speed.
    #[ara(asset_kind = "animation")]
    pub controller: Option<AssetRef>,
    /// Idle clip name.
    pub idle: String,
    /// Moving blend samples in increasing speed order.
    pub locomotion: Vec<LocomotionClip>,
    /// Ground speed separating idle from moving.
    #[ara(default = 0.15, unit = MetersPerSecond, range(min = 0))]
    pub move_threshold: f32,
    /// Cross-fade duration between idle and moving.
    #[ara(default = 0.2, unit = Seconds, range(min = 0))]
    pub fade: f32,
    /// Moves the entity by the extracted root motion (only without a character
    /// controller or navigation agent, which own movement).
    pub root_motion: bool,
}
impl Default for Animator {
    fn default() -> Self {
        Self {
            controller: None,
            idle: String::new(),
            locomotion: Vec::new(),
            move_threshold: 0.15,
            fade: 0.2,
            root_motion: false,
        }
    }
}

/// Streams a `.world`'s cells into play around the players (`ara-runtime-world`).
/// Put it on one entity of the level scene; the cell documents stay out of the
/// projection and arrive at tick boundaries. With a `Terrain`, navigation then
/// follows the resident `TerrainChunk`s.
#[derive(Debug, Clone, Default, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "world")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct StreamedWorld {
    /// The world manifest (`.world` document).
    #[ara(asset_kind = "world")]
    pub world: Option<AssetRef>,
}

/// Plays a `.music` document's interactive score (segments, transitions, intensity
/// layers) from this entity, non-spatial. Presentation intent: gameplay writes
/// `intensity` and `segment`; the audio presenter follows them.
#[derive(Debug, Clone, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "audio")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct MusicSource {
    /// The score (`.music` document).
    #[ara(asset_kind = "music")]
    pub music: Option<AssetRef>,
    /// Mixer bus the stems play on.
    #[ara(default = "music")]
    pub bus: String,
    /// Intensity from 0 to 1: stems whose range contains it are audible.
    #[ara(range(min = 0, max = 1))]
    pub intensity: f32,
    /// Segment to transition to (on the score's sync rule); empty follows the score.
    pub segment: String,
}
impl Default for MusicSource {
    fn default() -> Self {
        Self {
            music: None,
            bus: "music".into(),
            intensity: 0.0,
            segment: String::new(),
        }
    }
}

/// Plays a procedural audio graph (an ara-audio-graph `graph` document, domain
/// `audio`) from this entity, spatialized and attenuated by its `AudioSource` (whose
/// clip stays empty). Gameplay writes `parameters` and bumps `triggers`; the audio
/// presenter forwards changes sample-accurately at the next control frame.
#[derive(Debug, Clone, Default, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "audio")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct AudioGraphSource {
    /// The audio graph document.
    #[ara(asset_kind = "graph")]
    pub graph: Option<AssetRef>,
    /// Parameter values by name (clamped to the graph's declared ranges).
    pub parameters: std::collections::BTreeMap<String, f32>,
    /// Trigger counters by name: each increment fires the trigger once.
    pub triggers: std::collections::BTreeMap<String, u32>,
}

/// A GPU particle effect drawn at this entity. Presentation only: gameplay reads
/// ara-vfx CPU events, never GPU particles. `effect` names a cooked library effect
/// (`campfire`, `sparks`, `depth_sparks`, `magic_swirl`, `smoke`, `muzzle_flash`,
/// `rain`, `dust`).
#[derive(Debug, Clone, PartialEq, Component, Schema, Serialize, Deserialize)]
#[ara(component, module = "vfx")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct VisualEffect {
    /// Cooked library effect name.
    #[ara(default = "campfire")]
    pub effect: String,
    /// Uniform size: the effect's shape, sprite sizes and speeds grow together
    /// (2 doubles a campfire's height and spread).
    #[ara(default = 1, range(min = 0.05, max = 50))]
    pub scale: f32,
    /// Brightness multiplier on the effect's emitted and lit colour; its opacity is
    /// unchanged. Lower it for effects that glow too hot under a scene's exposure.
    #[ara(default = 1, range(min = 0, max = 50))]
    pub intensity: f32,
}
impl Default for VisualEffect {
    fn default() -> Self {
        Self {
            effect: "campfire".into(),
            scale: 1.0,
            intensity: 1.0,
        }
    }
}

/// Registers the components the runtime simulates itself (physics, navigation and
/// the runtime's own). Part of [`crate::engine::register_schemas`]. Idempotent.
pub fn register_schemas(db: &mut SchemaDb) {
    ara_physics::register_schemas(db);
    ara_nav::register(db);
    db.register::<PrimitiveMaterialOverride>()
        .register::<MeshRenderer>()
        .register::<Light>()
        .register::<Environment>()
        .register::<Camera>()
        .register::<FollowCamera>()
        .register::<PlayerControl>()
        .register::<Wanderer>()
        .register::<Animator>()
        .register::<VisualEffect>()
        .register::<StreamedWorld>()
        .register::<MusicSource>()
        .register::<AudioGraphSource>()
        .register::<crate::terrain::Terrain>()
        .register::<crate::terrain::TerrainChunk>();
}
