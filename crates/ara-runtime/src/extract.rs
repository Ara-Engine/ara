//! Presentation extraction: a renderer-neutral [`RenderFrame`] built from the world
//! at an interpolation phase. Reads only; buffers are reused between frames.
use crate::{
    animation::{AnimatorSlot, Library},
    assets::asset_id,
    components::{Camera, Environment, Light, LightKind, MeshRenderer, MeshShape, VisualEffect},
    math::{Pose, stable_id},
    model::{Model, ModelSelection},
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
    /// Procedural emissive radiance, or a model's RGB emission multiplier.
    pub emissive: [f32; 3],
    /// Replaces an imported material's emissive factor before multiplying `emissive`.
    pub emissive_factor: Option<[f32; 3]>,
    /// Optional immutable native graph/instance material replacing the source slot.
    pub material: Option<Arc<crate::material::MaterialAsset>>,
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

/// An authored reflection capture with its entity's world pose (interpolated).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaptureView {
    /// The authored capture.
    pub capture: ara_components::render::ReflectionCapture,
    /// World position of the entity.
    pub translation: [f32; 3],
    /// World rotation of the entity (quaternion xyzw).
    pub rotation: [f32; 4],
    /// World scale of the entity (scales the boxes and offsets).
    pub scale: [f32; 3],
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
    /// Uniform world size of the effect (`VisualEffect.scale`).
    pub scale: f32,
    /// Radiance multiplier (`VisualEffect.intensity`).
    pub intensity: f32,
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

/// A named camera at its resolved ECS pose. Ambiguous names require an ID/address.
pub fn named_camera(world: &mut World, selector: &str) -> Result<CameraView, String> {
    let selector = selector.strip_prefix("name:").unwrap_or(selector);
    let mut query = world.query::<(
        &Identity,
        &Camera,
        Option<&SceneMetadata>,
        Option<&WorldPose>,
    )>();
    let mut found = None;
    for (identity, camera, metadata, pose) in query.iter(world) {
        if identity.0 != selector
            && !identity.0.ends_with(&format!("#{selector}"))
            && metadata.is_none_or(|m| m.name != selector)
        {
            continue;
        }
        if found.is_some() {
            return Err(format!(
                "camera `{selector}` is ambiguous; use its document address"
            ));
        }
        let pose = pose.map_or(Pose::IDENTITY, |p| p.0);
        found = Some(CameraView {
            position: pose.translation.to_array(),
            forward: (pose.rotation * Vec3::NEG_Z).normalize().to_array(),
            fov: camera.fov,
        });
    }
    found.ok_or_else(|| format!("no Camera matches `{selector}`"))
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
    /// Authored physical sky and height fog (first entity in ID order per component)
    /// and every post-process volume; renderers fall back to `environment` without them.
    pub look: ara_components::render::SceneLook,
    /// Directional light: direction toward the light and linear RGB lux.
    pub sun: Option<([f32; 3], [f32; 3])>,
    /// Local lights in stable order.
    pub lights: Vec<LightView>,
    /// Authored reflection captures in stable order (renderers place their own
    /// when there are none).
    pub reflection_captures: Vec<CaptureView>,
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
        self.look.sky = None;
        self.look.fog = None;
        self.look.volumes.clear();
        self.look.luts.clear();
        self.sun = None;
        self.lights.clear();
        self.reflection_captures.clear();
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
            if self.look.sky.is_none() {
                self.look.sky = entity
                    .get::<ara_components::render::SkyAtmosphere>()
                    .copied();
            }
            if self.look.fog.is_none() {
                self.look.fog = entity.get::<ara_components::render::HeightFog>().copied();
            }
            if let Some(volume) = entity.get::<ara_components::post::PostProcessVolume>() {
                // The lookup tables it references, shared from the library.
                for lut in volume.overrides.iter().filter_map(|o| o.lut.as_ref()) {
                    if let (Some(library), Ok(id)) = (library, crate::assets::asset_id(lut))
                        && let Some(table) = library.luts.get(id)
                        && !self.look.luts.iter().any(|(known, _)| known == id)
                    {
                        self.look
                            .luts
                            .push((id.to_owned(), std::sync::Arc::clone(table)));
                    }
                }
                self.look.volumes.push(ara_components::post::PostVolume {
                    volume: volume.clone(),
                    translation: pose.translation.to_array(),
                    rotation: pose.rotation.to_array(),
                    scale: pose.scale.to_array(),
                });
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
            if let Some(capture) = entity.get::<ara_components::render::ReflectionCapture>() {
                self.reflection_captures.push(CaptureView {
                    capture: *capture,
                    translation: pose.translation.to_array(),
                    rotation: pose.rotation.to_array(),
                    scale: pose.scale.to_array(),
                });
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
                    scale: effect.scale,
                    intensity: effect.intensity,
                });
            }
            if let Some(renderer) = entity.get::<MeshRenderer>() {
                let name = entity
                    .get::<SceneMetadata>()
                    .map(|m| m.name.as_str())
                    .filter(|n| !n.is_empty())
                    .unwrap_or("entity");
                let label = format!("{name} ({address})");
                let strength = if renderer.emissive_enabled {
                    renderer.emissive_strength
                } else {
                    0.
                };
                let material = if renderer.model.is_none() {
                    material_override(renderer, 0, library)
                } else {
                    None
                };
                let mut base = Draw {
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
                    emissive: renderer
                        .emissive_factor
                        .unwrap_or(renderer.emissive)
                        .map(|v| v * strength),
                    emissive_factor: None,
                    material,
                    skin: None,
                };
                if base.material.is_some() {
                    base.emissive = emission_multiplier(renderer, strength);
                    base.emissive_factor = renderer.emissive_factor;
                }
                let model = renderer
                    .model
                    .as_ref()
                    .and_then(|m| asset_id(m).ok())
                    .and_then(|id| library?.models.get(id).cloned());
                match model {
                    None => self.draws.push(if renderer.model.is_some() {
                        Draw {
                            color: [1.0, 0.0, 1.0, 1.0],
                            emissive: [0.; 3],
                            ..base
                        }
                    } else {
                        base
                    }),
                    Some(model) => {
                        let selection = if renderer.node.is_none() && renderer.primitive.is_none() {
                            ModelSelection {
                                primitives: 0..model.primitives.len(),
                                base: 0,
                                offset: [0.; 3],
                            }
                        } else {
                            let Ok(selection) =
                                model.select(renderer.node.as_deref(), renderer.primitive)
                            else {
                                self.draws.push(Draw {
                                    color: [1., 0., 1., 1.],
                                    emissive: [0.; 3],
                                    ..base
                                });
                                continue;
                            };
                            selection
                        };
                        if selection.offset != [0.; 3] {
                            base.world *=
                                Mat4::from_translation(Vec3::from_array(selection.offset));
                        }
                        base.emissive = emission_multiplier(renderer, strength);
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
                        for i in selection.primitives {
                            let primitive = &model.primitives[i];
                            let material = material_override(renderer, i - selection.base, library);
                            self.draws.push(Draw {
                                entity: base.entity.wrapping_add(i as u64),
                                geometry: Geometry::Model {
                                    model: Arc::clone(&model),
                                    primitive: i,
                                },
                                skin: primitive.skin.and(palette.clone()),
                                emissive_factor: emission_override(
                                    renderer,
                                    &model,
                                    primitive.material,
                                ),
                                material,
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

#[inline]
fn emission_multiplier(renderer: &MeshRenderer, strength: f32) -> [f32; 3] {
    let multiplier = if renderer.emissive == [0.; 3] {
        [1.; 3]
    } else {
        renderer.emissive
    };
    multiplier.map(|v| v * strength)
}

#[inline]
fn material_override(
    renderer: &MeshRenderer,
    slot: usize,
    library: Option<&Library>,
) -> Option<Arc<crate::material::MaterialAsset>> {
    renderer
        .material_overrides
        .iter()
        .find(|entry| entry.primitive as usize == slot)
        .and_then(|entry| asset_id(&entry.material).ok())
        .and_then(|id| library?.materials.get(id))
        .cloned()
}

#[inline]
fn emission_override(
    renderer: &MeshRenderer,
    model: &Model,
    material: Option<usize>,
) -> Option<[f32; 3]> {
    renderer.emissive_factor.filter(|_| {
        renderer.emissive_materials.is_empty()
            || material.is_some_and(|index| {
                model
                    .materials
                    .get(index)
                    .is_some_and(|m| renderer.targets_emissive_material(index, &m.name))
            })
    })
}
