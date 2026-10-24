//! Reuses prepared source data; live frames only change poses, factors and skin data.
use super::*;
use crate::material::Source;
use ara_runtime::{components::Environment, extract::Geometry, model::ModelVertex};

/// Whether the live frame fits the currently prepared content generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameChange {
    /// Geometry/texture generations and draw identities still match.
    Ready {
        /// Root draws whose world matrix changed.
        poses: usize,
        /// Independent skin geometries updated from changed joint palettes.
        skins: usize,
    },
    /// Prepare replacement content while continuing to show the last good view.
    Rebuild,
}

struct Link {
    entity: u64,
    geometry: u64,
    source: (u8, [u8; 32]),
    palette: Vec<glam::Mat4>,
    deforming: bool,
}

pub(super) struct Bindings {
    links: Vec<Link>,
    keys: BTreeMap<(String, usize), u32>,
    terrain: Option<Arc<ara_runtime::terrain::TerrainData>>,
    tiles: Vec<ara_terrain::TileCoord>,
    environment: Environment,
    lights: Vec<ara_runtime::extract::LightView>,
    scratch: Vec<ara_render::scene::Vertex>,
    live: bool,
}

fn source(draw: &Draw) -> (u8, [u8; 32]) {
    if let Some(material) = &draw.material {
        (1, material.digest.0)
    } else if let Geometry::Model { model, .. } = &draw.geometry {
        (2, model.digest.0)
    } else {
        (0, [0; 32])
    }
}

impl Bindings {
    pub(super) fn new(frame: &RenderFrame, keys: BTreeMap<(String, usize), u32>) -> Result<Self> {
        let links = frame
            .draws
            .iter()
            .map(|draw| {
                Ok(Link {
                    entity: draw.entity,
                    geometry: crate::geometry_key(&draw.geometry),
                    source: source(draw),
                    palette: match &draw.skin {
                        Some(range) => frame
                            .palettes
                            .get(range.start..range.end)
                            .context("invalid skin palette")?
                            .to_vec(),
                        None => Vec::new(),
                    },
                    deforming: draw.skin.is_some(),
                })
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            links,
            keys,
            terrain: frame.terrain.as_ref().map(|t| Arc::clone(&t.data)),
            tiles: frame
                .terrain
                .as_ref()
                .map_or_else(Vec::new, |t| t.tiles.to_vec()),
            environment: frame.environment,
            lights: frame.lights.to_vec(),
            scratch: Vec::new(),
            live: false,
        })
    }
}

impl Prepared {
    /// Reserves independent root material slots and skin scratch at a content
    /// boundary, before GPU upload. Static capture snapshots retain material sharing.
    pub fn enable_motion(&mut self) {
        if self.bindings.live {
            return;
        }
        let mut largest = 0;
        for instance in self
            .scene
            .instances
            .iter_mut()
            .take(self.bindings.links.len())
        {
            let material = self.scene.materials[instance.material];
            instance.material = self.scene.materials.len();
            self.scene.materials.push(material);
            let geometry = &self.scene.geometry[instance.geometry];
            if geometry.deforming {
                largest = largest.max(geometry.vertices.len());
            }
        }
        self.bindings.scratch.reserve(largest);
        self.bindings.live = true;
    }

    /// Streams one extraction without copying static vertices, hashing cooked
    /// bytes, parsing graphs or cloning asset handles. Source generations/layout
    /// changes request a replacement; GPU publication remains the view's job.
    pub fn update_frame(&mut self, frame: &RenderFrame) -> Result<FrameChange> {
        ara_gpu::cpu_scope!("Native.update_instances");
        anyhow::ensure!(
            self.bindings.live,
            "enable native motion before streaming frames"
        );
        let bindings = &mut self.bindings;
        if frame.draws.len() != bindings.links.len()
            || frame.environment.sky != bindings.environment.sky
            || frame.environment.horizon != bindings.environment.horizon
            || frame.environment.ground != bindings.environment.ground
            || frame.environment.intensity != bindings.environment.intensity
        {
            return Ok(FrameChange::Rebuild);
        }
        match (&frame.terrain, &bindings.terrain) {
            (Some(current), Some(old))
                if Arc::ptr_eq(&current.data, old) && current.tiles == bindings.tiles => {}
            (None, None) => {}
            _ => return Ok(FrameChange::Rebuild),
        }
        for (draw, link) in frame.draws.iter().zip(&bindings.links) {
            if draw.entity != link.entity
                || crate::geometry_key(&draw.geometry) != link.geometry
                || draw.skin.is_some() != link.deforming
            {
                return Ok(FrameChange::Rebuild);
            }
        }
        let mut poses = 0;
        let mut skins = 0;
        for (index, (draw, link)) in frame.draws.iter().zip(&mut bindings.links).enumerate() {
            let instance = &mut self.scene.instances[index];
            let world = glam30::Mat4::from_cols_array(&draw.world.to_cols_array());
            if world != instance.world {
                instance.world = world;
                poses += 1;
            }
            let current = source(draw);
            let material = &mut self.scene.materials[instance.material];
            if current != link.source {
                let mut replacement = Material::default();
                if let Some(source) = Source::of(draw)
                    && !material::resolve_known(&mut replacement, source, &bindings.keys)
                {
                    return Ok(FrameChange::Rebuild);
                }
                *material = replacement;
                link.source = current;
            }
            material::apply_draw(material, draw, Source::of(draw));
            if let Some(range) = &draw.skin {
                let palette = frame
                    .palettes
                    .get(range.start..range.end)
                    .context("invalid live skin palette")?;
                if palette != link.palette {
                    if palette.len() != link.palette.len() {
                        return Ok(FrameChange::Rebuild);
                    }
                    let vertices = match &draw.geometry {
                        Geometry::Model { model, primitive } => {
                            model.primitives[*primitive].vertices.as_slice()
                        }
                        Geometry::Shape { .. } => {
                            anyhow::bail!("procedural geometry cannot own a skin palette")
                        }
                    };
                    update_skin(
                        &mut self.scene.geometry[instance.geometry],
                        vertices,
                        palette,
                        &mut bindings.scratch,
                    );
                    link.palette.copy_from_slice(palette);
                    skins += 1;
                }
            }
        }
        if frame.lights != bindings.lights {
            self.scene.lights = super::super::lights(frame);
            bindings.lights.clear();
            bindings.lights.extend_from_slice(&frame.lights);
        }
        let (direction, lux) = frame.sun.unwrap_or(([0.3, 1.0, 0.2], [0.0; 3]));
        self.scene.sun_direction = glam30::Vec3::from_array(direction).normalize();
        self.scene.sun_lux = glam30::Vec3::from_array(lux);
        Ok(FrameChange::Ready { poses, skins })
    }
}

fn update_skin(
    geometry: &mut render::Geometry,
    source: &[ModelVertex],
    palette: &[glam::Mat4],
    scratch: &mut Vec<ara_render::scene::Vertex>,
) {
    crate::geometry::transform(source, glam::Mat4::IDENTITY, Some(palette), scratch);
    let mut bounds = [
        glam30::Vec3::splat(f32::INFINITY),
        glam30::Vec3::splat(f32::NEG_INFINITY),
    ];
    for (vertex, posed) in geometry.vertices.iter_mut().zip(scratch) {
        vertex.position = posed.position;
        vertex.normal = posed.normal;
        vertex.tangent = posed.tangent;
        let position = glam30::Vec3::from_array(posed.position);
        bounds[0] = bounds[0].min(position);
        bounds[1] = bounds[1].max(position);
    }
    geometry.bounds = bounds;
    geometry.revision = geometry.revision.wrapping_add(1);
}
