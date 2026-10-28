//! Local reflection captures (shaders/look-captures.slang(h)): box-influenced,
//! box-projected octahedral radiance that the raster renderer bakes of its own lit
//! scene (six unjittered 90-degree faces of a dedicated view), GGX-prefiltered into
//! five roughness levels per capture in one RGBA16F atlas, and relit every frame by
//! the ratio of the ambient light at each capture point now to what it was at its
//! bake (id Tech style re-fit). Shading reads them in the High/Epic PSOs only.
//!
//! [`CaptureSpec`]s come from authored `ReflectionProbe` components or from
//! [`auto_place`]; `Renderer::set_reflection_captures` hands them over. [`Captures`]
//! plans the bakes: on new specs, a new content generation or a lighting change
//! (sun, local lights, sky, fog, probe GI arrival; rate-limited), never per frame.
//! Live views bake one step per frame (a face, or the reduce and prefilter), captures
//! and deterministic frames every due step at once, and only while probe GI is settled. A capture's previous bake stays in use
//! until its replacement lands in one prefilter dispatch.
use super::frame::LookFrame;
use crate::graph::{Graph, ResourceId, State};
use crate::instanced;
use anyhow::{Result, ensure};
use ara_gpu::{Access, Buffer, Device, GpuDevice, Image, Queue, Resource, vk};
use glam::{Mat3, Mat4, Quat, Vec3, Vec4};

/// Most captures a renderer keeps (atlas columns; the relight dispatch's width).
pub const MAX_CAPTURES: usize = 16;
/// Prefiltered roughness levels per capture: 0, .25, .5, .75 and 1.
pub(crate) const LEVELS: u32 = 5;
/// Cube faces rendered per bake (+x, -x, +y, -y, +z, -z).
pub(crate) const FACES: u32 = 6;
/// `CaptureRecord` (look-captures.slangh) and the relight state per capture.
const RECORD_BYTES: usize = 128;
/// Per capture in the frame buffer: the record's copy, relight ratio and baseline.
const SLOT_BYTES: usize = 160;
/// 16x16 render tiles the frame buffer holds capture masks for (8192 squared).
const TILE_CAPACITY: usize = 512 * 512;
/// Dev-only shading modes from `ARA_CAPTURE_DEBUG` (read once; look-captures.slangh
/// `CAPTURE_DEBUG_*`): `mirror` shows every surface as a mirror of the captures alone
/// (roughness zero, no probe, sky or screen-space reflection); `only` keeps normal
/// shading but drops screen-space reflections where captures cover; `infinite` drops
/// the box parallax (orientation checks). Combine with commas.
fn debug_modes() -> u32 {
    static MODES: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *MODES.get_or_init(|| {
        let value = std::env::var("ARA_CAPTURE_DEBUG").unwrap_or_default();
        value.split(',').fold(0, |modes, mode| match mode.trim() {
            "mirror" => modes | 1,
            "only" => modes | 2,
            "infinite" => modes | 4,
            _ => modes,
        })
    })
}

/// Frames after a bake before lighting changes may start another (content changes
/// and new specs never wait).
const COOLDOWN: u32 = 60;
/// Live frames a bake waits for streamed textures before baking anyway.
const TEXTURE_WAIT: u32 = 240;

/// One reflection capture in world space, as the renderer bakes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaptureSpec {
    /// Origin of the capture's local frame (metres): the influence box's centre.
    pub position: Vec3,
    /// Orientation of the local frame (world from local); boxes follow its axes.
    pub rotation: Quat,
    /// Influence box half-extents (local axes, metres): surfaces inside reflect
    /// this capture.
    pub influence: Vec3,
    /// Distance inside the influence box over which the capture fades out (metres).
    pub blend: f32,
    /// Projection (parallax) box centre relative to `position` (local axes).
    pub projection_centre: Vec3,
    /// Projection box half-extents (local axes, metres): reflected directions are
    /// looked up where they leave it, as seen from the capture point.
    pub projection: Vec3,
    /// Capture point relative to `position` (local axes); keep it in open space.
    pub capture_offset: Vec3,
    /// Where captures overlap, higher priorities blend first (then smaller boxes).
    pub priority: i32,
    /// Level-0 interior texels per side of the octahedral map (a power of two,
    /// 64 to 1024); zero uses the quality's.
    pub resolution: u32,
    /// Radiance multiplier.
    pub intensity: f32,
    /// Stable identity of an authored probe (its entity's render ID); zero for
    /// automatic placements.
    pub key: u64,
    /// Bakes only when requested ([`crate::Renderer::request_reflection_capture`]) or
    /// restored from a cache ([`crate::Renderer::load_reflection_capture`]), never on
    /// content or lighting changes: an authored probe's image then always matches a
    /// receipt, and goes stale instead of changing under it. Automatic placements bake
    /// themselves.
    pub explicit: bool,
}

impl CaptureSpec {
    /// World position the faces are rendered from.
    pub fn capture_point(&self) -> Vec3 {
        self.position + self.rotation * self.capture_offset
    }

    /// Rejects values the shaders cannot use.
    pub fn validate(&self) -> Result<()> {
        let finite = [
            self.position,
            self.influence,
            self.projection_centre,
            self.projection,
            self.capture_offset,
        ]
        .iter()
        .all(|v| v.is_finite());
        ensure!(
            finite
                && self.rotation.is_finite()
                && self.rotation.length() > 0.5
                && self.influence.min_element() > 0.
                && self.projection.min_element() > 0.
                && self.blend.is_finite()
                && self.blend >= 0.
                && (self.resolution == 0
                    || (self.resolution.is_power_of_two()
                        && (64..=1024).contains(&self.resolution)))
                && self.intensity.is_finite()
                && self.intensity >= 0.,
            "Invalid reflection capture"
        );
        Ok(())
    }

    /// Whether the influence box may meet the frustum of `view_projection`: false
    /// only when all its corners lie beyond one clip plane (sides or behind the eye).
    fn in_view(&self, view_projection: Mat4) -> bool {
        let corners = (0..8).map(|i| {
            let sign = Vec3::new(
                if i & 1 == 0 { -1. } else { 1. },
                if i & 2 == 0 { -1. } else { 1. },
                if i & 4 == 0 { -1. } else { 1. },
            );
            view_projection * (self.position + self.rotation * (self.influence * sign)).extend(1.)
        });
        let mut outside = [true; 5];
        for c in corners {
            let planes: [fn(Vec4) -> bool; 5] = [
                |c| c.x < -c.w,
                |c| c.x > c.w,
                |c| c.y < -c.w,
                |c| c.y > c.w,
                |c| c.w <= 0.,
            ];
            for (out, plane) in outside.iter_mut().zip(planes) {
                *out &= plane(c);
            }
        }
        !outside.contains(&true)
    }

    fn volume(&self) -> f32 {
        self.influence.x * self.influence.y * self.influence.z
    }

    /// The shader record (look-captures.slangh `CaptureRecord`) for atlas column
    /// `column` (texel origin) with `resolution` level-0 interior texels.
    fn record(&self, column: [u32; 2], resolution: u32) -> [u8; RECORD_BYTES] {
        let rotation = Mat3::from_quat(self.rotation.normalize());
        let mut floats = [0f32; 28];
        // World to local: the transpose's rows are the rotation's columns.
        for i in 0..3 {
            let axis = rotation.col(i);
            floats[i * 4..i * 4 + 3].copy_from_slice(&axis.to_array());
            floats[i * 4 + 3] = -axis.dot(self.position);
        }
        floats[12..15].copy_from_slice(&self.influence.to_array());
        floats[15] = self.blend;
        floats[16..19].copy_from_slice(&self.projection_centre.to_array());
        floats[20..23].copy_from_slice(&self.projection.to_array());
        floats[23] = self.intensity;
        floats[24..27].copy_from_slice(&self.capture_point().to_array());
        let mut bytes = [0u8; RECORD_BYTES];
        bytes[..112].copy_from_slice(bytemuck::cast_slice(&floats));
        let tile = [column[0], column[1], resolution, 0];
        bytes[112..].copy_from_slice(bytemuck::cast_slice(&tile));
        bytes
    }
}

/// Reflection captures for content without authored ones: one over the static
/// enclosure (up to 200 m across), taken from its most open point near the largest
/// cluster of smooth objects (metals, glossy and clear surfaces), and one more over
/// each other cluster far from that point, where an open point exists near it. Open
/// points keep at least [`OPEN`] metres from every object (and from walls and
/// ceilings), so no neighbour fills a capture's view. All project onto the
/// enclosure's box, its floor at the widest thin slab's top. At most `limit`.
pub fn auto_place(scene: &instanced::Scene, limit: usize) -> Vec<CaptureSpec> {
    let boxes: Vec<_> = scene
        .instances
        .iter()
        .filter_map(|instance| {
            let bounds = instance.bounds(scene.geometry.get(instance.geometry)?);
            let material = scene.materials.get(instance.material)?;
            let (roughness, metal) = (material.pbr.emissive_roughness[3], material.pbr.surface[0]);
            let smooth =
                roughness <= 0.4 || metal >= 0.5 || material.extras.transmission_factor > 0.;
            Some((bounds, smooth))
        })
        .collect();
    place(&boxes, limit)
}

fn union(a: [Vec3; 2], b: [Vec3; 2]) -> [Vec3; 2] {
    [a[0].min(b[0]), a[1].max(b[1])]
}

/// Clearance (m) that makes a capture point open: nearer geometry would fill its view.
const OPEN: f32 = 1.5;
/// Capture heights above the floor that candidates sample (eye level first).
const HEIGHTS: [f32; 4] = [1.6, 1.2, 2.0, 2.5];

/// Distance from `p` to box `b` (zero inside).
fn outside(b: &[Vec3; 2], p: Vec3) -> f32 {
    (b[0] - p).max(p - b[1]).max(Vec3::ZERO).length()
}

/// [`auto_place`] over instance bounds and whether each is smooth.
fn place(boxes: &[([Vec3; 2], bool)], limit: usize) -> Vec<CaptureSpec> {
    let valid: Vec<_> = boxes
        .iter()
        .copied()
        .filter(|(b, _)| b[0].is_finite() && b[1].is_finite() && b[0].cmple(b[1]).all())
        .collect();
    let Some(enclosure) = valid.iter().map(|(b, _)| *b).reduce(union) else {
        return Vec::new();
    };
    let size = enclosure[1] - enclosure[0];
    let footprint = size.x.max(0.01) * size.z.max(0.01);
    let area = |b: &[Vec3; 2]| (b[1].x - b[0].x) * (b[1].z - b[0].z);
    // The floor: the widest thin slab spanning a quarter of the footprint.
    let slab = valid
        .iter()
        .map(|(b, _)| *b)
        .filter(|b| {
            let s = b[1] - b[0];
            area(b) >= footprint * 0.25 && s.y <= 0.1 * s.x.min(s.z)
        })
        .max_by(|a, b| area(a).total_cmp(&area(b)));
    let floor = slab.map_or(enclosure[0].y, |b| b[1].y);
    // Above the highest capture point: open scenes project the sky high enough.
    let top = enclosure[1].y.max(floor + HEIGHTS[3] + 0.5);
    // Clearance from geometry: objects by their bounds; shells spanning the enclosure
    // (rooms, terrain) by their faces from inside. The floor slab is the projection
    // box's own floor and costs nothing. Very large scenes score their largest 2048.
    let mut solids: Vec<([Vec3; 2], bool)> = valid
        .iter()
        .filter(|(b, _)| Some(*b) != slab)
        .map(|(b, _)| (*b, area(b) >= footprint * 0.25))
        .collect();
    let volume = |b: &[Vec3; 2]| (b[1] - b[0]).max(Vec3::splat(0.01)).element_product();
    solids.sort_by(|a, b| volume(&b.0).total_cmp(&volume(&a.0)));
    solids.truncate(2048);
    let clearance = |p: Vec3| {
        let mut nearest = OPEN;
        for (b, shell) in &solids {
            let inside = p.cmpge(b[0]).all() && p.cmple(b[1]).all();
            let d = if inside && *shell {
                (p - b[0]).min(b[1] - p).min_element()
            } else {
                outside(b, p)
            };
            nearest = nearest.min(d);
        }
        (p.y - floor).min(nearest).max(0.)
    };
    // The most open point of a horizontal region (clearance capped at OPEN), nearest
    // `anchor` and eye level among equals.
    let open = |region: [Vec3; 2], anchor: Vec3| {
        let mut best: Option<(f32, f32, Vec3)> = None;
        for height in HEIGHTS {
            let y = floor + height;
            for i in 0..=16 {
                for k in 0..=16 {
                    let p = Vec3::new(
                        region[0].x + (region[1].x - region[0].x) * i as f32 / 16.,
                        y,
                        region[0].z + (region[1].z - region[0].z) * k as f32 / 16.,
                    );
                    let score = clearance(p);
                    let away = Vec3::new(p.x - anchor.x, 0., p.z - anchor.z).length()
                        + 0.5 * (height - HEIGHTS[0]).abs();
                    let better = best
                        .is_none_or(|(s, a, _)| score > s + 1e-3 || (score > s - 1e-3 && away < a));
                    if better {
                        best = Some((score, away, p));
                    }
                }
            }
        }
        best.map(|(score, _, p)| (score, p))
    };
    let projection = [
        Vec3::new(enclosure[0].x, floor, enclosure[0].z),
        Vec3::new(enclosure[1].x, top, enclosure[1].z),
    ];
    let spec = |influence: [Vec3; 2], blend: f32, point: Vec3, priority: i32| {
        let position = (influence[0] + influence[1]) * 0.5;
        CaptureSpec {
            position,
            rotation: Quat::IDENTITY,
            influence: (influence[1] - influence[0]) * 0.5,
            blend,
            projection_centre: (projection[0] + projection[1]) * 0.5 - position,
            projection: ((projection[1] - projection[0]) * 0.5).max(Vec3::splat(0.05)),
            capture_offset: point - position,
            priority,
            resolution: 0,
            intensity: 1.,
            key: 0,
            explicit: false,
        }
    };
    let objects: Vec<_> = valid
        .iter()
        .filter(|(b, _)| area(b) < footprint * 0.25)
        .copied()
        .collect();
    let smooth: Vec<_> = objects
        .iter()
        .filter(|(_, smooth)| *smooth)
        .map(|(b, _)| *b)
        .collect();
    let centre = |b: &[Vec3; 2]| (b[0] + b[1]) * 0.5;
    let around = |b: [Vec3; 2], margin: f32| {
        [
            (b[0] - margin).max(enclosure[0]),
            (b[1] + margin).min(enclosure[1]),
        ]
    };
    let mut clusters = cluster(&smooth, 1.5, 12.);
    clusters.sort_by(|a, b| area(b).total_cmp(&area(a)));
    // Where reflections matter most: around the largest smooth cluster, else around
    // the objects, else the enclosure's centre.
    let (region, anchor) = match clusters.first() {
        Some(c) => (around(*c, 4.), centre(c)),
        None => {
            let content = objects
                .iter()
                .map(|(b, _)| *b)
                .reduce(union)
                .unwrap_or(enclosure);
            (around(content, 2.), centre(&content))
        }
    };
    let mut specs = Vec::new();
    let mut main = None;
    if size.x.max(size.z) <= 200.
        && limit > 0
        && let Some((_, point)) = open(region, anchor)
    {
        let blend = (0.1 * size.x.min(size.z)).clamp(0.5, 4.);
        specs.push(spec(
            [enclosure[0] - 0.5, enclosure[1] + 0.5],
            blend,
            point,
            0,
        ));
        main = Some(point);
    }
    // Smooth clusters far from the enclosure's capture get their own, where open.
    for c in clusters {
        if specs.len() >= limit {
            break;
        }
        let middle = centre(&c);
        if main.is_some_and(|p: Vec3| Vec3::new(p.x - middle.x, 0., p.z - middle.z).length() < 6.) {
            continue;
        }
        let Some((score, point)) = open(around(c, 3.), middle) else {
            continue;
        };
        if score < OPEN - 1e-3 {
            continue;
        }
        let extent = c[1] - c[0];
        let margin = (0.35 * extent.x.max(extent.z)).clamp(1., 4.);
        let influence = [
            Vec3::new(
                c[0].x - margin,
                (c[0].y - margin).max(floor - 0.1),
                c[0].z - margin,
            ),
            c[1] + margin,
        ];
        specs.push(spec(influence, margin * 0.6, point, 1));
    }
    specs
}

/// Groups boxes whose gap is at most `gap` metres while a group stays within
/// `extent` metres horizontally (boxes first bin into `extent / 3` cells).
fn cluster(boxes: &[[Vec3; 2]], gap: f32, extent: f32) -> Vec<[Vec3; 2]> {
    let cell = extent / 3.;
    let mut cells: Vec<((i32, i32), [Vec3; 2], usize)> = Vec::new();
    for b in boxes {
        let centre = (b[0] + b[1]) * 0.5;
        let key = (
            (centre.x / cell).floor() as i32,
            (centre.z / cell).floor() as i32,
        );
        match cells.iter_mut().find(|(k, _, _)| *k == key) {
            Some((_, bounds, count)) => {
                *bounds = union(*bounds, *b);
                *count += 1;
            }
            None => cells.push((key, *b, 1)),
        }
    }
    // Keep the busiest cells of very large scenes (the merge below is cubic).
    cells.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    cells.truncate(256);
    let mut groups: Vec<[Vec3; 2]> = cells.into_iter().map(|(_, b, _)| b).collect();
    loop {
        let mut merged = None;
        'search: for i in 0..groups.len() {
            for j in i + 1..groups.len() {
                let (a, b) = (groups[i], groups[j]);
                let separation = (a[0] - b[1]).max(b[0] - a[1]).max(Vec3::ZERO);
                let u = union(a, b);
                if separation.length() <= gap && (u[1].x - u[0].x).max(u[1].z - u[0].z) <= extent {
                    merged = Some((i, j, u));
                    break 'search;
                }
            }
        }
        let Some((i, j, u)) = merged else {
            return groups;
        };
        groups[i] = u;
        groups.remove(j);
    }
}

/// First row of roughness level `k`'s bordered tile in a column whose level 0 has
/// `n` interior texels (look-captures.slangh `captureLevelY`).
pub(crate) fn level_y(n: u32, k: u32) -> u32 {
    2 * n - ((2 * n) >> k) + 2 * k
}

/// Atlas columns: one per record (x, level-0 interior texels), then the bake's
/// scratch column, sized for the largest.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Layout {
    pub columns: Vec<(u32, u32)>,
    pub scratch: u32,
    pub size: [u32; 2],
}

impl Layout {
    fn new(resolutions: &[u32]) -> Self {
        let mut x = 0;
        let columns = resolutions
            .iter()
            .map(|&n| {
                let column = (x, n);
                x += n + 2;
                column
            })
            .collect();
        let largest = resolutions.iter().copied().max().unwrap_or(64);
        Self {
            columns,
            scratch: x,
            size: [x + largest + 2, level_y(largest, LEVELS)],
        }
    }
}

/// What the baked captures' lighting depends on (`Captures::plan`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Lighting {
    /// The published content generation (`Content::base_hash`).
    pub content: String,
    /// Direction towards the sun and its illuminance (lux).
    pub sun: [f32; 6],
    /// The authored sky and fog (grading and lens effects come after the bake).
    pub look: (Option<super::Sky>, Option<super::Fog>),
    /// Hash of the local lights.
    pub lights: u64,
    /// The settled probe GI pool: twice its probes, plus one once its finer
    /// lattices landed (zero: none).
    pub gi: u32,
}

impl Lighting {
    /// Whether light changed enough for a re-bake: the sun turned by half a degree
    /// or its illuminance changed by 2% (the relight ratio covers the rest), or the
    /// sky, fog, local lights or probe pool changed.
    fn differs(&self, other: &Self) -> bool {
        let [a, b] = [self, other].map(|l| Vec3::new(l.sun[0], l.sun[1], l.sun[2]));
        let [x, y] = [self, other].map(|l| Vec3::new(l.sun[3], l.sun[4], l.sun[5]).length());
        a.normalize_or_zero().dot(b.normalize_or_zero()) < 0.99996
            || (x - y).abs() > 0.02 * x.max(y)
            || self.look != other.look
            || self.lights != other.lights
            || self.gi != other.gi
    }
}

/// One bake step (`Captures::plan`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Work {
    /// Render face `face` of record `capture` from `position` (a square view of
    /// `resolution` pixels) and project it into the scratch column.
    Face {
        capture: usize,
        face: u32,
        position: Vec3,
        resolution: u32,
    },
    /// Reduce the scratch column and prefilter it into the record's column.
    Finish { capture: usize, resolution: u32 },
}

/// The atlas and buffers of the current layout.
struct CaptureGpu {
    atlas: Image,
    sampled: u32,
    storage: u32,
    atlas_state: State,
    records: Buffer,
    records_index: u32,
    /// Device-local, persistent (look-captures.slangh): record copies (rewritten
    /// when the records change), relight state and the per-tile capture masks
    /// (every frame before lighting).
    frame: Buffer,
    frame_index: u32,
    frame_access: Access,
}

impl CaptureGpu {
    fn new(gpu: &mut GpuDevice, size: [u32; 2]) -> Result<Self> {
        let atlas = gpu.create_image(
            size[0],
            size[1],
            vk::Format::R16G16B16A16_SFLOAT,
            vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::STORAGE
                | vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::TRANSFER_DST,
        )?;
        let sampled = gpu.sampled_image(atlas, false)?.index;
        let storage = gpu.storage_image(atlas)?.index;
        let records = gpu.create_buffer(
            (MAX_CAPTURES * RECORD_BYTES) as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )?;
        let records_index = gpu.storage_buffer(records)?.index;
        // Its baselines start undefined: the first relight clears them (`reset`).
        let frame = gpu.create_buffer(
            (MAX_CAPTURES * SLOT_BYTES + TILE_CAPACITY * 4) as u64,
            vk::BufferUsageFlags::STORAGE_BUFFER,
            false,
        )?;
        let frame_index = gpu.storage_buffer(frame)?.index;
        Ok(Self {
            atlas,
            sampled,
            storage,
            atlas_state: State {
                access: Access::Undefined,
                queue: Queue::Graphics,
            },
            records,
            records_index,
            frame,
            frame_index,
            frame_access: Access::Undefined,
        })
    }

    fn destroy(self, gpu: &mut GpuDevice) -> Result<()> {
        gpu.retire(Resource::Image(self.atlas))?;
        gpu.retire(Resource::Buffer(self.records))?;
        gpu.retire(Resource::Buffer(self.frame))?;
        Ok(())
    }
}

/// Graph identities of the captures this frame relights and shading reads.
pub(crate) struct CaptureIds {
    pub atlas: ResourceId,
    pub records: ResourceId,
    pub frame: ResourceId,
}

/// The renderer's reflection captures: specs, their atlas and the bake plan.
#[derive(Default)]
pub(crate) struct Captures {
    /// Specs as the host set them (`Renderer::set_reflection_captures`).
    specs: Vec<CaptureSpec>,
    /// Records: specs in blending order (priority, then smaller first), capped by
    /// the quality.
    records: Vec<CaptureSpec>,
    layout: Option<Layout>,
    gpu: Option<CaptureGpu>,
    /// Per record: needs a (re)bake.
    pending: Vec<bool>,
    /// Per record: a bake of the current atlas landed (shading may read it).
    ready: Vec<bool>,
    /// The record being baked and its next face (live views).
    progress: Option<(usize, u32)>,
    /// What the last requested bakes are lit by.
    lighting: Option<Lighting>,
    /// Records whose bake landed since the last relight pass (bit per record).
    rebaseline: u32,
    /// Records whose baseline the next relight clears (a new atlas, a moved capture).
    reset: u32,
    /// Records changed since the frame buffer's copy: the next relight copies them
    /// (`copy_records`).
    copy: bool,
    cooldown: u32,
    waited: u32,
    /// Bake frames are rendering: their shading must not read the captures.
    pub baking: bool,
    /// A bake failed: captures stay off until new specs arrive.
    failed: bool,
    /// Per record: bakes completed into its column (explicit records' receipts).
    bakes: Vec<u64>,
    /// Explicit records' columns must be restored from retained images (a new atlas,
    /// or another probe moved into a column): `Renderer` uploads them.
    restore: bool,
}

/// A published explicit capture as read back from the atlas
/// (`Renderer::take_reflection_capture_images`, `Renderer::load_reflection_capture`):
/// its column, borders and all five prefiltered levels, row-major RGBA16F.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureImage {
    /// The probe's key ([`CaptureSpec::key`]).
    pub key: u64,
    /// Which of the column's bakes it is (`BakeStatus::bakes`); 0 for a loaded image.
    pub bake: u64,
    /// Level-0 interior texels per side.
    pub resolution: u32,
    /// Texel rows and row width of `bytes` (`resolution + 2` wide).
    pub extent: [u32; 2],
    /// `extent[0] * extent[1]` RGBA16F texels.
    pub bytes: Vec<u8>,
}

impl CaptureImage {
    /// The column extent of a capture of `resolution` texels: width, height.
    pub fn extent_for(resolution: u32) -> [u32; 2] {
        [resolution + 2, level_y(resolution, LEVELS)]
    }
}

/// One record's bake state (`Captures::status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BakeStatus {
    /// A bake is requested and not yet published.
    pub pending: bool,
    /// Steps done while this record is the one baking (0-5 faces rendered; 6: the
    /// faces are done and the reduce and prefilter publish next); `None` while it
    /// waits its turn or nothing is pending.
    pub step: Option<u32>,
    /// Bake steps per capture (six faces and the prefilter).
    pub steps: u32,
    /// The column holds a published image shading reads.
    pub ready: bool,
    /// Bakes published into the column so far.
    pub bakes: u64,
    /// Level-0 texels per side of the column.
    pub resolution: u32,
}

/// Probe GI and texture residency are settled enough to bake (`Captures::plan`).
pub(crate) struct Settled {
    pub gi: bool,
    pub textures: bool,
}

impl Captures {
    /// Replaces the specs (validated); unchanged specs keep every bake.
    pub(crate) fn set_specs(&mut self, specs: &[CaptureSpec]) -> Result<()> {
        for spec in specs {
            spec.validate()?;
        }
        if self.specs != specs {
            self.specs = specs.to_vec();
            self.failed = false;
        }
        Ok(())
    }

    /// Stops baking and reading captures after a failed bake, until new specs.
    pub(crate) fn fail(&mut self) {
        self.failed = true;
        self.progress = None;
    }

    /// Fits the atlas to the specs and quality, notes what invalidates bakes and
    /// returns this frame's bake work: one step (a face or a finish) in live frames,
    /// every due step when `sync` (captures, deterministic frames). Nothing bakes
    /// while probe GI converges, nor in live frames while textures stream (for at
    /// most `TEXTURE_WAIT` frames).
    pub(crate) fn plan(
        &mut self,
        gpu: &mut GpuDevice,
        quality: Option<super::ReflectionCaptures>,
        lighting: Lighting,
        settled: Settled,
        sync: bool,
    ) -> Result<Vec<Work>> {
        let Some(quality) = quality else {
            return Ok(Vec::new());
        };
        if self.failed || self.specs.is_empty() {
            return Ok(Vec::new());
        }
        let mut records = self.specs.clone();
        records.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then(a.volume().total_cmp(&b.volume()))
        });
        records.truncate((quality.captures as usize).min(MAX_CAPTURES));
        let resolutions: Vec<u32> = records
            .iter()
            .map(|r| match r.resolution {
                0 => quality.resolution,
                n => n,
            })
            .collect();
        let layout = Layout::new(&resolutions);
        if self.layout.as_ref() != Some(&layout) {
            if let Some(old) = self.gpu.take() {
                old.destroy(gpu)?;
            }
            self.layout = None;
            self.gpu = Some(CaptureGpu::new(gpu, layout.size)?);
            self.pending = records.iter().map(|r| !r.explicit).collect();
            self.ready = vec![false; records.len()];
            self.bakes = vec![0; records.len()];
            self.restore = records.iter().any(|r| r.explicit);
            self.progress = None;
            self.rebaseline = 0;
            self.reset = u32::MAX;
            self.records = records;
            self.layout = Some(layout);
            self.upload_records(gpu)?;
        } else if self.records != records {
            for (i, (old, new)) in self.records.iter().zip(&records).enumerate() {
                if old == new {
                    continue;
                }
                if new.explicit {
                    // An explicit probe keeps its image (stale) until a requested bake
                    // replaces it; another probe moving into the column restores its own.
                    if old.key != new.key {
                        self.ready[i] = false;
                        self.pending[i] = false;
                        self.bakes[i] = 0;
                        self.restore = true;
                        if self.progress.is_some_and(|(c, _)| c == i) {
                            self.progress = None;
                        }
                    }
                    continue;
                }
                self.pending[i] = true;
                if self.progress.is_some_and(|(c, _)| c == i) {
                    self.progress = None;
                }
                // A capture taken elsewhere is wrong until it is baked again.
                if old.capture_point() != new.capture_point() {
                    self.ready[i] = false;
                    self.reset |= 1 << i;
                }
            }
            self.records = records;
            self.upload_records(gpu)?;
        }
        // A converging probe volume (a capture reset, say) keeps the pool it had.
        let mut lighting = lighting;
        if !settled.gi
            && let Some(old) = &self.lighting
        {
            lighting.gi = old.gi;
        }
        let rebake = match &self.lighting {
            None => true,
            Some(old) if old.content != lighting.content => true,
            Some(old) => old.differs(&lighting) && (self.cooldown == 0 || sync),
        };
        if rebake {
            for (pending, record) in self.pending.iter_mut().zip(&self.records) {
                *pending |= !record.explicit;
            }
            if self
                .progress
                .is_some_and(|(c, _)| !self.records.get(c).is_some_and(|r| r.explicit))
            {
                self.progress = None;
            }
            self.lighting = Some(lighting);
        }
        self.cooldown = self.cooldown.saturating_sub(1);
        if !self.pending.contains(&true) {
            self.waited = 0;
            return Ok(Vec::new());
        }
        if !settled.gi {
            // Faces lit before and after a change of probe light would not match.
            self.progress = None;
            return Ok(Vec::new());
        }
        if !settled.textures && !sync && self.waited < TEXTURE_WAIT {
            self.waited += 1;
            return Ok(Vec::new());
        }
        Ok(self.schedule(sync))
    }

    /// The due faces (and finishes) from the current progress.
    fn schedule(&mut self, sync: bool) -> Vec<Work> {
        let Some(layout) = &self.layout else {
            return Vec::new();
        };
        let due: Vec<usize> = (0..self.records.len())
            .filter(|&i| self.pending[i])
            .collect();
        let mut cursor = self
            .progress
            .filter(|(c, _)| self.pending.get(*c).copied().unwrap_or(false))
            .or_else(|| due.first().map(|&c| (c, 0)));
        // Steps 0..FACES render faces; step FACES reduces and prefilters, in a live
        // frame of its own.
        let mut work = Vec::new();
        while let Some((capture, step)) = cursor {
            let resolution = layout.columns[capture].1;
            if step < FACES {
                work.push(Work::Face {
                    capture,
                    face: step,
                    position: self.records[capture].capture_point(),
                    resolution,
                });
                cursor = Some((capture, step + 1));
            } else {
                work.push(Work::Finish {
                    capture,
                    resolution,
                });
                cursor = due.iter().copied().find(|&c| c > capture).map(|c| (c, 0));
            }
            if !sync {
                break;
            }
        }
        self.progress = if sync { None } else { cursor };
        work
    }

    /// A capture's prefilter landed: shading reads it from the next relight on.
    pub(crate) fn finished(&mut self, capture: usize) {
        if let Some(pending) = self.pending.get_mut(capture) {
            *pending = false;
            self.ready[capture] = true;
            self.bakes[capture] += 1;
            self.rebaseline |= 1 << capture;
        }
        if !self.pending.contains(&true) {
            self.cooldown = COOLDOWN;
            self.waited = 0;
        }
    }

    /// The record of explicit probe `key`, if the quality holds it.
    fn index(&self, key: u64) -> Option<usize> {
        self.records.iter().position(|r| r.explicit && r.key == key)
    }

    /// Requests a bake of explicit probe `key` (no cooldown); false when the renderer
    /// holds no such record (unknown, or past the quality's capture count).
    pub(crate) fn request(&mut self, key: u64) -> bool {
        let Some(i) = self.index(key) else {
            return false;
        };
        self.pending[i] = true;
        true
    }

    /// Drops a requested, unpublished bake of `key` (its column keeps its image); false
    /// when nothing was pending. A bake publishes within one frame's work, so between
    /// frames every pending bake can still be cancelled.
    pub(crate) fn cancel(&mut self, key: u64) -> bool {
        let Some(i) = self.index(key) else {
            return false;
        };
        let was = std::mem::replace(&mut self.pending[i], false);
        if self.progress.is_some_and(|(c, _)| c == i) {
            self.progress = None;
        }
        was
    }

    /// Explicit probe `key`'s bake state, if the quality holds its record.
    pub(crate) fn status(&self, key: u64) -> Option<BakeStatus> {
        let i = self.index(key)?;
        Some(BakeStatus {
            pending: self.pending[i],
            step: self.progress.filter(|(c, _)| *c == i).map(|(_, step)| step),
            steps: FACES + 1,
            ready: self.ready[i],
            bakes: self.bakes[i],
            resolution: self.layout.as_ref()?.columns.get(i)?.1,
        })
    }

    /// Takes the restore flag set by a new atlas, a moved column or a loaded image. A
    /// restore rewrites the whole atlas, scratch column included, so a bake under way
    /// starts its faces again.
    pub(crate) fn take_restore(&mut self) -> bool {
        let restore = std::mem::take(&mut self.restore) && self.gpu.is_some();
        if restore {
            self.progress = None;
        }
        restore
    }

    /// Asks for a restore of the explicit columns (an image was loaded).
    pub(crate) fn request_restore(&mut self) {
        self.restore = true;
    }

    /// Whether record `capture` is explicit.
    pub(crate) fn is_explicit(&self, capture: usize) -> bool {
        self.records.get(capture).is_some_and(|r| r.explicit)
    }

    /// Record `capture`'s key and bakes published so far.
    pub(crate) fn key_and_bakes(&self, capture: usize) -> Option<(u64, u64)> {
        Some((self.records.get(capture)?.key, *self.bakes.get(capture)?))
    }

    /// The explicit records and their columns (`key`, x, resolution) for a restore.
    pub(crate) fn explicit_columns(&self) -> Vec<(usize, u64, u32, u32)> {
        let Some(layout) = &self.layout else {
            return Vec::new();
        };
        self.records
            .iter()
            .zip(&layout.columns)
            .enumerate()
            .filter(|(_, (r, _))| r.explicit)
            .map(|(i, (r, &(x, n)))| (i, r.key, x, n))
            .collect()
    }

    /// Marks record `capture` as holding a restored image shading may read.
    pub(crate) fn restored(&mut self, capture: usize) {
        if let Some(ready) = self.ready.get_mut(capture) {
            *ready = true;
            self.rebaseline |= 1 << capture;
        }
    }

    /// The atlas extent (texels).
    pub(crate) fn atlas_size(&self) -> Option<[u32; 2]> {
        Some(self.layout.as_ref()?.size)
    }

    fn upload_records(&mut self, gpu: &mut GpuDevice) -> Result<()> {
        let (Some(layout), Some(state)) = (&self.layout, &self.gpu) else {
            return Ok(());
        };
        let mut bytes = vec![0u8; MAX_CAPTURES * RECORD_BYTES];
        for (i, (record, &(x, n))) in self.records.iter().zip(&layout.columns).enumerate() {
            bytes[i * RECORD_BYTES..(i + 1) * RECORD_BYTES]
                .copy_from_slice(&record.record([x, 0], n));
        }
        gpu.upload_queued(state.records, &bytes)?;
        self.copy = true;
        Ok(())
    }

    /// Whether shading reads captures this frame (`enabled`: the quality has them).
    fn active(&self, enabled: bool) -> bool {
        enabled && !self.baking && !self.failed && self.gpu.is_some() && self.ready.contains(&true)
    }

    /// Writes this frame's Look rows (zero rows: no captures): shading visits only
    /// the baked captures whose influence box meets the camera's frustum
    /// (`view_projection`).
    pub(crate) fn write(&self, frame: &mut LookFrame, enabled: bool, view_projection: Mat4) {
        let (Some(gpu), Some(layout), true) = (&self.gpu, &self.layout, self.active(enabled))
        else {
            return;
        };
        let visible = self
            .records
            .iter()
            .zip(&self.ready)
            .enumerate()
            .filter(|(_, (record, ready))| **ready && record.in_view(view_projection))
            .fold(0u32, |mask, (i, _)| mask | 1 << i);
        frame.capture_info = [
            [
                gpu.records_index,
                self.records.len() as u32,
                gpu.sampled,
                gpu.frame_index,
            ],
            [layout.size[0], layout.size[1], visible, debug_modes()],
        ];
    }

    /// The relight's root word 2: records whose bake landed since the last relight
    /// (low half; their baseline is measured) and whose baseline it clears (high half).
    pub(crate) fn refit_masks(&self) -> u32 {
        (self.rebaseline & 0xffff) | (self.reset & 0xffff) << 16
    }

    /// Whether this frame's relight first copies the records into the frame buffer
    /// (they changed since its last copy; the copies persist).
    pub(crate) fn copy_records(&self) -> bool {
        self.copy
    }

    /// Imports what shading reads, when it reads captures this frame.
    pub(crate) fn import(&self, graph: &mut Graph, enabled: bool) -> Option<CaptureIds> {
        let gpu = self.gpu.as_ref().filter(|_| self.active(enabled))?;
        let state = |access| State {
            access,
            queue: Queue::Graphics,
        };
        Some(CaptureIds {
            atlas: graph.import(Resource::Image(gpu.atlas), gpu.atlas_state),
            records: graph.import(Resource::Buffer(gpu.records), state(Access::HostWrite)),
            frame: graph.import(Resource::Buffer(gpu.frame), state(gpu.frame_access)),
        })
    }

    /// Records the final states of a frame that read the captures; its relight
    /// took every pending baseline.
    pub(crate) fn finish(&mut self, ids: &CaptureIds, final_states: &[State]) {
        if let Some(gpu) = &mut self.gpu {
            gpu.atlas_state = final_states[ids.atlas.0];
            gpu.frame_access = final_states[ids.frame.0].access;
            self.rebaseline = 0;
            self.reset = 0;
            self.copy = false;
        }
    }

    /// The atlas a bake writes: image, storage index, state and the scratch
    /// column's x.
    pub(crate) fn atlas(&self) -> Option<(Image, u32, State, u32)> {
        let gpu = self.gpu.as_ref()?;
        Some((
            gpu.atlas,
            gpu.storage,
            gpu.atlas_state,
            self.layout.as_ref()?.scratch,
        ))
    }

    /// A bake graph left the atlas in `state`.
    pub(crate) fn set_atlas_state(&mut self, state: State) {
        if let Some(gpu) = &mut self.gpu {
            gpu.atlas_state = state;
        }
    }

    /// Atlas column x of record `capture`.
    pub(crate) fn column(&self, capture: usize) -> Option<u32> {
        Some(self.layout.as_ref()?.columns.get(capture)?.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slab(lo: [f32; 3], hi: [f32; 3], smooth: bool) -> ([Vec3; 2], bool) {
        ([Vec3::from_array(lo), Vec3::from_array(hi)], smooth)
    }

    #[test]
    fn levels_stack_bordered_halving_tiles() {
        assert_eq!(level_y(256, 0), 0);
        assert_eq!(level_y(256, 1), 258);
        assert_eq!(level_y(256, 2), 258 + 130);
        assert_eq!(level_y(256, 4), 258 + 130 + 66 + 34);
        assert_eq!(level_y(256, LEVELS), 258 + 130 + 66 + 34 + 18);
        let layout = Layout::new(&[256, 128]);
        assert_eq!(layout.columns, vec![(0, 256), (258, 128)]);
        assert_eq!(layout.scratch, 388);
        assert_eq!(layout.size, [388 + 258, level_y(256, LEVELS)]);
    }

    #[test]
    fn records_map_world_into_the_rotated_frame() {
        let spec = CaptureSpec {
            position: Vec3::new(1., 2., 3.),
            rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            influence: Vec3::new(4., 2., 1.),
            blend: 0.5,
            projection_centre: Vec3::ZERO,
            projection: Vec3::new(4., 2., 1.),
            capture_offset: Vec3::new(0., 1., 0.),
            priority: 0,
            resolution: 0,
            intensity: 1.,
            key: 0,
            explicit: false,
        };
        let bytes = spec.record([10, 0], 128);
        let words = bytes.as_chunks::<4>().0;
        let floats: Vec<f32> = words.iter().map(|w| f32::from_le_bytes(*w)).collect();
        // Local x of the entity points along world -z: world (1, 2, 2) is local x = 1.
        let world = Vec3::new(1., 2., 2.);
        let local: Vec<f32> = (0..3)
            .map(|i| Vec3::from_slice(&floats[i * 4..i * 4 + 3]).dot(world) + floats[i * 4 + 3])
            .collect();
        assert!((local[0] - 1.).abs() < 1e-5 && local[1].abs() < 1e-5 && local[2].abs() < 1e-5);
        assert_eq!(&floats[24..27], &[1., 3., 3.]);
        let tile: Vec<u32> = words[28..].iter().map(|w| u32::from_le_bytes(*w)).collect();
        assert_eq!(tile, [10, 0, 128, 0]);
    }

    /// A stage like the Showcase: a wide floor, a figure at its centre and a grid of
    /// smooth spheres off to one side.
    fn stage() -> Vec<([Vec3; 2], bool)> {
        let mut boxes = vec![
            slab([-15., -0.2, -15.], [15., 0.12, 15.], false),
            slab([-0.97, 0.12, -0.16], [0.97, 1.95, 0.21], false),
        ];
        for row in 0..3 {
            for column in 0..6 {
                let (x, z) = (-2.25 + column as f32 * 0.9, 3. + row as f32 * 0.9);
                boxes.push(slab([x - 0.3, 0., z - 0.3], [x + 0.3, 0.6, z + 0.3], true));
            }
        }
        boxes
    }

    #[test]
    fn the_stage_is_captured_from_open_space_near_its_smooth_objects() {
        let boxes = stage();
        let specs = place(&boxes, 8);
        assert_eq!(specs.len(), 1, "{specs:?}");
        let spec = specs[0];
        spec.validate().expect("valid automatic capture");
        // Nothing within the clearance, eye level or above, near the spheres.
        let point = spec.capture_point();
        for (b, _) in &boxes[1..] {
            assert!(outside(b, point) >= OPEN - 1e-3, "{point} near {b:?}");
        }
        assert!(point.y >= 0.12 + 1.2 - 1e-3, "{point}");
        assert!(point.x.abs() < 2. && (point.z - 3.9).abs() < 2., "{point}");
        // Projected onto the floor's top.
        let floor = spec.position.y + spec.projection_centre.y - spec.projection.y;
        assert!((floor - 0.12).abs() < 1e-4, "{floor}");
    }

    #[test]
    fn far_smooth_clusters_get_their_own_open_capture() {
        let mut boxes = stage();
        boxes[0] = slab([-40., -0.2, -40.], [40., 0.12, 40.], false);
        boxes.push(slab([29., 0., 29.], [31., 1., 31.], true));
        let specs = place(&boxes, 8);
        assert_eq!(specs.len(), 2, "{specs:?}");
        let far = specs[1];
        assert_eq!(far.priority, 1);
        let point = far.capture_point();
        assert!(
            (point - Vec3::new(30., point.y, 30.)).length() < 4.,
            "{point}"
        );
        for (b, _) in &boxes[1..] {
            assert!(outside(b, point) >= OPEN - 1e-3, "{point} near {b:?}");
        }
    }

    #[test]
    fn huge_worlds_only_capture_their_smooth_clusters() {
        let boxes = [
            slab([-2000., -10., -2000.], [2000., 0., 2000.], false),
            slab([4., 0., 4.], [5., 1., 5.], true),
        ];
        let specs = place(&boxes, 4);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].priority, 1);
        assert!(place(&[], 4).is_empty());
    }

    #[test]
    fn shading_visits_only_captures_in_the_frustum() {
        let view = Mat4::perspective_infinite_reverse_rh(1., 1., 0.1)
            * Mat4::look_at_rh(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y);
        let at = |z: f32| CaptureSpec {
            position: Vec3::new(0., 0., z),
            ..place(&[slab([-1., 0., -1.], [1., 1., 1.], true)], 1)[0]
        };
        assert!(at(-10.).in_view(view));
        assert!(!at(10.).in_view(view));
        // A box around the eye always meets the frustum.
        assert!(at(0.).in_view(view));
    }

    #[test]
    fn lighting_changes_need_a_turn_or_a_new_look() {
        let base = Lighting {
            content: "a".into(),
            sun: [0., 1., 0., 1000., 1000., 1000.],
            look: (None, None),
            lights: 1,
            gi: 0,
        };
        let mut dimmer = base.clone();
        dimmer.sun[3..].copy_from_slice(&[990.; 3]);
        assert!(!base.differs(&dimmer));
        let mut turned = base.clone();
        turned.sun[..3].copy_from_slice(&[0.1, 1., 0.]);
        assert!(base.differs(&turned));
        let mut gi = base.clone();
        gi.gi = 4096;
        assert!(base.differs(&gi));
    }

    #[test]
    fn live_bakes_take_one_face_per_frame_and_sync_bakes_take_all() {
        let mut captures = Captures {
            records: vec![place(&[slab([-1., 0., -1.], [1., 1., 1.], true)], 1)[0]; 2],
            layout: Some(Layout::new(&[64, 64])),
            pending: vec![true, true],
            ready: vec![false, false],
            bakes: vec![0, 0],
            ..Default::default()
        };
        let first = captures.schedule(false);
        assert!(matches!(
            first[..],
            [Work::Face {
                capture: 0,
                face: 0,
                ..
            }]
        ));
        for face in 1..FACES {
            let work = captures.schedule(false);
            assert!(matches!(work[..], [Work::Face { capture: 0, face: f, .. }] if f == face));
        }
        let finish = captures.schedule(false);
        assert!(matches!(finish[..], [Work::Finish { capture: 0, .. }]));
        assert_eq!(captures.progress, Some((1, 0)));
        captures.finished(0);
        assert_eq!(captures.refit_masks() & 0xffff, 1);
        let all = captures.schedule(true);
        assert_eq!(all.len(), FACES as usize + 1);
        assert!(matches!(all.last(), Some(Work::Finish { capture: 1, .. })));
    }
}
