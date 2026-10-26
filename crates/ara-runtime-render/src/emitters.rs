//! Emissive surfaces as direct lights for the raster path. Each emissive
//! geometry's triangles are sampled through their emissive map and gathered into
//! model-space patches (a festoon string becomes its bulbs, not its wire). Rays
//! from each patch against its own geometry find the light that escapes the
//! fixture (a bulb's socket or a lamp's shade absorbs the rest) and the lobe it
//! leaves in: a flat panel's hemisphere, a capped bulb's downward cone, a bare
//! bulb's sphere. Each lobe becomes a local light carrying its flux, a spot or a
//! point. Emission is two-sided, as in the path tracer, so an open sheet (a
//! single-quad panel) also emits from its back faces, while a closed surface's
//! inward emission stays trapped. Neighbouring lights cluster on a world grid that
//! coarsens only until they fit the preset's budget. Probes keep only the bounce
//! (the GI bake zeroes [`is_emitter`] materials' emission). The search runs off
//! the render thread: [`Job::new`] copies what it needs and [`Job::run`] traces.
use crate::gi_cpu::{Mesh, Shading};
use anyhow::{Context, Result};
use ara_render::{
    instanced::{
        Geometry, Lod, Material, NO_TEXTURE, Scene, TextureSlot, TextureTransform, Vertex,
    },
    scene::{EMISSIVE_DECODE_SRGB, Light},
};
use glam30::{Mat3, Mat4, Vec2, Vec3};
use std::{
    collections::{BTreeMap, BTreeSet},
    f32::consts::PI,
    sync::Arc,
};

/// Emitted radiance (mean luminance, cd/m2) above which a material lights the
/// scene directly; dimmer glow stays in the probe volume.
const EMITTER_RADIANCE: f32 = 5.;
/// Flux (lumens) below which a lobe is too dim to be a light.
const MIN_FLUX: f32 = 1.;
/// Illuminance (lux) at which a light's influence ends.
const CUTOFF_LUX: f32 = 0.1;
/// Patch size (metres): triangles gather per model-space cell of this size, and
/// lights cluster on a world grid doubling from it until they fit the budget.
const CELL: f32 = 0.75;
/// Side limit of the emissive map level sampled per triangle.
const MAP_SIDE: u32 = 128;
/// Normal agreement (length of the emission-weighted normal mean) above which a
/// surface counts as an open flat sheet.
const FLAT: f32 = 0.7;
/// Escape rays per patch side.
const RAYS: usize = 256;

fn luminance(c: Vec3) -> f32 {
    c.dot(Vec3::new(0.2126, 0.7152, 0.0722))
}

/// Whether a material with (mean) `shading` lights the scene as emitter lights.
pub(crate) fn is_emitter(shading: Shading) -> bool {
    luminance(shading.emissive) > EMITTER_RADIANCE
}

/// A decoded emissive map: size and linear RGBA texels, row-major.
pub(crate) type Map = ([u32; 2], Vec<[f32; 4]>);

/// Nearest texel of `map` at `uv`, wrapping.
fn sample((size, texels): &Map, uv: Vec2) -> Vec3 {
    let at = |v: f32, n: u32| ((v.rem_euclid(1.) * n as f32) as u32).min(n.saturating_sub(1));
    let index = at(uv.y, size[1]) * size[0] + at(uv.x, size[0]);
    texels
        .get(index as usize)
        .map_or(Vec3::ONE, |t| Vec3::new(t[0], t[1], t[2]))
}

/// A vertex's coordinates in `transform`'s map space, as the shaders compute them.
fn coordinates(transform: &TextureTransform, vertex: &Vertex) -> Vec2 {
    let raw = if transform.tex_coord == 1 {
        vertex.uv1
    } else {
        vertex.uv
    };
    let p = Vec2::from_array(raw) * Vec2::from_array(transform.scale);
    let (s, c) = transform.rotation.sin_cos();
    Vec2::from_array(transform.offset) + Vec2::new(c * p.x - s * p.y, s * p.x + c * p.y)
}

/// One emitting triangle in model space.
#[derive(Clone, Copy)]
struct Emitting {
    /// Corner and edges.
    a: Vec3,
    e1: Vec3,
    e2: Vec3,
    /// Outward unit normal (oriented with the vertex normals).
    normal: Vec3,
    /// Emitted radiance times area (one side), linear RGB.
    emission: Vec3,
}

/// The light leaving one side of a patch: escaped emission (radiance x area; pi
/// x this in lumens) and the mean escaped direction, whose length measures how
/// collimated the lobe is.
#[derive(Clone, Copy)]
struct Lobe {
    emission: Vec3,
    direction: Vec3,
}

/// Emission gathered from one model-space cell of a geometry.
#[derive(Clone)]
struct Patch {
    /// Emission-weighted centroid.
    centre: Vec3,
    /// Emitting area.
    area: f32,
    /// The escaping lobes: the outer faces', and the inner faces' of an open sheet.
    lobes: Vec<Lobe>,
}

/// A deterministic cosine-weighted direction about `normal` (sample `k` of `n`).
fn cosine(normal: Vec3, k: usize, n: usize) -> Vec3 {
    let u = (k as f32 + 0.5) / n as f32;
    let v = (k as f32 * 0.618_034).fract();
    let (s, c) = (2. * PI * v).sin_cos();
    let r = u.sqrt();
    let (t, b) = normal.any_orthonormal_pair();
    (t * (r * c) + b * (r * s) + normal * (1. - u).max(0.).sqrt()).normalize_or_zero()
}

/// The fraction of one side's emission escaping `mesh` and its mean direction,
/// from `RAYS` rays spread over `triangles` in proportion to their emission.
fn escape(mesh: &Mesh, triangles: &[Emitting], side: f32, reach: f32) -> (f32, Vec3) {
    let total: f32 = triangles.iter().map(|t| luminance(t.emission)).sum();
    let (mut escaped, mut direction) = (0usize, Vec3::ZERO);
    let (mut index, mut before) = (0, 0.);
    for k in 0..RAYS {
        // Systematic sampling along the emission CDF.
        let target = (k as f32 + 0.5) / RAYS as f32 * total;
        while index + 1 < triangles.len() && before + luminance(triangles[index].emission) < target
        {
            before += luminance(triangles[index].emission);
            index += 1;
        }
        let t = triangles[index];
        let (u, v) = (
            (k as f32 * 0.754_877).fract(),
            (k as f32 * 0.569_840).fract(),
        );
        let (u, v) = if u + v > 1. { (1. - u, 1. - v) } else { (u, v) };
        let normal = t.normal * side;
        let origin = t.a + t.e1 * u + t.e2 * v + normal * 1e-4;
        let ray = cosine(normal, k, RAYS);
        if mesh.trace(origin, ray, reach).is_none() {
            escaped += 1;
            direction += ray;
        }
    }
    let fraction = escaped as f32 / RAYS as f32;
    (fraction, direction / escaped.max(1) as f32)
}

/// The emitting patches of `geometry` (finest LOD) under `material`; `mesh` is
/// the geometry's, for the escape rays.
fn patches(geometry: &Geometry, material: &Material, map: Option<&Map>, mesh: &Mesh) -> Vec<Patch> {
    let Some(lod) = geometry.lods.first() else {
        return Vec::new();
    };
    let factor = Vec3::from_slice(&material.pbr.emissive_roughness[..3]);
    let transform = &material.extras.uv[TextureSlot::Emissive as usize];
    let mut cells: BTreeMap<[i32; 3], Vec<Emitting>> = BTreeMap::new();
    for tri in lod.indices.as_chunks::<3>().0 {
        let Some([a, b, c]) = tri
            .iter()
            .map(|&i| geometry.vertices.get(i as usize))
            .collect::<Option<Vec<_>>>()
            .and_then(|v| <[_; 3]>::try_from(v).ok())
        else {
            continue;
        };
        let [pa, pb, pc] = [a, b, c].map(|v| Vec3::from_array(v.position));
        let (e1, e2) = (pb - pa, pc - pa);
        let cross = e1.cross(e2);
        let area = cross.length() * 0.5;
        if area <= 0. {
            continue;
        }
        // Mean of four samples: the centroid and halfway from it to each corner.
        let radiance = match map {
            Some(map) => {
                let [ua, ub, uc] = [a, b, c].map(|v| coordinates(transform, v));
                let points = [
                    (ua + ub + uc) / 3.,
                    (ua * 4. + ub + uc) / 6.,
                    (ua + ub * 4. + uc) / 6.,
                    (ua + ub + uc * 4.) / 6.,
                ];
                factor * points.map(|p| sample(map, p)).into_iter().sum::<Vec3>() / 4.
            }
            None => factor,
        };
        if luminance(radiance) <= 0. {
            continue;
        }
        let mut normal = cross / (2. * area);
        let shading =
            Vec3::from_array(a.normal) + Vec3::from_array(b.normal) + Vec3::from_array(c.normal);
        if normal.dot(shading) < 0. {
            normal = -normal;
        }
        let centroid = (pa + pb + pc) / 3.;
        cells
            .entry((centroid / CELL).floor().as_ivec3().to_array())
            .or_default()
            .push(Emitting {
                a: pa,
                e1,
                e2,
                normal,
                emission: radiance * area,
            });
    }
    if cells.is_empty() {
        return Vec::new();
    }
    // Openness is the whole surface's: a thin box's faces may fall in different
    // cells, each flat, yet its inward emission is trapped.
    let all = cells.values().flatten();
    let weight: f32 = all.clone().map(|t| luminance(t.emission)).sum();
    let normal: Vec3 = all.map(|t| t.normal * luminance(t.emission)).sum();
    let open = normal.length() > FLAT * weight;
    let (lo, hi) = mesh.bounds();
    let reach = (hi - lo).length().max(1e-3) * 2.;
    cells
        .into_values()
        .map(|triangles| {
            let emission: Vec3 = triangles.iter().map(|t| t.emission).sum();
            let weight: f32 = triangles.iter().map(|t| luminance(t.emission)).sum();
            let centre = triangles
                .iter()
                .map(|t| (t.a + (t.e1 + t.e2) / 3.) * luminance(t.emission))
                .sum::<Vec3>()
                / weight;
            let area = triangles
                .iter()
                .map(|t| t.e1.cross(t.e2).length() * 0.5)
                .sum();
            let sides: &[f32] = if open { &[1., -1.] } else { &[1.] };
            let lobes = sides
                .iter()
                .map(|&side| {
                    let (fraction, direction) = escape(mesh, &triangles, side, reach);
                    Lobe {
                        emission: emission * fraction,
                        direction,
                    }
                })
                .filter(|l| luminance(l.emission) > 0.)
                .collect();
            Patch {
                centre,
                area,
                lobes,
            }
        })
        .collect()
}

/// One emitting instance: its model-space patches.
struct Emitter {
    instance: usize,
    patches: Arc<[Patch]>,
    /// The transform its current lights were placed with.
    placed: Option<Mat4>,
}

/// The emissive instances of one content generation; their lights follow the
/// instances' current transforms.
pub(crate) struct Emitters {
    emitters: Vec<Emitter>,
    /// Materials the search found emitting, so an edit to one re-runs it.
    materials: BTreeSet<usize>,
    /// The light budget the current lights were clustered for.
    budget: Option<usize>,
}

/// One emissive geometry and material pair, copied for a [`Job`].
struct Source {
    /// The scene's geometry index (meshes build once per geometry).
    geometry: usize,
    /// The scene's material index.
    material_index: usize,
    /// The finest LOD alone.
    copy: Geometry,
    material: Material,
    map: Option<Map>,
}

/// An emitter search: the emissive instances of one content generation with
/// copies of their geometry, materials and decoded maps, so the rays can run on
/// a worker thread while the scene keeps changing.
pub(crate) struct Job {
    /// (instance, source) per emissive instance.
    instances: Vec<(usize, usize)>,
    sources: Vec<Source>,
}

impl Job {
    /// Collects the emitters of `scene`; `shading` is indexed by material and
    /// `map` decodes an emissive texture (at most `side` texels a side, linear
    /// where the material decodes sRGB itself: `EMISSIVE_DECODE_SRGB`).
    pub(crate) fn new(
        scene: &Scene,
        shading: &[Shading],
        mut map: impl FnMut(u32, u32, bool) -> Option<Map>,
    ) -> Self {
        let mut maps: BTreeMap<(u32, bool), Option<Map>> = BTreeMap::new();
        let mut keys: BTreeMap<(usize, usize), usize> = BTreeMap::new();
        let (mut instances, mut sources) = (Vec::new(), Vec::new());
        for (instance, i) in scene.instances.iter().enumerate() {
            let (Some(&s), Some(material), Some(geometry)) = (
                shading.get(i.material),
                scene.materials.get(i.material),
                scene.geometry.get(i.geometry),
            ) else {
                continue;
            };
            if !is_emitter(s) {
                continue;
            }
            let source = *keys.entry((i.geometry, i.material)).or_insert_with(|| {
                let texture = material.pbr.textures[3];
                let decode = material.pbr.flags[1] & EMISSIVE_DECODE_SRGB != 0;
                let map = (texture != NO_TEXTURE)
                    .then(|| {
                        maps.entry((texture, decode))
                            .or_insert_with(|| map(texture, MAP_SIDE, decode))
                            .clone()
                    })
                    .flatten();
                sources.push(Source {
                    geometry: i.geometry,
                    material_index: i.material,
                    copy: Geometry {
                        key: geometry.key,
                        vertices: geometry.vertices.clone(),
                        lods: geometry
                            .lods
                            .first()
                            .map(|lod| Lod {
                                indices: lod.indices.clone(),
                                error: 0.,
                            })
                            .into_iter()
                            .collect(),
                        bounds: geometry.bounds,
                        deforming: geometry.deforming,
                        revision: geometry.revision,
                    },
                    material: *material,
                    map,
                });
                sources.len() - 1
            });
            instances.push((instance, source));
        }
        Self { instances, sources }
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    /// Traces every source's patches (on a worker thread).
    pub(crate) fn run(self) -> Emitters {
        let started = std::time::Instant::now();
        let mut meshes: BTreeMap<usize, Mesh> = BTreeMap::new();
        let patches: Vec<Arc<[Patch]>> = self
            .sources
            .iter()
            .map(|s| {
                let mesh = meshes
                    .entry(s.geometry)
                    .or_insert_with(|| Mesh::of(&s.copy, 0));
                patches(&s.copy, &s.material, s.map.as_ref(), mesh).into()
            })
            .collect();
        let emitters = Emitters {
            emitters: self
                .instances
                .into_iter()
                .filter(|&(_, source)| !patches[source].is_empty())
                .map(|(instance, source)| Emitter {
                    instance,
                    patches: patches[source].clone(),
                    placed: None,
                })
                .collect(),
            materials: self.sources.iter().map(|s| s.material_index).collect(),
            budget: None,
        };
        if !emitters.is_empty() {
            eprintln!(
                "ara-runtime-render: {} emissive surfaces ({:.0} lm escaping) found in {:.1} ms",
                emitters.len(),
                emitters.lumens(),
                started.elapsed().as_secs_f64() * 1e3
            );
        }
        emitters
    }
}

/// Whether editing the `changed` materials of `scene` changes its emitter lights:
/// one of `current`'s emitters was edited (strength, colour or its emissive
/// switch), or a material now emits enough to become one. `mean` gives a map's
/// mean texel (linear where the material decodes sRGB itself).
pub(crate) fn emission_edited(
    current: Option<&Emitters>,
    scene: &Scene,
    mean: impl Fn(u32, bool) -> Option<[f32; 4]>,
    changed: &[usize],
) -> bool {
    changed.iter().any(|&index| {
        current.is_some_and(|e| e.materials.contains(&index))
            || scene.materials.get(index).is_some_and(|m| {
                let texture = m.pbr.textures[3];
                let decode = m.pbr.flags[1] & EMISSIVE_DECODE_SRGB != 0;
                let map = (texture != NO_TEXTURE)
                    .then(|| mean(texture, decode))
                    .flatten()
                    .unwrap_or([1.; 4]);
                let e = m.pbr.emissive_roughness;
                is_emitter(Shading {
                    emissive: Vec3::new(e[0] * map[0], e[1] * map[1], e[2] * map[2]),
                    ..Shading::default()
                })
            })
    })
}

/// The emitter search off the render thread: at most one running job and one
/// queued rerun, so a burst of edits (a strength slider) runs one search at a
/// time and the last edit always lands. The current lights stay until then.
#[derive(Default)]
pub(crate) struct Search {
    running: Option<std::thread::JoinHandle<Emitters>>,
    queued: bool,
    #[cfg(test)]
    started: usize,
}

impl Search {
    /// Searches with `job` now, or once the running search lands (built then,
    /// from the content of that moment).
    pub(crate) fn request(&mut self, job: impl FnOnce() -> Job) -> Result<()> {
        if self.running.is_some() {
            self.queued = true;
            return Ok(());
        }
        self.start(job())
    }

    /// Starts over for new content: a search of the old content is left to
    /// finish unobserved, and a queued rerun is dropped.
    pub(crate) fn restart(&mut self, job: Job) -> Result<()> {
        self.running = None;
        self.queued = false;
        self.start(job)
    }

    fn start(&mut self, job: Job) -> Result<()> {
        self.running = Some(
            std::thread::Builder::new()
                .name("ara-emitters".into())
                .spawn(move || job.run())
                .context("emitter search thread")?,
        );
        #[cfg(test)]
        {
            self.started += 1;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn searches_started(&self) -> usize {
        self.started
    }

    /// Whether a search is running or queued.
    pub(crate) fn is_busy(&self) -> bool {
        self.running.is_some() || self.queued
    }

    /// The finished search, if any (waiting for the running one when `wait`); a
    /// queued rerun then starts from `job`.
    pub(crate) fn poll(
        &mut self,
        wait: bool,
        job: impl FnOnce() -> Job,
    ) -> Result<Option<Emitters>> {
        let Some(running) = self.running.take_if(|r| wait || r.is_finished()) else {
            return Ok(None);
        };
        let emitters = running
            .join()
            .map_err(|_| anyhow::anyhow!("the emitter search panicked"))?;
        if std::mem::take(&mut self.queued) {
            self.start(job())?;
        }
        Ok(Some(emitters))
    }
}

impl Emitters {
    #[cfg(test)]
    fn new(
        scene: &Scene,
        shading: &[Shading],
        map: impl FnMut(u32, u32, bool) -> Option<Map>,
    ) -> Self {
        Job::new(scene, shading, map).run()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.emitters.is_empty()
    }

    /// Emissive instances (before clustering).
    pub(crate) fn len(&self) -> usize {
        self.emitters.len()
    }

    /// Total escaping flux in lumens (unscaled instances), for logs.
    pub(crate) fn lumens(&self) -> f32 {
        self.emitters
            .iter()
            .flat_map(|e| e.patches.iter())
            .flat_map(|p| &p.lobes)
            .map(|l| luminance(l.emission) * PI)
            .sum()
    }

    /// Lights for the emitters at `scene`'s current transforms, at most `budget`;
    /// `None` while no emitter moved and the budget held since the last call
    /// (a per-frame check without allocation).
    pub(crate) fn update(&mut self, scene: &Scene, budget: usize) -> Option<Vec<Light>> {
        let mut moved = self.budget != Some(budget);
        for e in &mut self.emitters {
            let world = scene.instances.get(e.instance).map(|i| i.world);
            if world != e.placed {
                e.placed = world;
                moved = true;
            }
        }
        self.budget = Some(budget);
        moved.then(|| self.lights(budget))
    }

    fn lights(&self, budget: usize) -> Vec<Light> {
        let mut parts = Vec::new();
        for e in &self.emitters {
            let Some(world) = e.placed else {
                continue;
            };
            let linear = Mat3::from_mat4(world);
            let scale = linear.determinant().abs().powf(2. / 3.);
            let normals = linear.inverse().transpose();
            for p in e.patches.iter() {
                for lobe in &p.lobes {
                    let flux = lobe.emission * PI * scale;
                    if luminance(flux) < MIN_FLUX {
                        continue;
                    }
                    parts.push(Part::new(
                        world.transform_point3(p.centre),
                        flux,
                        normals * lobe.direction,
                        lobe.direction.length(),
                        (p.area * scale / PI).sqrt(),
                    ));
                }
            }
        }
        let mut cell = CELL;
        loop {
            let lights = cluster(&parts, cell);
            if lights.len() <= budget || cell > 1e4 {
                return lights.into_iter().take(budget).map(Part::light).collect();
            }
            cell *= 2.;
        }
    }
}

/// An emitter's light before clustering.
#[derive(Clone, Copy)]
struct Part {
    position: Vec3,
    flux: Vec3,
    /// Unit lobe axis of a spot; zero for a point.
    direction: Vec3,
    /// Outer cone cosine of a spot (the shading's smoothstep runs from it to one).
    outer: f32,
    /// Radius of an equal-area disk: the light's near-field softening.
    radius: f32,
}

impl Part {
    /// A lobe whose escaped rays average to `axis` with length `collimation`. The
    /// shading's spot weights smoothstep(outer, 1, cos), whose mean cosine is
    /// 0.7 + 0.3 x outer; matching it gives a Lambertian face (2/3) an outer cone
    /// of about 90 degrees. Lobes too broad for that are points.
    fn new(position: Vec3, flux: Vec3, axis: Vec3, collimation: f32, radius: f32) -> Self {
        let outer = (collimation - 0.7) / 0.3;
        let (direction, outer) = if outer > -0.9 {
            (axis.normalize_or_zero(), outer.min(0.95))
        } else {
            (Vec3::ZERO, 0.)
        };
        Self {
            position,
            flux,
            direction,
            outer,
            radius,
        }
    }

    fn light(self) -> Light {
        let lumens = luminance(self.flux).max(1e-6);
        let tint = self.flux / lumens;
        // Point-equivalent lumens (the shading convention): a spot's cone carries
        // its flux in pi x (1 - outer) of the 4 pi a point would fill.
        let spot = self.direction != Vec3::ZERO;
        let intensity = if spot {
            lumens * 4. / (1. - self.outer)
        } else {
            lumens
        };
        let range = (intensity / (4. * PI * CUTOFF_LUX)).sqrt().clamp(1., 80.);
        Light {
            position_range: [self.position.x, self.position.y, self.position.z, range],
            color_lumens: [tint.x, tint.y, tint.z, intensity],
            direction_outer: [
                self.direction.x,
                self.direction.y,
                self.direction.z,
                self.outer,
            ],
            // Unshadowed, so kept out of fog in-scattering (w).
            parameters: [f32::from(u8::from(spot)), 1., self.radius, 1.],
        }
    }
}

/// Merges parts sharing a grid cell and a facing into one light at their
/// flux-weighted position and cone; brightest first.
fn cluster(parts: &[Part], cell: f32) -> Vec<Part> {
    let mut cells: BTreeMap<([i64; 3], [i8; 3]), Part> = BTreeMap::new();
    for p in parts {
        let key = (
            (p.position / cell).floor().as_i64vec3().to_array(),
            (p.direction * 2.).round().as_i8vec3().to_array(),
        );
        cells
            .entry(key)
            .and_modify(|c| {
                let (a, b) = (luminance(c.flux), luminance(p.flux));
                let w = b / (a + b).max(1e-6);
                c.position = c.position.lerp(p.position, w);
                c.direction = (c.direction * a + p.direction * b).normalize_or_zero();
                c.outer += (p.outer - c.outer) * w;
                c.radius = (c.radius * c.radius + p.radius * p.radius).sqrt();
                c.flux += p.flux;
            })
            .or_insert(*p);
    }
    let mut lights: Vec<Part> = cells.into_values().collect();
    lights.sort_by(|a, b| luminance(b.flux).total_cmp(&luminance(a.flux)));
    lights
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_render::instanced::Instance;

    fn part(x: f32, flux: f32, direction: Vec3) -> Part {
        Part {
            position: Vec3::new(x, 3., 0.),
            flux: Vec3::splat(flux),
            direction,
            outer: 0.,
            radius: 0.05,
        }
    }

    /// The flux a light carries (inverting `Part::light`).
    fn flux(light: &Light) -> f32 {
        let outer = light.direction_outer[3];
        if light.parameters[0] > 0.5 {
            light.color_lumens[3] * (1. - outer) / 4.
        } else {
            light.color_lumens[3]
        }
    }

    /// Horizontal quads (x0, z0, x1, z1, y, u) facing down, each mapped to one
    /// column `u` of the emissive map.
    fn scene(quads: &[[f32; 6]], factor: f32, texture: u32) -> Scene {
        let (mut vertices, mut indices) = (Vec::new(), Vec::new());
        for &[x0, z0, x1, z1, y, u] in quads {
            let base = vertices.len() as u32;
            vertices.extend(
                [[x0, z0], [x1, z0], [x1, z1], [x0, z1]].map(|[x, z]| Vertex {
                    position: [x, y, z],
                    normal: [0., -1., 0.],
                    uv: [u, 0.5],
                    tangent: [1., 0., 0., 1.],
                    uv1: [0.; 2],
                    terrain_weights: 0,
                    reserved: 0,
                }),
            );
            indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        let mut material = Material::default();
        material.pbr.emissive_roughness = [factor, factor, factor, 0.5];
        material.pbr.textures[3] = texture;
        Scene {
            geometry: vec![Geometry {
                key: [1; 32],
                vertices,
                lods: vec![Lod { indices, error: 0. }],
                bounds: [Vec3::new(-10., 0., -10.), Vec3::new(10., 4., 10.)],
                deforming: false,
                revision: 0,
            }],
            instances: vec![Instance {
                entity: 1,
                name: String::new(),
                geometry: 0,
                material: 0,
                world: Mat4::IDENTITY,
                previous_world: Mat4::IDENTITY,
            }],
            materials: vec![material],
            textures: 0,
            lights: Vec::new(),
            sun_direction: Vec3::Y,
            sun_lux: Vec3::ZERO,
            environment: Vec::new(),
            environment_size: [0, 0],
        }
    }

    const BRIGHT: Shading = Shading {
        albedo: Vec3::ONE,
        emissive: Vec3::splat(100.),
        cutout: 0.,
    };

    #[test]
    fn nearby_emitters_cluster_only_as_far_as_the_budget_needs() {
        // A festoon string: twenty bulbs half a metre apart.
        let bulbs: Vec<Part> = (0..20)
            .map(|i| part(i as f32 * 0.5, 50., Vec3::ZERO))
            .collect();
        let fine = cluster(&bulbs, CELL);
        assert!(fine.len() >= 10, "{} lights", fine.len());
        let total: f32 = fine.iter().map(|p| p.flux.x).sum();
        assert!((total - 1000.).abs() < 1e-3, "flux is conserved: {total}");
        let coarse = cluster(&bulbs, CELL * 8.);
        assert!(coarse.len() < fine.len());
        // A downward panel and an upward one never merge.
        let panels = [part(0., 500., Vec3::NEG_Y), part(0.1, 500., Vec3::Y)];
        assert_eq!(cluster(&panels, 10.).len(), 2);
    }

    #[test]
    fn emitter_lights_follow_their_instances_and_recompute_only_on_motion() {
        // A 1 m2 open ceiling panel at 3 m (inside one patch cell), emitting
        // 100 cd/m2 from both faces.
        let quad = [-0.125, -0.125, 0.875, 0.875, 3., 0.];
        let mut scene = scene(&[quad], 100., NO_TEXTURE);
        let mut emitters = Emitters::new(&scene, &[BRIGHT], |_, _, _| None);
        let lights = emitters.update(&scene, 8).expect("first placement");
        let [down, up] = lights.as_slice() else {
            panic!("one open panel, two lights: {}", lights.len());
        };
        assert_eq!(down.position_range[..3], [0.375, 3., 0.375]);
        assert!(down.direction_outer[1] < -0.999, "a downward spot");
        assert!(up.direction_outer[1] > 0.999, "and an upward one");
        assert!(down.direction_outer[3].abs() < 0.2, "about a hemisphere");
        // Each face emits pi x radiance x area.
        assert!((flux(down) - 100. * PI).abs() < 0.5);
        assert!((flux(up) - 100. * PI).abs() < 0.5);
        assert!(emitters.update(&scene, 8).is_none(), "nothing moved");
        scene.instances[0].world = Mat4::from_translation(Vec3::new(2., 0., 0.));
        let moved = emitters.update(&scene, 8).expect("the panel moved");
        assert_eq!(moved[0].position_range[..3], [2.375, 3., 0.375]);
        assert!(emitters.update(&scene, 1).is_some(), "a new budget");
    }

    #[test]
    fn emission_follows_the_emissive_map_over_surface_area() {
        // A 4 m2 quad on the map's dark half and a 0.25 m2 one on its lit half:
        // the mean texel would credit the large quad; the surface integral does not.
        let scene = scene(
            &[[-1., -1., 1., 1., 3., 0.25], [3., 3., 3.5, 3.5, 3., 0.75]],
            100.,
            7,
        );
        let map: Map = ([2, 1], vec![[0.; 4], [1.; 4]]);
        let mut emitters = Emitters::new(&scene, &[BRIGHT], |texture, _, _| {
            (texture == 7).then(|| map.clone())
        });
        let lights = emitters.update(&scene, 8).expect("placed");
        assert_eq!(lights.len(), 2, "the small quad's two faces");
        assert!((flux(&lights[0]) - 100. * PI * 0.25).abs() < 0.2);
        assert_eq!(lights[0].position_range[..3], [3.25, 3., 3.25]);
    }

    #[test]
    fn emissive_maps_decode_where_their_material_decodes_srgb() {
        // A graph (raw view) material decodes its emissive map in the shader; the
        // CPU sampling must be asked for linear texels too.
        let mut scene = scene(&[[-1., -1., 1., 1., 3., 0.5]], 100., 7);
        let decoded = std::cell::Cell::new(None);
        let search = |scene: &Scene| {
            Emitters::new(scene, &[BRIGHT], |_, _, decode| {
                decoded.set(Some(decode));
                None
            })
        };
        search(&scene);
        assert_eq!(decoded.take(), Some(false), "cooked views arrive linear");
        scene.materials[0].pbr.flags[1] = EMISSIVE_DECODE_SRGB;
        search(&scene);
        assert_eq!(decoded.take(), Some(true));
    }

    #[test]
    fn a_fixture_absorbs_what_does_not_escape_it() {
        // A lit panel under a wide dark cap 5 cm above it: the panel's upper face
        // is shaded by its own fixture, so only the downward lobe remains.
        let scene = scene(
            &[
                [0.1, 0.1, 0.6, 0.6, 3., 0.75],
                [-1., -1., 1.7, 1.7, 3.05, 0.25],
            ],
            100.,
            7,
        );
        let map: Map = ([2, 1], vec![[0.; 4], [1.; 4]]);
        let mut emitters = Emitters::new(&scene, &[BRIGHT], |_, _, _| Some(map.clone()));
        let lights = emitters.update(&scene, 8).expect("placed");
        let [down] = lights.as_slice() else {
            panic!("only the downward lobe escapes: {}", lights.len());
        };
        assert!(down.direction_outer[1] < -0.999);
        assert!((flux(down) - 100. * PI * 0.25).abs() < 0.2);
    }

    #[test]
    fn a_closed_thin_panel_emits_once_each_way() {
        // A thin box straddling a patch boundary in y: its downward bottom face and
        // upward top face (both seen from outside) gather into separate cells.
        let mut scene = scene(&[[0.05, 0.05, 0.65, 0.65, -0.01, 0.]], 100., NO_TEXTURE);
        let geometry = &mut scene.geometry[0];
        let top: Vec<Vertex> = geometry
            .vertices
            .iter()
            .map(|v| Vertex {
                position: [v.position[0], 0.01, v.position[2]],
                normal: [0., 1., 0.],
                ..*v
            })
            .collect();
        geometry.vertices.extend(top);
        geometry.lods[0].indices.extend([4, 6, 5, 4, 7, 6]);
        let mut emitters = Emitters::new(&scene, &[BRIGHT], |_, _, _| None);
        let lights = emitters.update(&scene, 8).expect("placed");
        assert_eq!(lights.len(), 2, "one spot per face, none mirrored");
        assert!(lights.iter().any(|l| l.direction_outer[1] < -0.999));
        assert!(lights.iter().any(|l| l.direction_outer[1] > 0.999));
        let face = 100. * PI * 0.36;
        assert!(lights.iter().all(|l| (flux(l) - face).abs() < 0.2));
    }

    #[test]
    fn emission_edits_rerun_the_search_and_the_last_edit_lands() -> Result<()> {
        let quad = [-0.125, -0.125, 0.875, 0.875, 3., 0.];
        let mut scene = scene(&[quad], 100., NO_TEXTURE);
        let mut dark = Material::default();
        dark.pbr.emissive_roughness = [1., 1., 1., 0.5];
        scene.materials.push(dark);
        let shading = |scene: &Scene| -> Vec<Shading> {
            scene
                .materials
                .iter()
                .map(|m| Shading {
                    albedo: Vec3::ONE,
                    emissive: Vec3::from_slice(&m.pbr.emissive_roughness[..3]),
                    ..Shading::default()
                })
                .collect()
        };
        let job = |scene: &Scene| Job::new(scene, &shading(scene), |_, _, _| None);
        let current = job(&scene).run();
        let none = |_, _| None;
        assert!(
            !emission_edited(Some(&current), &scene, none, &[1]),
            "still dark"
        );
        assert!(
            emission_edited(Some(&current), &scene, none, &[0]),
            "an emitter edited"
        );
        scene.materials[1].pbr.emissive_roughness = [50., 50., 50., 0.5];
        assert!(
            emission_edited(Some(&current), &scene, none, &[1]),
            "now an emitter"
        );

        // A slider: edits while a search runs queue one rerun of the newest scene.
        let mut search = Search::default();
        scene.materials[0].pbr.emissive_roughness = [200., 200., 200., 0.5];
        search.request(|| job(&scene))?;
        scene.materials[0].pbr.emissive_roughness = [300., 300., 300., 0.5];
        search.request(|| unreachable!("queued, not built"))?;
        search.request(|| unreachable!("queued, not built"))?;
        assert!(search.is_busy());
        let first = search
            .poll(true, || job(&scene))?
            .expect("the first search");
        let second = search.poll(true, || unreachable!())?.expect("the rerun");
        assert!(!search.is_busy());
        assert!(
            second.lumens() > first.lumens() * 1.4,
            "the last edit landed"
        );
        Ok(())
    }

    #[test]
    fn a_lambertian_face_is_a_hemispherical_spot_with_its_peak() {
        let light = Part::new(Vec3::ZERO, Vec3::splat(100.), Vec3::NEG_Y, 2. / 3., 0.05).light();
        assert_eq!(light.parameters[0], 1., "spot");
        assert!(
            light.direction_outer[3].abs() < 0.15,
            "about a 90 degree cone"
        );
        assert!((flux(&light) - 100.).abs() < 1e-3, "flux is conserved");
        let bulb = Part::new(Vec3::ZERO, Vec3::splat(100.), Vec3::Y * 0.05, 0.05, 0.05).light();
        assert_eq!(bulb.parameters[0], 0., "a broad lobe is a point");
        assert!((bulb.color_lumens[3] - 100.).abs() < 1e-3);
    }
}
