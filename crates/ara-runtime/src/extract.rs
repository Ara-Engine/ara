//! Presentation extraction: a renderer-neutral [`RenderFrame`] built from the world
//! at an interpolation phase. Reads only; buffers are reused between frames.
use crate::{
    animation::{AnimatorSlot, Library},
    assets::asset_id,
    components::{Camera, Environment, Light, LightKind, MeshRenderer, MeshShape, VisualEffect},
    math::{Pose, stable_id},
    model::Model,
    sync::{PreviousPose, WorldPose},
};
use ara_sim::{Identity, SceneMetadata};
use bevy_ecs::prelude::World;
use glam::{Mat4, Vec3};
use std::{ops::Range, sync::Arc};

/// What a draw renders.
#[derive(Debug, Clone)]
pub enum Geometry {
    /// A procedural shape of a given size.
    Shape {
        /// Built-in shape.
        shape: MeshShape,
        /// Size in metres (see `MeshRenderer.size`).
        size: [f32; 3],
    },
    /// One primitive of a decoded model generation.
    Model {
        /// Live generation.
        model: Arc<Model>,
        /// Index into `Model::primitives`.
        primitive: usize,
    },
}

/// One drawable item at its interpolated world transform.
#[derive(Debug, Clone)]
pub struct Draw {
    /// Stable render identity (`stable_id` of the address, plus the primitive index).
    pub entity: u64,
    /// Stable document address of the drawing entity.
    pub address: String,
    /// Display label for capture legends: scene name and address.
    pub label: String,
    /// Geometry to draw.
    pub geometry: Geometry,
    /// Model-to-world matrix.
    pub world: Mat4,
    /// Linear base color multiplier.
    pub color: [f32; 4],
    /// Perceptual roughness multiplier for shapes, factor override for models.
    pub roughness: f32,
    /// Metalness.
    pub metallic: f32,
    /// Emissive radiance (cd/m²).
    pub emissive: [f32; 3],
    /// Skin-order palette range in [`RenderFrame::palettes`] for skinned primitives.
    pub skin: Option<Range<usize>>,
}

/// A point or spot light in world space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightView {
    /// Point or spot.
    pub kind: LightKind,
    /// World position.
    pub position: [f32; 3],
    /// Unit emission direction (spot).
    pub direction: [f32; 3],
    /// Linear RGB tint.
    pub color: [f32; 3],
    /// Luminous flux in lumens.
    pub lumens: f32,
    /// Influence radius in metres.
    pub range: f32,
    /// Inner/outer cone half-angles in degrees.
    pub cone: [f32; 2],
}

/// A GPU particle effect to composite at an entity.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectView {
    /// Stable draw identity of the entity.
    pub entity: u64,
    /// `Name (address)` label for capture legends.
    pub label: String,
    /// Cooked library effect name.
    pub effect: String,
    /// World position (interpolated).
    pub position: [f32; 3],
}

/// The active viewpoint.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraView {
    /// Eye position.
    pub position: [f32; 3],
    /// Unit view direction.
    pub forward: [f32; 3],
    /// Vertical field of view in degrees.
    pub fov: f32,
}

/// Terrain for presentation: the shared evaluated tiles and foliage, and the tiles
/// whose `TerrainChunk` entities are resident (detail pages follow them).
#[derive(Clone)]
pub struct TerrainView {
    /// Immutable evaluated terrain (shared, never copied per frame).
    pub data: std::sync::Arc<crate::terrain::TerrainData>,
    /// Resident tiles in stable order.
    pub tiles: Vec<ara_terrain::TileCoord>,
}

impl std::fmt::Debug for TerrainView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerrainView")
            .field("tiles", &self.data.tiles.len())
            .field("foliage", &self.data.foliage.len())
            .field("resident", &self.tiles)
            .finish()
    }
}

/// Everything presentation needs for one frame.
#[derive(Debug, Default)]
pub struct RenderFrame {
    /// Completed simulation tick being presented.
    pub tick: u64,
    /// Interpolation phase toward the next tick, in [0, 1).
    pub alpha: f32,
    /// Active camera, if any.
    pub camera: Option<CameraView>,
    /// Sky and exposure.
    pub environment: Environment,
    /// Directional light: direction toward the light and linear RGB lux.
    pub sun: Option<([f32; 3], [f32; 3])>,
    /// Local lights in stable order.
    pub lights: Vec<LightView>,
    /// Draws in stable order.
    pub draws: Vec<Draw>,
    /// Flat skinning palettes (model space, skin joint order).
    pub palettes: Vec<Mat4>,
    /// GPU particle effects in stable order.
    pub effects: Vec<EffectView>,
    /// The world's terrain and its resident collision tiles (landscape renderer).
    pub terrain: Option<TerrainView>,
    scratch: Vec<Mat4>,
    palette: Vec<Mat4>,
}

impl RenderFrame {
    /// Rebuilds the frame from `world` at phase `alpha`, reusing buffers.
    pub fn extract(&mut self, world: &World, tick: u64, alpha: f32) {
        self.tick = tick;
        self.alpha = alpha;
        self.camera = None;
        self.environment = Environment::default();
        self.sun = None;
        self.lights.clear();
        self.draws.clear();
        self.palettes.clear();
        self.effects.clear();
        self.terrain = world
            .get_resource::<crate::terrain::TerrainState>()
            .map(|state| TerrainView {
                data: state.0.clone(),
                tiles: Vec::new(),
            });
        let library = world.get_resource::<Library>();
        let mut rows: Vec<_> = world
            .iter_entities()
            .filter_map(|e| Some((e.get::<Identity>()?.0.as_str(), e)))
            .collect();
        rows.sort_by(|a, b| a.0.cmp(b.0));
        let mut environment_set = false;
        let mut camera: Option<(i32, CameraView)> = None;
        for (address, entity) in rows {
            let pose = match (entity.get::<PreviousPose>(), entity.get::<WorldPose>()) {
                (Some(p), Some(c)) => p.0.lerp(c.0, alpha),
                (None, Some(c)) => c.0,
                _ => Pose::IDENTITY,
            };
            if let Some(env) = entity.get::<Environment>()
                && !environment_set
            {
                self.environment = *env;
                environment_set = true;
            }
            if let Some(c) = entity.get::<Camera>()
                && camera.is_none_or(|(p, _)| c.priority < p)
            {
                camera = Some((
                    c.priority,
                    CameraView {
                        position: pose.translation.to_array(),
                        forward: (pose.rotation * Vec3::NEG_Z).normalize().to_array(),
                        fov: c.fov,
                    },
                ));
            }
            if let Some(light) = entity.get::<Light>() {
                self.light(light, &pose);
            }
            if let (Some(tile), Some(terrain)) = (
                entity.get::<crate::terrain::TerrainChunk>(),
                &mut self.terrain,
            ) {
                terrain.tiles.push(ara_terrain::TileCoord {
                    x: tile.x,
                    z: tile.z,
                });
            }
            if let Some(effect) = entity.get::<VisualEffect>() {
                let name = entity
                    .get::<SceneMetadata>()
                    .map(|m| m.name.as_str())
                    .filter(|n| !n.is_empty())
                    .unwrap_or("effect");
                self.effects.push(EffectView {
                    entity: stable_id(address),
                    label: format!("{name} ({address})"),
                    effect: effect.effect.clone(),
                    position: pose.translation.to_array(),
                });
            }
            if let Some(renderer) = entity.get::<MeshRenderer>() {
                let name = entity
                    .get::<SceneMetadata>()
                    .map(|m| m.name.as_str())
                    .filter(|n| !n.is_empty())
                    .unwrap_or("entity");
                let label = format!("{name} ({address})");
                let base = Draw {
                    entity: stable_id(address),
                    address: address.to_owned(),
                    label,
                    geometry: Geometry::Shape {
                        shape: renderer.shape,
                        size: renderer.size,
                    },
                    world: pose.matrix(),
                    color: renderer.color.0,
                    roughness: renderer.roughness,
                    metallic: renderer.metallic,
                    emissive: renderer.emissive,
                    skin: None,
                };
                let model = renderer
                    .model
                    .as_ref()
                    .and_then(|m| asset_id(m).ok())
                    .and_then(|id| library?.models.get(id).cloned());
                match model {
                    None => self.draws.push(if renderer.model.is_some() {
                        Draw {
                            color: [1.0, 0.0, 1.0, 1.0],
                            ..base
                        }
                    } else {
                        base
                    }),
                    Some(model) => {
                        let palette = match entity.get::<AnimatorSlot>() {
                            Some(slot) if slot.program.model.digest == model.digest => slot
                                .palette(&mut self.scratch, &mut self.palette)
                                .ok()
                                .map(|()| {
                                    let start = self.palettes.len();
                                    self.palettes.extend_from_slice(&self.palette);
                                    start..self.palettes.len()
                                }),
                            _ => None,
                        };
                        for (i, primitive) in model.primitives.iter().enumerate() {
                            self.draws.push(Draw {
                                entity: base.entity.wrapping_add(i as u64),
                                geometry: Geometry::Model {
                                    model: Arc::clone(&model),
                                    primitive: i,
                                },
                                skin: primitive.skin.and(palette.clone()),
                                ..base.clone()
                            });
                        }
                    }
                }
            }
        }
        self.camera = camera.map(|(_, c)| c);
    }

    fn light(&mut self, light: &Light, pose: &Pose) {
        let [r, g, b, _] = light.color.0;
        match light.kind {
            LightKind::Directional => {
                if self.sun.is_none() {
                    // Shines along local -Z; the renderer wants the direction toward it.
                    let toward = (pose.rotation * Vec3::Z).normalize();
                    self.sun = Some((
                        toward.to_array(),
                        [
                            r * light.intensity,
                            g * light.intensity,
                            b * light.intensity,
                        ],
                    ));
                }
            }
            kind => self.lights.push(LightView {
                kind,
                position: pose.translation.to_array(),
                direction: (pose.rotation * Vec3::NEG_Z).normalize().to_array(),
                color: [r, g, b],
                lumens: light.intensity,
                range: light.range,
                cone: [light.inner_angle, light.outer_angle],
            }),
        }
    }
}
