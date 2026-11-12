//! Explicit native-scene adapter. Construction and updates are opt-in host calls;
//! the renderer only reads the published FP32 surface irradiance at draw time.

use super::{
    Budget, DirectionScheme, EventSolver, TraceGraph,
    graph::{self, BuildSettings, Materials, Optical, Query},
    readout::SurfaceMap,
    visibility::{CpuTrace, SurfaceHit},
};
use crate::{
    instanced::Scene,
    look::{
        trace::{self, TraceInstance, TraceScene},
        transport::{
            changes::{BodyClass, TransportChanges},
            links::ExternalSource,
            media::{IrradianceVolume, MediaGrid},
            patches::{PatchRegistry, Placement},
            reverse::{Memory, Region},
        },
    },
};
use anyhow::{Result, ensure};
use ara_bvh::Ray;
use ara_patch_charts::{Charts, Mesh, Settings, Triangle, cache};
use glam::{Affine3A, IVec3, Vec3, Vec4};
use std::{collections::BTreeMap, path::Path, sync::Arc, time::Instant};
mod channel_bake;
mod forcing;
mod material;
mod receivers;
mod source;
mod source_support;
use material::*;
use source::{lamp, light_gain};

/// A derived smoke or sub-patch volume snapshot. The host owns simulation state;
/// transport rasterizes this snapshot into fixed 25 cm cells at the next update.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransportMedium {
    /// Minimum world-space corner in metres.
    pub min: [f32; 3],
    /// Maximum world-space corner in metres.
    pub max: [f32; 3],
    /// Occupied fraction of the submitted box, from zero to one.
    pub occupancy: f32,
    /// Extinction coefficient in inverse metres.
    pub extinction: f32,
    /// Linear scattering colour from zero to one.
    pub albedo: [f32; 3],
    /// Linear outgoing source radiance; Beer-Lambert integration supplies opacity.
    pub outgoing: [f32; 3],
}

/// Settings for the off-by-default patch A/B. Resolution is always 25 cm.
#[derive(Clone, Copy, Debug)]
pub struct TransportOptions {
    /// Number of deterministic hemisphere directions at each patch.
    pub directions: u32,
    /// True uses world-global directions; false uses patch-local cosine directions.
    pub global_directions: bool,
    /// Capped patch updates per frame. R5-3 measured about 78k/ms on the 760M GPU;
    /// the CPU implementation reports its actual wall cost separately.
    pub patch_passes: usize,
    /// Maximum whole link repairs in an event frame.
    pub repair_links: usize,
    /// Absolute linear-radiance stopping tolerance for the patch system.
    pub tolerance: f32,
    /// Maximum resident per-light response data; zero disables optional channels.
    pub channel_bytes: usize,
}
impl Default for TransportOptions {
    fn default() -> Self {
        Self {
            directions: 48,
            global_directions: false,
            patch_passes: 78_000,
            repair_links: 78_000,
            tolerance: 1e-5,
            channel_bytes: 0,
        }
    }
}

/// Measured work and persistent debt. CPU ms are never labelled GPU timestamps.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct TransportReport {
    /// Complete host discovery/forcing/repair/solve wall time for this update.
    pub event_cpu_ms: f64,
    /// Receiver volume acquisition and media splatting, inside the event total.
    pub receiver_cpu_ms: f64,
    /// Link repair wall time, inside the event total.
    pub repair_cpu_ms: f64,
    /// Source forcing and dependency marking, inside the event total.
    pub source_cpu_ms: f64,
    /// Capped or uncapped numerical solve, inside the event total.
    pub solve_cpu_ms: f64,
    /// Actual completed patch updates.
    pub patch_passes: usize,
    /// Number of deferred cached-ray repairs.
    pub repair_debt: usize,
    /// Age of the oldest pending solve or repair in render frames.
    pub oldest_debt: u64,
    /// Number of blocks already at/past the three-frame deadline.
    pub missed_deadlines: usize,
    /// Residual/norm estimate for the published patch system; excludes FP32 roundoff
    /// and geometric quadrature error. G3 independently compares an FP64 solution.
    /// An unbounded estimate is infinity, represented by null in JSON receipts.
    #[serde(deserialize_with = "read_solution_bound")]
    pub solution_bound: f32,
    /// Accounted host transport residency, including private acceleration structures.
    /// Array capacities are exact; private map/node overhead is conservatively estimated.
    pub resident_bytes: usize,
    /// A2 forward + reverse + index, with actual sampled-ray overrides.
    pub link_bytes: usize,
    /// Resident optional channel data on the host; GPU copies are charged separately.
    pub channel_bytes: usize,
    /// Index bytes per link, including original and repaired geometry witnesses.
    pub index_bytes_per_link: f64,
    /// Whether this frame required a cold topology publication outside repair packets.
    pub topology_publication: bool,
}

/// Recover the unbounded estimate represented by JSON's nonfinite-number null.
fn read_solution_bound<'de, D: serde::Deserializer<'de>>(reader: D) -> Result<f32, D::Error> {
    Ok(<Option<f32> as serde::Deserialize>::deserialize(reader)?.unwrap_or(f32::INFINITY))
}

struct ChartInstance {
    charts: Arc<Charts>,
    incarnation: u32,
}

/// A host-owned patch solve for one resident scene. It is independent of screen
/// dimensions/camera and retains no screen colour. The renderer can enable/disable
/// its readout without changing today's probe and screen-GI controls.
pub struct PatchTransport {
    pub(super) channels: super::channels::Channels,
    pub(crate) graph: TraceGraph,
    pub(crate) solver: EventSolver,
    pub(super) external_irradiance: Vec<Vec3>,
    pub(crate) trace: TraceScene,
    surfaces: SurfaceMap,
    charts: BTreeMap<u64, ChartInstance>,
    /// Local cache revisions advanced only by A3 motion/appearance events.
    geometry_generations: Vec<u64>,
    registry: PatchRegistry,
    media: MediaGrid,
    media_sources: Vec<TransportMedium>,
    pub(super) volume: IrradianceVolume,
    bodies: BTreeMap<u64, BodyClass>,
    body_sources: BTreeMap<u64, u32>,
    body_radiance: BTreeMap<u64, Vec3>,
    options: TransportOptions,
    frame: u64,
    distance: f32,
    report: TransportReport,
}

impl PatchTransport {
    /// Build from native LOD0 and optional CPU texture pixels. `bodies` uses A3's
    /// publication-local instance IDs. Cache roots must be project/scratch owned.
    pub fn new(
        scene: &Scene,
        textures: &[ara_frame::scene::Texture],
        bodies: BTreeMap<u64, BodyClass>,
        options: TransportOptions,
        cache_root: Option<&Path>,
    ) -> Result<Self> {
        let started = Instant::now();
        ensure!(
            options.directions > 0 && options.tolerance > 0. && options.tolerance.is_finite(),
            "invalid transport options"
        );
        ensure!(
            scene.lights.iter().all(|l| l.tables[0] == 0),
            "CPU transport forcing needs photometric table pixels before accepting profiled lights"
        );
        let mut trace = TraceScene::default();
        let mut meshes = Vec::with_capacity(scene.geometry.len());
        let blas_cache = cache_root.map(|p| trace::BlasCache::new(p.join("bvh")));
        for geometry in &scene.geometry {
            let (mesh, _) = trace::mesh_from_geometry(geometry, blas_cache.as_ref(), true)?;
            meshes.push(mesh.blas.hash);
            trace.add_mesh(mesh);
        }
        let mut registry = PatchRegistry::default();
        let mut charts = BTreeMap::new();
        let mut minimum = Vec3::splat(f32::INFINITY);
        let mut maximum = Vec3::splat(f32::NEG_INFINITY);
        for (row, instance) in scene.instances.iter().enumerate() {
            let id = row as u64;
            let geometry = &scene.geometry[instance.geometry];
            let bounds = instance.bounds(geometry);
            minimum = minimum.min(bounds[0]);
            maximum = maximum.max(bounds[1]);
            let small = !bodies.get(&id).copied().unwrap_or_default().owns_links()
                && (bounds[1] - bounds[0]).max_element() < 0.5;
            let key = trace.set_instance(
                id,
                TraceInstance {
                    mesh: meshes[instance.geometry],
                    world: Affine3A::from_mat4(instance.world),
                    mask: if small { 0 } else { 255 },
                    materials: vec![trace::NO_MATERIAL],
                    user: row as u32,
                },
            )?;
            if !bodies.get(&id).copied().unwrap_or_default().owns_links() {
                continue;
            }
            let (scale, rotation, translation) = instance.world.to_scale_rotation_translation();
            let reconstructed =
                glam::Mat4::from_scale_rotation_translation(scale, rotation, translation);
            ensure!(
                instance
                    .world
                    .to_cols_array()
                    .iter()
                    .zip(reconstructed.to_cols_array())
                    .all(|(a, b)| (a - b).abs() < 1e-4),
                "transport chart instance has shear; bake it into geometry"
            );
            let positions: Vec<_> = geometry.vertices.iter().map(|v| v.position).collect();
            let material = scene.materials[instance.material];
            let albedo = std::array::from_fn(|i| {
                material.pbr.base_color[i]
                    * (1. - material.pbr.surface[0])
                    * (1. - material.extras.transmission_factor)
            });
            let triangles: Vec<_> = geometry.lods[0]
                .indices
                .as_chunks::<3>()
                .0
                .iter()
                .map(|&vertices| Triangle {
                    vertices,
                    material: instance.material as u32,
                    albedo,
                })
                .collect();
            let mesh = Mesh {
                positions: &positions,
                triangles: &triangles,
            };
            let settings = Settings {
                scale: scale.to_array(),
                ..Settings::default()
            };
            let key_chart = cache::Key::new(mesh, settings);
            let path = cache_root.map(|p| p.join(key_chart.relative_path()));
            let cached = path.as_ref().and_then(|p| std::fs::read(p).ok());
            let prepared = cache::prepare(mesh, settings, cached.as_deref())?;
            if let (Some(path), Some(bytes)) = (path, prepared.write_back) {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
                std::fs::write(&temporary, bytes)?;
                std::fs::rename(&temporary, &path)?;
            }
            let chart = Arc::new(prepared.charts);
            registry
                .publish_traced(
                    id,
                    key,
                    prepared.key,
                    Arc::clone(&chart),
                    0,
                    Placement {
                        rotation: glam::Mat3::from_quat(rotation).to_cols_array_2d(),
                        translation: translation.to_array(),
                    },
                )
                .map_err(|e| anyhow::anyhow!("publish charts: {e:?}"))?;
            charts.insert(
                id,
                ChartInstance {
                    charts: chart,
                    incarnation: 1,
                },
            );
        }
        trace.commit();
        let patches = Arc::new(
            registry
                .snapshot()
                .map_err(|e| anyhow::anyhow!("patch snapshot: {e:?}"))?,
        );
        let mut surfaces = SurfaceMap::default();
        for (&id, chart) in &mut charts {
            chart.incarnation = patches
                .stamps
                .iter()
                .find(|p| p.id.instance == id)
                .map_or(1, |p| p.id.incarnation);
            surfaces.add(id, chart.incarnation, &chart.charts, &patches);
        }
        let distance = (maximum - minimum).length().max(1.) * 2.;
        let boundary = patches.len() as u32;
        let mut external = vec![ExternalSource {
            id: u64::MAX,
            generation: 1,
            position: [distance; 3],
        }];
        let mut body_sources = BTreeMap::new();
        for (&id, body) in &bodies {
            if !body.owns_links() {
                body_sources.insert(id, boundary + external.len() as u32);
                external.push(ExternalSource {
                    id,
                    generation: 1,
                    position: scene.instances[id as usize]
                        .world
                        .w_axis
                        .truncate()
                        .to_array(),
                });
            }
        }
        let media = MediaGrid::default();
        let geometry_generations = vec![1; scene.instances.len()];
        let materials = NativeMaterials {
            scene,
            textures,
            bodies: &bodies,
            generations: &geometry_generations,
        };
        let cpu = CpuTrace::new(&trace);
        let query = Query {
            trace: &cpu,
            surfaces: &surfaces,
            materials: &materials,
            media: &media,
            bodies: &body_sources,
            boundary,
        };
        let graph = TraceGraph::build(
            patches,
            external,
            &query,
            BuildSettings {
                directions: options.directions,
                distance,
                scheme: if options.global_directions {
                    DirectionScheme::Global
                } else {
                    DirectionScheme::PerPatch
                },
            },
        )?;
        let albedo = graph
            .links
            .patches
            .stamps
            .iter()
            .map(|stamp| {
                materials
                    .patch(
                        stamp.id.instance,
                        &charts[&stamp.id.instance].charts,
                        stamp.id.local,
                    )
                    .0
            })
            .collect();
        let solver = EventSolver::new_albedo(&graph.links, albedo)?;
        let external_irradiance = vec![Vec3::ZERO; graph.links.patches.len()];
        let mut result = Self {
            channels: super::channels::Channels::new(options.channel_bytes / 2),
            graph,
            solver,
            external_irradiance,
            trace,
            surfaces,
            charts,
            geometry_generations,
            registry,
            media,
            media_sources: Vec::new(),
            volume: IrradianceVolume::default(),
            bodies,
            body_sources,
            body_radiance: BTreeMap::new(),
            options,
            frame: 0,
            distance,
            report: TransportReport::default(),
        };
        result.refresh_forcing(scene, textures, None)?;
        let stats = result.solver.run(
            &result.graph.links,
            &result.graph,
            Budget {
                patch_passes: usize::MAX,
                reverse_edges: usize::MAX,
                solution_tolerance: options.tolerance,
            },
        )?;
        result.report.patch_passes = stats.patch_passes;
        result.report.solution_bound = stats.solution_bound;
        // Awake bodies have readout at the first publication too. Their derived
        // media/large-body response changes crossing links and external forcing.
        if result.bodies.values().any(|b| !b.owns_links()) {
            result.initialize_receivers(scene, textures)?;
        }
        if options.channel_bytes > 0 {
            result.bake_channels(scene, textures)?;
        }
        result.report.event_cpu_ms = started.elapsed().as_secs_f64() * 1000.;
        result.account();
        Ok(result)
    }

    /// Current complete accounting for the most recent publication/update.
    pub fn report(&self) -> &TransportReport {
        &self.report
    }

    /// Publish a complete derived media snapshot. Old and new coverage invalidate
    /// crossing links, including links that were previously completely clear.
    pub fn set_media(&mut self, sources: &[TransportMedium]) -> Result<()> {
        for source in sources {
            let lo = Vec3::from(source.min);
            let hi = Vec3::from(source.max);
            let albedo = Vec3::from(source.albedo);
            let outgoing = Vec3::from(source.outgoing);
            ensure!(
                lo.is_finite()
                    && hi.is_finite()
                    && hi.cmpgt(lo).all()
                    && source.occupancy.is_finite()
                    && (0. ..=1.).contains(&source.occupancy)
                    && source.extinction.is_finite()
                    && source.extinction >= 0.
                    && albedo.is_finite()
                    && albedo.cmpge(Vec3::ZERO).all()
                    && albedo.cmple(Vec3::ONE).all()
                    && super::valid_radiance(outgoing),
                "invalid transport medium"
            );
        }
        if self.media_sources == sources {
            return Ok(());
        }
        for source in self.media_sources.iter().chain(sources) {
            if let Some(region) = Region::new(source.min, source.max) {
                self.graph.discover(region, self.frame + 1);
            }
        }
        // A newly introduced medium changes support even without a host material event.
        if !self.channels.is_empty() {
            for p in 0..self.solver.radiance.len() {
                self.solver.radiance[p] += self.channels.read(p as u32).map_or(Vec3::ZERO, |v| v.0);
            }
            self.channels.clear();
            self.graph.invalidate_all(self.frame + 1);
        }
        self.media_sources.clear();
        self.media_sources.extend_from_slice(sources);
        Ok(())
    }

    /// Independent exact patch-system reference; geometry/media approximation is
    /// assessed separately against current-geometry path tracing.
    pub fn exact_patch_solution(&self) -> Result<Vec<[f32; 3]>> {
        Ok(self
            .solver
            .exact(&self.graph.links, 1e-9)?
            .into_iter()
            .enumerate()
            .map(|(p, v)| (v + self.channels.read(p as u32).map_or(Vec3::ZERO, |v| v.0)).to_array())
            .collect())
    }

    /// Current outgoing surface radiance for per-frame gate scoring.
    pub fn patch_radiance(&self) -> Vec<[f32; 3]> {
        self.solver
            .radiance
            .iter()
            .enumerate()
            .map(|(i, v)| {
                (*v + self.channels.read(i as u32).map_or(Vec3::ZERO, |r| r.0)).to_array()
            })
            .collect()
    }

    /// Consume an A3 frame. Uncapped replay uses identical events and geometry,
    /// with unlimited repair/solve packets. CPU preparation is fully timed; cold
    /// publication is explicitly reported because it is not yet packetized.
    pub fn advance(
        &mut self,
        scene: &Scene,
        textures: &[ara_frame::scene::Texture],
        events: &TransportChanges,
        uncapped: bool,
    ) -> Result<&TransportReport> {
        let started = Instant::now();
        ensure!(
            events.frame > self.frame,
            "transport replay frame must increase"
        );
        ensure!(
            scene.lights.iter().all(|l| l.tables[0] == 0),
            "CPU transport forcing needs photometric table pixels before accepting profiled lights"
        );
        let previous_bytes = self.report.resident_bytes;
        let row_light_ids = events.lights.iter().all(|event| {
            event.current.is_none_or(|state| {
                scene
                    .lights
                    .get(event.id as usize)
                    .is_some_and(|light| bytemuck::bytes_of(light) == bytemuck::bytes_of(&state.0))
            })
        });
        let channels_changed = !self.channels.is_empty()
            && (events.reset
                || !row_light_ids
                || !events.instances.is_empty()
                || !events.geometries.is_empty()
                || !events.materials.is_empty()
                || !events.portals.is_empty()
                || events.lights.iter().any(|e| {
                    e.flags
                        & (crate::look::transport::changes::flags::ADDED
                            | crate::look::transport::changes::flags::REMOVED
                            | crate::look::transport::changes::flags::MOVED
                            | crate::look::transport::changes::flags::SHAPE)
                        != 0
                }));
        if channels_changed {
            for p in 0..self.solver.radiance.len() {
                self.solver.radiance[p] += self.channels.read(p as u32).map_or(Vec3::ZERO, |v| v.0);
            }
            self.channels.clear();
        }
        for event in &events.lights {
            if let Some(state) = event.current {
                let light: ara_frame::scene::Light =
                    bytemuck::pod_read_unaligned(bytemuck::bytes_of(&state.0));
                self.channels.set_gain(event.id, light_gain(&light))?;
            }
        }
        let mut topology = events.reset || !events.geometries.is_empty();
        if events.reset {
            // Row IDs are scoped to this publication. A reset has no per-row
            // tombstones for the old layout, so retain no body metadata from it.
            self.bodies.clear();
        }
        for event in &events.instances {
            let change = &event.change;
            let before = self.bodies.get(&change.id).copied().unwrap_or_default();
            match change.current {
                Some(current) => {
                    self.bodies.insert(change.id, current.body);
                    topology |= before.owns_links() != current.body.owns_links();
                }
                None => {
                    self.bodies.remove(&change.id);
                    topology = true;
                }
            }
            topology |= change.previous.is_none();
            // Static rigid moves preserve chart IDs, but dense snapshot centroids
            // must be republished. Moving receivers only refit the TLAS.
            topology |= before.owns_links()
                && change.flags & crate::look::transport::changes::flags::MOVED != 0;
        }
        if topology {
            let old_origin = self.solver.oldest_dirty();
            let bodies = self.bodies.iter().map(|(&id, &body)| (id, body)).collect();
            let mut replacement = Self::new(scene, textures, bodies, self.options, None)?;
            replacement.set_media(&self.media_sources)?;
            replacement.frame = events.frame;
            replacement.solver.begin_frame(events.frame)?;
            if !replacement.media_sources.is_empty() {
                replacement.initialize_receivers(scene, textures)?;
            }
            if let Some(origin) = old_origin {
                replacement.solver.retain_debt(origin);
            }
            replacement.report.topology_publication = true;
            replacement.account();
            replacement.report.resident_bytes += previous_bytes;
            replacement.report.event_cpu_ms = started.elapsed().as_secs_f64() * 1000.;
            *self = replacement;
            return Ok(&self.report);
        }
        self.frame = events.frame;
        self.solver.begin_frame(events.frame)?;
        if events.instances.is_empty()
            && events.geometries.is_empty()
            && events.materials.is_empty()
            && events.lights.is_empty()
            && events.portals.is_empty()
            && events.sun.is_none()
            && self.bodies.values().all(|b| b.owns_links())
            && self.graph.debt(events.frame).0 == 0
            && self.solver.solution_bound() <= self.options.tolerance
            && !channels_changed
        {
            self.report = TransportReport {
                event_cpu_ms: started.elapsed().as_secs_f64() * 1000.,
                resident_bytes: self.report.resident_bytes,
                link_bytes: self.report.link_bytes,
                channel_bytes: self.report.channel_bytes,
                index_bytes_per_link: self.report.index_bytes_per_link,
                solution_bound: self.solver.solution_bound(),
                ..TransportReport::default()
            };
            return Ok(&self.report);
        }
        let mut affected = std::collections::BTreeSet::new();
        let mut changed_regions = Vec::new();
        let geometry_revision = events
            .frame
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("transport geometry revision exhausted"))?;
        if channels_changed {
            affected.extend(0..self.graph.links.patches.len() as u32);
        }
        for event in &events.instances {
            self.geometry_generations[event.change.id as usize] = geometry_revision;
            self.graph.changed(event.change.id);
            if let Some(current) = event.change.current {
                let mask = if !current.body.owns_links() && !current.is_large() {
                    0
                } else {
                    255
                };
                let key = self
                    .trace
                    .instance_key(event.change.id)
                    .ok_or_else(|| anyhow::anyhow!("changed trace instance is missing"))?;
                let slot = self
                    .trace
                    .slot(key.slot)
                    .expect("live trace key has a slot");
                if slot.instance.mask != mask {
                    let mesh = slot.instance.mesh;
                    self.trace.set_instance(
                        event.change.id,
                        TraceInstance {
                            mesh,
                            world: Affine3A::from_mat4(current.world),
                            mask,
                            materials: vec![trace::NO_MATERIAL],
                            user: event.change.id as u32,
                        },
                    )?;
                } else {
                    self.trace
                        .set_transform(event.change.id, Affine3A::from_mat4(current.world));
                }
            }
            if let Some(region) =
                Region::new(event.swept.min.to_array(), event.swept.max.to_array())
            {
                changed_regions.push(region);
                // Sub-patch bodies use the media grid, not triangle visibility.
                // Project only edits that can alter the direct triangle query.
                if [event.change.previous, event.change.current]
                    .into_iter()
                    .flatten()
                    .any(|state| state.body.owns_links() || state.is_large())
                {
                    self.shadow_receivers(scene, region, &mut affected);
                }
            }
            for (i, stamp) in self.graph.links.patches.stamps.iter().enumerate() {
                if stamp.id.instance == event.change.id {
                    affected.insert(i as u32);
                }
            }
        }
        self.trace.commit();
        if !events.portals.is_empty() {
            self.geometry_generations.fill(geometry_revision);
            self.graph.invalidate_all(events.frame);
        }
        for event in &events.materials {
            for (i, &material) in self.graph.links.patches.material.iter().enumerate() {
                if u64::from(material) == event.id {
                    if let Some(current) = scene.materials.get(material as usize) {
                        self.solver.albedo[i] = Vec3::new(
                            current.pbr.base_color[0],
                            current.pbr.base_color[1],
                            current.pbr.base_color[2],
                        ) * (1. - current.pbr.surface[0])
                            * (1. - current.extras.transmission_factor);
                    }
                    affected.insert(i as u32);
                }
            }
            // Alpha/transmission can expose a previously hidden suffix. All rays
            // through the edited instance's current box are rediscovered.
            for (id, instance) in scene
                .instances
                .iter()
                .enumerate()
                .filter(|(_, i)| i.material as u64 == event.id)
            {
                self.geometry_generations[id] = geometry_revision;
                self.graph.changed(id as u64);
                let bounds = instance.bounds(&scene.geometry[instance.geometry]);
                if let Some(region) = Region::new(bounds[0].to_array(), bounds[1].to_array()) {
                    changed_regions.push(region);
                    self.shadow_receivers(scene, region, &mut affected);
                }
            }
        }
        self.graph
            .discover_regions(&mut changed_regions, events.frame);
        let receiver_started = Instant::now();
        self.update_receivers(scene, textures)?;
        let receiver_cpu_ms = receiver_started.elapsed().as_secs_f64() * 1000.;
        // Large movers are external source rows. Their reflected radiance must
        // reach every receiver before a receiver can be skipped this frame.
        for &source in self.body_sources.values() {
            for link in super::scheduler::Dependencies::edges(&self.graph, source) {
                affected.insert(self.graph.links.receiver(link));
            }
        }
        let errors = self.solver.errors();
        let cpu = CpuTrace::new(&self.trace);
        let materials = NativeMaterials {
            scene,
            textures,
            bodies: &self.bodies,
            generations: &self.geometry_generations,
        };
        let query = Query {
            trace: &cpu,
            surfaces: &self.surfaces,
            materials: &materials,
            media: &self.media,
            bodies: &self.body_sources,
            boundary: self.graph.links.patches.len() as u32,
        };
        let repair_started = Instant::now();
        let repaired = self.graph.repair(
            &query,
            &errors,
            if uncapped {
                usize::MAX
            } else {
                self.options.repair_links
            },
            events.frame + 1,
        )?;
        let repair_cpu_ms = repair_started.elapsed().as_secs_f64() * 1000.;
        let source_started = Instant::now();
        for &(patch, _) in &repaired {
            affected.insert(patch);
        }
        // Source/material preparation is charged to the event total. Finite light
        // support selects only patches in old/new ranges; the sun is global.
        for event in &events.lights {
            if self.channels.contains(event.id) {
                continue;
            }
            for state in [event.previous, event.current].into_iter().flatten() {
                let light: ara_frame::scene::Light =
                    bytemuck::pod_read_unaligned(bytemuck::bytes_of(&state.0));
                let position = Vec3::new(
                    light.position_range[0],
                    light.position_range[1],
                    light.position_range[2],
                );
                let radius = light.position_range[3];
                for p in 0..self.graph.links.patches.len() as u32 {
                    if Vec3::from(self.graph.links.patches.position(p)).distance_squared(position)
                        < radius * radius
                    {
                        affected.insert(p);
                    }
                }
            }
        }
        if events.sun.is_some() {
            affected.extend(0..self.graph.links.patches.len() as u32);
        }
        let rows: Vec<_> = affected.into_iter().collect();
        self.refresh_forcing(scene, textures, Some(&rows))?;
        for &patch in &rows {
            let origin = repaired
                .iter()
                .find(|(p, _)| *p == patch)
                .map_or(events.frame, |(_, f)| *f);
            self.solver.refresh_row(&self.graph.links, patch, origin);
        }
        if !events.materials.is_empty() || !repaired.is_empty() {
            self.solver
                .update_contraction_rows(&self.graph.links, &rows)?;
        }
        let source_cpu_ms = source_started.elapsed().as_secs_f64() * 1000.;
        let solve_started = Instant::now();
        let stats = self.solver.run(
            &self.graph.links,
            &self.graph,
            Budget {
                patch_passes: if uncapped {
                    usize::MAX
                } else {
                    self.options.patch_passes
                },
                reverse_edges: if uncapped {
                    usize::MAX
                } else {
                    self.options
                        .patch_passes
                        .saturating_mul(self.options.directions as usize)
                },
                solution_tolerance: self.options.tolerance,
            },
        )?;
        let (repair_debt, repair_age) = self.graph.debt(events.frame);
        self.report = TransportReport {
            event_cpu_ms: started.elapsed().as_secs_f64() * 1000.,
            receiver_cpu_ms,
            repair_cpu_ms,
            source_cpu_ms,
            solve_cpu_ms: solve_started.elapsed().as_secs_f64() * 1000.,
            patch_passes: stats.patch_passes,
            repair_debt,
            oldest_debt: stats.oldest_age.max(repair_age),
            missed_deadlines: stats.missed_deadlines + usize::from(repair_age >= 3),
            solution_bound: if repair_debt > 0 {
                f32::INFINITY
            } else {
                stats.solution_bound
            },
            ..TransportReport::default()
        };
        self.account();
        Ok(&self.report)
    }

    fn account(&mut self) {
        let memory = Memory::measure(&self.graph.links, &self.graph.reverse, &self.graph.index);
        self.report.link_bytes = memory.link_bytes()
            + self.graph.repaired_index_bytes()
            + self.graph.repaired_reverse_bytes()
            + self.geometry_generations.capacity() * 8;
        self.report.index_bytes_per_link = if self.graph.links.len() == 0 {
            0.
        } else {
            (memory.index
                + self.graph.repaired_index_bytes()
                + self.geometry_generations.capacity() * 8) as f64
                / self.graph.links.len() as f64
        };
        self.report.channel_bytes = self.channels.resident_bytes();
        self.report.resident_bytes = memory.resident_bytes()
            + self.graph.extra_bytes()
            + self.solver.resident_bytes()
            + self.channels.resident_bytes()
            + self.external_irradiance.capacity() * size_of::<Vec3>()
            + self.geometry_generations.capacity() * 8
            + self.surfaces.resident_bytes()
            + self.media.resident_bytes()
            + self.media_sources.capacity() * size_of::<TransportMedium>()
            + self.volume.resident_bytes()
            + self.trace_bytes()
            // Registry/chart-instance/body maps: conservative per-entry allowance
            // including allocator/node slack; chart payloads are counted below.
            + self.charts.len() * 512
            + (self.bodies.len() + self.body_sources.len() + self.body_radiance.len()) * 128
            + self
                .charts
                .values()
                .map(|c| c.charts.resident_bytes())
                .sum::<usize>();
    }

    fn trace_bytes(&self) -> usize {
        self.trace
            .blas_slots()
            .map(|(_, mesh)| {
                (mesh.blas.nodes.capacity() + mesh.blas.triangles.capacity()) * 16
                    + mesh.blas.submeshes.capacity() * 4
                    + mesh.uvs.as_ref().map_or(0, |uvs| uvs.capacity() * 48)
                    + 512
            })
            .sum::<usize>()
            + self.trace.tlas().nodes.capacity() * size_of::<ara_bvh::tlas::TlasNode>()
            + self.trace.instance_slots().len() * 512
            + self.trace.blas_slot_count() * 64
    }

    /// Incident diffuse irradiance for native readout. The patch solution stores
    /// outgoing radiance; gathering once is readout, never another coarser solve.
    pub(crate) fn irradiance(&self, patch: u32, _scene: &Scene) -> Vec3 {
        self.solver.gathered(patch as usize) * std::f32::consts::PI
            + self.external_irradiance[patch as usize]
    }
}
