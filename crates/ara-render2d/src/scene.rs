use anyhow::{Result, ensure};
use ara_2d::{Atlas, Flipbook, Light2d, Sprite, Tilemap};
use ara_components::Transform;
use ara_physics2d::PhysicsPose2d;
use ara_sim::{
    Identity, SceneMetadata,
    bevy_ecs::prelude::{ChildOf, Entity, World},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Resident atlas input, resolved from canonical metadata plus a cached pixel object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtlasImage {
    /// Canonical source region metadata.
    pub atlas: Atlas,
    /// Tightly packed RGBA8. Colors are sRGB; normal atlases are linear UNORM.
    pub rgba: Vec<u8>,
    /// True for normal/data textures; false for sRGB color.
    pub linear: bool,
}
impl AtlasImage {
    /// Reads the worker's AR2A v1 payload with exact size and version validation.
    pub fn from_cooked(atlas: Atlas, bytes: &[u8], linear: bool) -> Result<Self> {
        ensure!(
            bytes.len() >= 16 && &bytes[..8] == b"AR2A\x01\0\0\0",
            "invalid atlas pixel header"
        );
        let width = u32::from_le_bytes(bytes[8..12].try_into()?);
        let height = u32::from_le_bytes(bytes[12..16].try_into()?);
        ensure!(
            atlas.width == width
                && atlas.height == height
                && (1..=8192).contains(&width)
                && (1..=8192).contains(&height)
                && bytes.len() == 16 + width as usize * height as usize * 4,
            "invalid atlas pixel extent"
        );
        Ok(Self {
            atlas,
            rgba: bytes[16..].to_vec(),
            linear,
        })
    }
}
/// One immutable sprite extraction, independent of native handles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpriteDraw {
    /// Stable source identity used for picking and painter tie-breaking.
    pub entity: u64,
    /// Human-readable name.
    pub name: String,
    /// World pivot position in metres.
    pub position: [f32; 2],
    /// Row-major 2x2 local-to-world basis, preserving hierarchy rotation/scale/shear.
    pub basis: [f32; 4],
    /// Typed authoring values and symbolic asset references.
    pub sprite: Sprite,
}
/// One immutable tilemap extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TilemapDraw {
    /// Stable source identity; all cells pick their map entity.
    pub entity: u64,
    /// Human-readable name.
    pub name: String,
    /// World map origin in metres.
    pub origin: [f32; 2],
    /// Row-major 2x2 local-to-world basis.
    pub basis: [f32; 4],
    /// Canonical text source, cooked into visible GPU chunks at upload.
    pub map: Tilemap,
}
/// Immutable render extraction; ECS remains the authority.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Scene2d {
    /// Symbolic `@path#id` to resident atlas inputs.
    pub atlases: BTreeMap<String, AtlasImage>,
    /// Sprites, sorted by the renderer rather than extraction traversal.
    pub sprites: Vec<SpriteDraw>,
    /// Tile layers, split into 32x32 GPU batches.
    pub tilemaps: Vec<TilemapDraw>,
    /// Opt-in local lights in stable identity order.
    pub lights: Vec<Light2d>,
    /// World-space shadow segments, including derived tile silhouettes.
    pub occluders: Vec<[f32; 4]>,
}
impl Scene2d {
    /// Extracts a fixed-tick ECS snapshot, evaluating flipbooks without mutable state.
    /// Assets are supplied by the host's boundary-published ara-asset generation.
    pub fn extract(
        world: &mut World,
        atlases: BTreeMap<String, AtlasImage>,
        tick: u64,
    ) -> Result<Self> {
        let mut scene = Self {
            atlases,
            ..Default::default()
        };
        let mut query = world.query::<(
            Entity,
            &Identity,
            Option<&SceneMetadata>,
            Option<&Transform>,
            Option<&PhysicsPose2d>,
            Option<&Sprite>,
            Option<&Tilemap>,
            Option<&Light2d>,
            Option<&Flipbook>,
        )>();
        let mut rows: Vec<_> = query.iter(world).collect();
        rows.sort_by(|a, b| a.1.cmp(b.1));
        for (handle, id, name, transform, pose, sprite, map, light, clip) in rows {
            let digest = blake3::hash(id.0.as_bytes());
            let entity = u64::from_le_bytes(digest.as_bytes()[..8].try_into()?);
            let matrix = if let Some(pose) = pose {
                let scale = transform
                    .map(|t| [t.scale[0], t.scale[1], 1.])
                    .unwrap_or([1.; 3]);
                glam::Mat4::from_scale_rotation_translation(
                    glam::Vec3::from_array(scale),
                    glam::Quat::from_rotation_z(pose.rotation),
                    glam::Vec3::new(pose.position[0], pose.position[1], 0.),
                )
            } else {
                world_matrix(world, handle)?
            };
            let position = [matrix.w_axis.x, matrix.w_axis.y];
            let basis = [
                matrix.x_axis.x,
                matrix.y_axis.x,
                matrix.x_axis.y,
                matrix.y_axis.y,
            ];
            let name = name.map(|n| n.name.as_str()).unwrap_or(&id.0);
            if let Some(sprite) = sprite {
                let mut sprite = sprite.to_owned();
                if let Some(clip) = clip
                    && let Some(region) = clip.region_at(tick)?
                {
                    sprite.region = region.to_owned();
                }
                scene.sprites.push(SpriteDraw {
                    entity,
                    name: name.to_owned(),
                    position,
                    basis,
                    sprite,
                });
            }
            if let Some(map) = map {
                for hull in ara_2d::tilemap::colliders(map)? {
                    for i in 0..hull.vertices.len() {
                        let a = hull.vertices[i];
                        let b = hull.vertices[(i + 1) % hull.vertices.len()];
                        let a = matrix.transform_point3(glam::Vec3::new(a[0], a[1], 0.));
                        let b = matrix.transform_point3(glam::Vec3::new(b[0], b[1], 0.));
                        scene.occluders.push([a.x, a.y, b.x, b.y]);
                    }
                }
                scene.tilemaps.push(TilemapDraw {
                    entity,
                    name: name.to_owned(),
                    origin: position,
                    basis,
                    map: map.to_owned(),
                });
            }
            if let Some(light) = light {
                scene.lights.push(light.to_owned());
            }
        }
        Ok(scene)
    }
}

fn world_matrix(world: &World, mut entity: Entity) -> Result<glam::Mat4> {
    let mut result = glam::Mat4::IDENTITY;
    let mut depth = 0;
    loop {
        if let Some(t) = world.get::<Transform>(entity) {
            ensure!(
                t.position
                    .iter()
                    .chain(t.rotation.iter())
                    .chain(t.scale.iter())
                    .all(|v| v.is_finite())
                    && t.rotation[0] == 0.
                    && t.rotation[1] == 0.,
                "2D extraction requires finite XY transforms with Z rotation"
            );
            result = glam::Mat4::from_scale_rotation_translation(
                glam::Vec3::from_array(t.scale),
                glam::Quat::from_rotation_z(t.rotation[2].to_radians()),
                glam::Vec3::from_array(t.position),
            ) * result;
        }
        let Some(parent) = world.get::<ChildOf>(entity) else {
            break;
        };
        entity = parent.parent();
        depth += 1;
        ensure!(
            depth <= 256,
            "2D hierarchy is cyclic or exceeds 256 parents"
        );
    }
    ensure!(result.is_finite(), "overflowing 2D hierarchy transform");
    Ok(result)
}
