//! The render-v3 sun page service (council-v3 3.1, 3.5, D10; gates G2 and G4): an
//! opt-in mode of [`super::VirtualShadowMap`] that keeps sun pages valid while the sun
//! moves. Off by default; today's virtual shadow maps stay the A/B control and the
//! default image is unchanged.
//!
//! - **Pages** are fixed squares of the world's ground grid ([`oblique`]): their texels
//!   hold the height of the first surface along the sun's rays, drawn under the shear
//!   of the sun at draw time and read with that page's own shear. A moving sun does not
//!   re-key the cache (today's map re-keys every page on any sun change).
//! - **Invalidation** ([`bound`]): a page's lag bound is
//!   `(caster height above its lowest receiver) x |k_now - k_drawn|` in its texels, exact
//!   for a caster-receiver pair at every elevation and azimuth (`h delta-alpha / sin^2
//!   alpha` plus azimuth); cross-page reads are moved into the reading page's frame, so
//!   no seam term. Caster tops come from instance boxes; receiver heights from `VSM.Mark`
//!   (every lit pixel within 16 frames) where available, else instance boxes.
//! - **Demand** comes from the pixels that sample pages (`shadow-pages.slang`
//!   `sunPagesMarkMain`); reflection hits, patches and froxels can mark the same
//!   request buffer later.
//! - **Scheduling** ([`schedule`]): pages become demand at `trigger x line`; cold pages
//!   first, then earliest deadline; static and dynamic layers under separate caps;
//!   excess queues as debt with per-page lag in texels ([`schedule::Counters`]).
//! - **Dynamic layer**: pages a moved caster's old-and-new box touches since their draw.
//!   The renderer has no per-instance displacement here, so such a page's lag is
//!   unbounded until it is redrawn (reported as `schedule::UNBOUNDED` if deferred).
//! - **Lookups** (`sunPagesProjectMain`): source-shaped soft shadows by per-texel
//!   back-projection onto a fixed 16x16 grid of sun-disc samples (a 256-bit mask per
//!   pixel): every blocker texel at its own depth occludes its own samples, so multiple
//!   blocker depths and static x mover occluders combine per source sample (one atlas
//!   holds both). Rings of 8x8 texels at the pixel's level and every second coarser
//!   level, as far as the page's blockers spread (up to eight rings), keep contact texels
//!   screen-matched while reaching the full penumbra of tall casters at low sun. No
//!   contact ray. Also: the dynamic layer's rigid movers carry their pre-move transform
//!   (`rigid_displacement`), and receiver feedback from each pixel's own level propagates
//!   to its ring pages.
mod bound;
mod oblique;
mod schedule;

pub use schedule::Counters;

use super::{
    Casters, LightSpace, PageDraw, Requested, VirtualShadowMap, key, page_step, table_index,
};
use anyhow::{Context, Result, ensure};
use ara_gpu::{Buffer, Device, GpuDevice, Pipeline, Resource};
use ara_vsm::{PAGE_SIDE, PAGE_SIZE, PageKey, Projection};
use bound::{Heights, PageHeights};
use glam::{IVec2, Mat4, Vec2, Vec3, Vec4};
use schedule::{Budget, Demand, Layer};
use std::collections::BTreeMap;
use std::path::Path;

/// Settings of the opt-in sun page service ([`crate::Renderer::set_sun_pages`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SunPages {
    /// Runtime cap of the static sun pages and near ring, GPU ms per frame (council-v3
    /// section 5: 1.0 on the 760M, 0.15 on the RTX 4070 SUPER); `None`: every page of
    /// demand draws each frame (the uncapped replay that scores cost).
    pub static_cap_ms: Option<f32>,
    /// Runtime cap of the dynamic layer (movers, wind, skinned casters); `None` as above.
    pub dynamic_cap_ms: Option<f32>,
    /// Edge-lag line outside the near ring, in texels of the page (1.0).
    pub lag_line: f32,
    /// Edge-lag line of the near ring (0.5).
    pub near_lag_line: f32,
    /// Pages (any level) whose ground square comes within this many metres of the
    /// camera's ground position form the near ring.
    pub near_radius: f32,
    /// A page becomes demand at this fraction of its line (0.5) and is due at the line:
    /// the slack between them is what deadline staggering spreads under the cap.
    pub trigger: f32,
    /// World height (m) of the sheared page grid's reference plane (the ground).
    pub reference_height: f32,
}

impl SunPages {
    /// The capped run on the RTX 4070 SUPER (both layers at 0.15 ms).
    pub fn capped_4070s() -> Self {
        Self {
            static_cap_ms: Some(0.15),
            dynamic_cap_ms: Some(0.15),
            ..Self::uncapped()
        }
    }
    /// The capped run on the Radeon 760M (both layers at 1.0 ms).
    pub fn capped_760m() -> Self {
        Self {
            static_cap_ms: Some(1.),
            dynamic_cap_ms: Some(1.),
            ..Self::uncapped()
        }
    }
    /// The uncapped replay: all demand draws every frame.
    pub fn uncapped() -> Self {
        Self {
            static_cap_ms: None,
            dynamic_cap_ms: None,
            lag_line: 1.,
            near_lag_line: 0.5,
            near_radius: 16.,
            trigger: 0.5,
            reference_height: 0.,
        }
    }
}

/// One frame's service counters for the lab (T1's `sun_pages` and `dynamic_pages`
/// stage rows) plus the page traffic behind them.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct SunPageCounters {
    /// The renderer frame these counters describe.
    pub frame: u64,
    /// Static sun pages and near ring.
    pub sun_pages: Counters,
    /// Movers, wind and skinned casters.
    pub dynamic_pages: Counters,
    /// Pages drawn this frame per layer (static, dynamic) after the caster rows fit.
    pub drawn: [u64; 2],
    /// Demanded pages never drawn (their pixels use coarser levels meanwhile).
    pub cold: u64,
    /// Pages resident in the atlas.
    pub resident: u64,
    /// Sun shear `|k|` (the cotangent of the elevation).
    pub shear: f32,
    /// `|k_now - k_last|`: the shear's change since the last frame.
    pub shear_rate: f32,
    /// Whether the elevation was too low for sheared pages (no pages this frame).
    pub inactive: bool,
}

/// Physical page draws per frame the service sizes its buffers for.
pub(crate) const MAX_DRAWS: usize = 4096;
/// Bits plus one receiver word per page-table entry.
pub(crate) const RECEIVER_WORDS: usize = super::LEVELS * (PAGE_SIDE * PAGE_SIDE) as usize;
/// Request buffer words: page bits, then per entry `(frame tag << 16) | (65535 - floor(65535 depth))`
/// of the lowest receiver marking it (an `InterlockedMax` keeps the newest frame's minimum).
pub(crate) const REQUEST_WORDS: usize = super::REQUEST_WORDS + RECEIVER_WORDS;
/// Frames a page stays demand after its last mark (marks are read back irregularly).
const DEMAND_FRAMES: u64 = 4;
/// Frames of receiver feedback before it replaces the box bound (the marks' rotation).
const FEEDBACK_FRAMES: u32 = 16;
/// Light id of the sheared sun in the page cache.
const SUN_PAGES: u64 = 2;

/// The service's pipelines (`shadow-pages.slang`).
#[derive(Clone, Copy)]
pub(crate) struct Pipelines {
    pub mark: Pipeline,
    pub project: Pipeline,
}

/// What a page knows about its last draw.
#[derive(Debug, Clone, Copy)]
struct Drawn {
    k: Vec2,
    heights: Option<PageHeights>,
}

/// Per-page service state, beside the cache's slot assignment.
#[derive(Debug, Clone, Copy)]
struct Record {
    /// The cache assignment (slot and generation) of its current residency.
    page: Option<ara_vsm::DirtyPage>,
    level: u8,
    xy: IVec2,
    drawn: Option<Drawn>,
    /// Lowest receiver height any pixel has marked since the page was first seen (GPU
    /// feedback; marks rotate over 16 frames, so it covers every pixel after 16).
    receivers: Option<f32>,
    /// Frames of receiver feedback received (the box bound applies until 16).
    feedback: u32,
    /// The last frame that fed it receiver feedback.
    fed: u64,
    /// How far (ground metres) moved casters' shadows may have shifted in the page since
    /// its draw: the sum over frames of the largest shift of that frame (infinite where
    /// a change has no displacement bound).
    moved: f32,
    /// This frame's largest shift, and the frame it belongs to.
    shift: (f32, u64),
    /// Last frame a pixel marked it.
    demanded: u64,
    /// First frame of its current demand (debt age).
    since: Option<u64>,
}

/// The service: settings, per-page records, the height index, budgets, counters and
/// its GPU resources (per-slot shear records and its pipelines).
pub(crate) struct Service {
    pub settings: SunPages,
    pub pipelines: Pipelines,
    records: BTreeMap<PageKey, Record>,
    heights: Heights,
    scratch: Vec<u32>,
    shear: Option<Vec2>,
    last_shear: Option<Vec2>,
    budgets: [Budget; 2],
    /// Pages drawn by the previous plan per layer (matched with the timings it
    /// produced, which arrive one frame later).
    last_drawn: [usize; 2],
    /// This frame's planned draws per layer, then what fits (`drawn`).
    pub layers: [usize; 2],
    counters: SunPageCounters,
    /// Per physical slot: shear x, shear y (ground z), top height, 0.
    slots: Buffer,
    pub slots_index: u32,
    slots_mirror: Vec<[f32; 4]>,
    /// The camera's ground position at the last armed frame (the near ring's centre).
    camera: Vec2,
    /// Frame tag of each request slot's receiver words.
    pub tags: [u32; 3],
    tag: u32,
}

impl Service {
    /// The service's GPU resources for an atlas of `pages` slots; pipelines come from
    /// the renderer's cooked cohort `artifacts` (rejecting another cohort's `key`).
    pub(crate) fn new(
        gpu: &mut GpuDevice,
        settings: SunPages,
        pages: u32,
        (artifacts, key): (&Path, &str),
    ) -> Result<Self> {
        ensure!(
            settings.lag_line > 0.
                && settings.near_lag_line > 0.
                && (0.0..=1.0).contains(&settings.trigger)
                && settings.trigger > 0.
                && settings.static_cap_ms.is_none_or(|c| c > 0.)
                && settings.dynamic_cap_ms.is_none_or(|c| c > 0.),
            "sun page settings out of range: {settings:?}"
        );
        let mut load = |name: &str| -> Result<Pipeline> {
            let cooked: ara_gpu::cooked::CookedPipeline =
                serde_json::from_slice(&std::fs::read(artifacts.join(format!("{name}.json")))?)?;
            ensure!(
                cooked.key == key,
                "Sun page cook is incomplete; recook the pipeline manifest"
            );
            Ok(gpu.create_pipeline(&cooked, artifacts)?)
        };
        let pipelines = Pipelines {
            mark: load("shadow_pages_mark")?,
            project: load("shadow_pages_project")?,
        };
        let bytes = pages as usize * 16;
        let slots = gpu.create_buffer(
            bytes as u64,
            ara_gpu::vk::BufferUsageFlags::STORAGE_BUFFER,
            true,
        )?;
        gpu.upload(slots, &vec![0; bytes])?;
        let slots_index = gpu.storage_buffer(slots)?.index;
        Ok(Self {
            settings,
            pipelines,
            records: BTreeMap::new(),
            heights: Heights::default(),
            scratch: Vec::new(),
            shear: None,
            last_shear: None,
            budgets: [
                Budget::new(settings.static_cap_ms),
                Budget::new(settings.dynamic_cap_ms),
            ],
            last_drawn: [0; 2],
            layers: [0; 2],
            counters: SunPageCounters::default(),
            slots,
            slots_index,
            slots_mirror: vec![[0.; 4]; pages as usize],
            camera: Vec2::ZERO,
            tags: [0; 3],
            tag: 0,
        })
    }

    pub(crate) fn destroy(self, gpu: &mut GpuDevice) -> Result<()> {
        gpu.retire(Resource::Buffer(self.slots))?;
        Ok(())
    }

    /// The last frame's counters.
    pub(crate) fn counters(&self) -> &SunPageCounters {
        &self.counters
    }

    /// The next receiver-word frame tag (16 bits, never zero); wraps by clearing.
    pub(crate) fn next_tag(&mut self) -> (u32, bool) {
        self.tag = self.tag % 0xffff + 1;
        (self.tag, self.tag == 1)
    }
}

/// One frame's inputs to the service's plan (`VirtualShadowMap::plan`'s arguments).
struct Inputs<'a> {
    sun: Vec3,
    bounds: [Vec3; 2],
    casters: Casters<'a>,
    pass_ms: [Option<f64>; 2],
    settle: bool,
}

/// The largest distance any point of world box `bounds` (at transform `new`) moved since
/// transform `old`: `(new - old) new^-1` is affine, so its largest length over the box is
/// at a corner. The dynamic layer's bound for a rigidly moved caster (the renderer keeps
/// each moved row's pre-move transform; only service mode evaluates this).
pub(crate) fn rigid_displacement(old: Mat4, new: Mat4, [lo, hi]: [Vec3; 2]) -> f32 {
    let delta = (new - old) * new.inverse();
    (0..8u32)
        .map(|c| {
            let p = Vec3::new(
                if c & 1 == 0 { lo.x } else { hi.x },
                if c & 2 == 0 { lo.y } else { hi.y },
                if c & 4 == 0 { lo.z } else { hi.z },
            );
            (delta * p.extend(1.)).truncate().length()
        })
        .fold(0., f32::max)
}

/// Grid "rotation" handed to the native caster grid: world to (ground x, ground z,
/// height), so its light-space rectangles are this frame's sheared rectangles.
fn grid_matrix(k: Vec2, y_ref: f32) -> Mat4 {
    Mat4::from_cols(
        Vec4::new(1., 0., 0., 0.),
        Vec4::new(-k.x, -k.y, 1., 0.),
        Vec4::new(0., 1., 0., 0.),
        Vec4::new(k.x * y_ref, k.y * y_ref, 0., 1.),
    )
}

/// Ground rectangle (min, max) of world page `xy` at `level`, dilated by `texels`.
fn page_rect(level: usize, xy: IVec2, texels: f32) -> (Vec2, Vec2) {
    let step = page_step(level);
    let pad = texels * step / PAGE_SIZE as f32;
    let lo = xy.as_vec2() * step;
    (lo - pad, lo + Vec2::splat(step + pad))
}

/// Texel radius of a ring's window (shadow-pages.slang `SP_RING`).
const RING_TEXELS: f32 = 3.5;
/// Ring levels a pixel reads: its own and every second coarser one (shadow-pages.slang
/// `SP_RINGS`).
const RINGS: usize = 8;

/// Lowers `r`'s receiver height to `low`, counting at most one feedback frame per frame.
fn feed(r: &mut Record, low: f32, frame: u64) {
    r.receivers = Some(r.receivers.map_or(low, |v| v.min(low)));
    if r.fed != frame {
        r.fed = frame;
        r.feedback += 1;
    }
}

/// Inverse of the mark shader's receiver encoding: the height of a word's low half.
fn receiver_height(word: u32, depth: [f32; 2]) -> f32 {
    let q = 65535 - (word & 0xffff);
    depth[0] + (depth[1] - depth[0]) * q as f32 / 65535.
}

/// Largest sun angular radius (radians) the window tops are sized for (the real sun is
/// 0.00465; Look's sun radius stays below this).
const MAX_SUN_RADIUS: f32 = 0.01;

/// Padding (m) of the shared height range: casters may rise or sink this far past the
/// content before every page re-keys.
const HEIGHT_PAD: f32 = 64.;

impl VirtualShadowMap {
    /// The service form of [`VirtualShadowMap::plan`]: `instances` are the native
    /// instance rows' world boxes, `pass_ms` the completed static and dynamic page
    /// passes' GPU milliseconds (the budgets learn per-page cost from them).
    pub(super) fn plan_sun(
        &mut self,
        gpu: &mut GpuDevice,
        sun: Vec3,
        bounds: [Vec3; 2],
        casters: Casters<'_>,
        pass_ms: [Option<f64>; 2],
        settle: bool,
    ) -> Result<Vec<PageDraw>> {
        self.frame += 1;
        let frame = self.frame;
        let mut service = self.service.take().context("planned in service mode")?;
        let inputs = Inputs {
            sun,
            bounds,
            casters,
            pass_ms,
            settle,
        };
        let result = self.plan_service(gpu, &mut service, inputs);
        service.counters.frame = frame;
        self.service = Some(service);
        result
    }

    fn plan_service(
        &mut self,
        gpu: &mut GpuDevice,
        s: &mut Service,
        inputs: Inputs<'_>,
    ) -> Result<Vec<PageDraw>> {
        let Inputs {
            sun,
            bounds,
            casters:
                Casters {
                    all,
                    moved,
                    displacement,
                    instances,
                },
            pass_ms,
            settle,
        } = inputs;
        let frame = self.frame;
        let y_ref = s.settings.reference_height;
        for (layer, ms) in pass_ms.into_iter().enumerate() {
            if let Some(ms) = ms {
                s.budgets[layer].measured(s.last_drawn[layer], ms);
            }
        }
        s.counters = SunPageCounters {
            sun_pages: Counters {
                cap_ms: s.settings.static_cap_ms.map(f64::from),
                ..Counters::default()
            },
            dynamic_pages: Counters {
                cap_ms: s.settings.dynamic_cap_ms.map(f64::from),
                ..Counters::default()
            },
            ..SunPageCounters::default()
        };
        let Some(k) = oblique::shear(sun) else {
            // Too low for sheared pages: shading keeps the cascades (no rows).
            s.counters.inactive = true;
            s.shear = None;
            s.layers = [0; 2];
            return Ok(Vec::new());
        };
        let depth = [bounds[0].y - HEIGHT_PAD, bounds[1].y + HEIGHT_PAD];
        let rekey = all
            || self.space.is_none_or(|space| {
                space.depth[0] > bounds[0].y - 1. || space.depth[1] < bounds[1].y + 1.
            });
        if rekey {
            // New content or casters past the height range: every page re-keys.
            self.revision += 1;
            self.table_mirror.fill([0; 2]);
            gpu.upload_queued(self.table, bytemuck::cast_slice(&self.table_mirror))?;
            s.records.clear();
            s.heights.rebuild(instances);
            s.last_shear = None;
        } else {
            s.heights.update(instances);
        }
        let space = if rekey {
            LightSpace {
                rotation: grid_matrix(k, y_ref),
                depth,
            }
        } else {
            LightSpace {
                rotation: grid_matrix(k, y_ref),
                ..self.space.context("kept light space")?
            }
        };
        self.space = Some(space);
        s.last_shear = s.shear.filter(|_| !rekey);
        s.shear = Some(k);
        let rate_dk = s.last_shear.map_or(0., |last| (k - last).length());
        s.counters.shear = k.length();
        s.counters.shear_rate = rate_dk;
        self.cache.begin(
            frame,
            &[ara_vsm::Light {
                id: SUN_PAGES,
                projection: Projection::Directional(0),
                vp: Mat4::IDENTITY,
                revision: self.revision,
            }],
        )?;
        // Marks of completed frames: demanded pages and their lowest receivers.
        let completed = gpu.completed()?;
        for i in 0..self.requests.len() {
            let Some(requested) = self.requested[i] else {
                continue;
            };
            let Some(point) = requested.point else {
                self.requested[i] = None;
                continue;
            };
            if completed[point.queue.index()] < point.value {
                if !settle {
                    continue;
                }
                gpu.wait(point)?;
            }
            self.requested[i] = None;
            if requested.revision != self.revision {
                continue;
            }
            let bytes = gpu.read_completed(self.requests[i], point)?;
            let words: &[u32] = bytemuck::cast_slice(&bytes);
            let (bits, receivers) = words.split_at(super::REQUEST_WORDS);
            let tag = s.tags[i];
            let mut keys = Vec::new();
            let mut reported = Vec::new();
            super::requested_pages(bits, requested.camera, |level, xy| {
                let key = key(SUN_PAGES, level, xy);
                keys.push(key);
                let word = receivers[table_index(level, xy)];
                let low = (word >> 16 == tag).then(|| receiver_height(word, space.depth));
                let r = s.records.entry(key).or_insert(Record {
                    page: None,
                    level: level as u8,
                    xy,
                    drawn: None,
                    receivers: None,
                    feedback: 0,
                    fed: 0,
                    moved: 0.,
                    shift: (0., 0),
                    demanded: frame,
                    since: None,
                });
                r.demanded = frame;
                if let Some(low) = low {
                    feed(r, low, frame);
                    reported.push((level, xy, low));
                }
            });
            // Receiver words come from each pixel's own level; the ring pages two, four, ...
            // levels coarser read the same pixels (the ground grid nests), so they take those
            // heights over the dilated square their rings reach.
            for &(level, xy, low) in &reported {
                let (lo, hi) = page_rect(level, xy, 0.);
                for coarse in (1..RINGS).map(|i| level + 2 * i) {
                    if coarse >= super::LEVELS {
                        continue;
                    }
                    let step = page_step(coarse);
                    let pad = (RING_TEXELS + 1.) * step / PAGE_SIZE as f32;
                    let a = ((lo - pad) / step).floor().as_ivec2();
                    let b = ((hi + pad) / step).floor().as_ivec2();
                    for y in a.y..=b.y {
                        for x in a.x..=b.x {
                            let k = key(SUN_PAGES, coarse, IVec2::new(x, y));
                            if let Some(r) = s.records.get_mut(&k) {
                                feed(r, low, frame);
                            }
                        }
                    }
                }
            }
            for page in self.cache.request(keys)? {
                self.assign_sun(gpu, s, page)?;
                if let Some(r) = s.records.get_mut(&page.key) {
                    // Assigned but not yet drawn (or invalidated): cold.
                    r.drawn = None;
                    r.page = Some(page);
                }
            }
        }
        // Moved casters: the pages their old-and-new boxes touch, each by the caster's
        // shadow shift (a point moving by d moves its ground position by at most
        // d sqrt(1 + |k|^2)).
        let shift_of =
            |d: Option<f32>| d.map_or(f32::INFINITY, |d| d * (1. + k.length_squared()).sqrt());
        let touch = |r: &mut Record, shift: f32| {
            r.shift = if r.shift.1 == frame {
                (r.shift.0.max(shift), frame)
            } else {
                (shift, frame)
            };
        };
        if !rekey {
            for (i, &b) in moved.iter().enumerate() {
                let shift = shift_of(displacement.get(i).copied().flatten());
                let (lo, hi) = oblique::sheared_rect(b, k, y_ref);
                for level in 0..super::LEVELS {
                    let step = page_step(level);
                    let pad = step / PAGE_SIZE as f32;
                    let a = ((lo - pad) / step).floor().as_ivec2();
                    let c = ((hi + pad) / step).floor().as_ivec2();
                    if (c - a).max_element() >= 64 {
                        // A caster wider than 64 pages: test the level's records.
                        let first = key(SUN_PAGES, level, IVec2::MIN);
                        let last = key(SUN_PAGES, level, IVec2::MAX);
                        for (_, r) in s.records.range_mut(first..=last) {
                            if r.xy.cmpge(a).all() && r.xy.cmple(c).all() {
                                touch(r, shift);
                            }
                        }
                        continue;
                    }
                    for y in a.y..=c.y {
                        for x in a.x..=c.x {
                            if let Some(r) =
                                s.records.get_mut(&key(SUN_PAGES, level, IVec2::new(x, y)))
                            {
                                touch(r, shift);
                            }
                        }
                    }
                }
            }
        }
        // This frame's demand.
        let mut demand: [Vec<Demand<PageKey>>; 2] = [Vec::new(), Vec::new()];
        for (&key, r) in &mut s.records {
            // Every page accumulates its casters' shifts, read this frame or not.
            let step = if r.shift.1 == frame { r.shift.0 } else { 0. };
            r.moved += step;
            if frame.saturating_sub(r.demanded) > DEMAND_FRAMES {
                r.since = None;
                continue;
            }
            let level = r.level as usize;
            let texel = page_step(level) / PAGE_SIZE as f32;
            let (lo, hi) = page_rect(level, r.xy, 0.);
            let near = (s.camera.clamp(lo, hi) - s.camera).length() <= s.settings.near_radius;
            let line = if near {
                s.settings.near_lag_line
            } else {
                s.settings.lag_line
            };
            let resident = self.cache.resident(key).is_some();
            let (layer, cold, lag, rate) = match r.drawn {
                Some(drawn) if resident => {
                    let heights = drawn.heights.map(|h| PageHeights {
                        receivers: [
                            r.receivers
                                .filter(|_| r.feedback >= FEEDBACK_FRAMES)
                                .map_or(h.receivers[0], |v| v.max(h.receivers[0])),
                            h.receivers[1],
                        ],
                        ..h
                    });
                    let lag = heights.map_or(0., |h| h.lag_texels((k - drawn.k).length(), texel));
                    let rate = heights.map_or(0., |h| h.reach() * rate_dk / texel);
                    let dynamic = r.moved / texel;
                    let layer = if dynamic > 0. && dynamic >= lag {
                        Layer::Dynamic
                    } else {
                        Layer::Static
                    };
                    let lag = lag + dynamic;
                    if lag >= s.settings.trigger * line {
                        (layer, false, lag, rate + step / texel)
                    } else {
                        r.since = None;
                        continue;
                    }
                }
                _ => (Layer::Static, true, f32::INFINITY, 0.),
            };
            let since = *r.since.get_or_insert(frame);
            demand[layer as usize].push(Demand {
                key,
                cold,
                level: r.level,
                lag,
                rate,
                line,
                near,
                since,
            });
        }
        s.counters.cold = demand[0].iter().filter(|d| d.cold).count() as u64;
        let mut draws = Vec::new();
        let mut counts = [0; 2];
        let mut room = MAX_DRAWS;
        for layer in [Layer::Static, Layer::Dynamic] {
            let list = &mut demand[layer as usize];
            schedule::order(list);
            let budget = s.budgets[layer as usize].pages().min(room);
            let counters = match layer {
                Layer::Static => &mut s.counters.sun_pages,
                Layer::Dynamic => &mut s.counters.dynamic_pages,
            };
            let (picked, _) = schedule::select(list, budget, frame, counters);
            room -= picked.len();
            for key in picked {
                // Its assignment, unless the slot went to another page since.
                let Some(page) = s.records.get(&key).and_then(|r| r.page) else {
                    continue;
                };
                if self.slot_keys[page.slot as usize] != Some(key) {
                    continue;
                }
                let Projection::Directional(level) = key.projection else {
                    continue;
                };
                let (level, xy) = (level as usize, IVec2::from_array(key.xy));
                counts[layer as usize] += 1;
                draws.push(PageDraw {
                    page,
                    level,
                    xy,
                    matrix: oblique::page_matrix(k, y_ref, space.depth, page_step(level), xy),
                });
            }
        }
        s.layers = counts;
        s.counters.resident = s
            .records
            .iter()
            .filter(|(key, r)| r.drawn.is_some() && self.cache.resident(**key).is_some())
            .count() as u64;
        let bytes = self.cache.bytes()
            + (self.table_mirror.len() * 8) as u64
            + (self.requests.len() * REQUEST_WORDS * 4) as u64
            + (s.slots_mirror.len() * 16) as u64;
        s.counters.sun_pages.resident_bytes = Some(bytes);
        s.counters.dynamic_pages.resident_bytes = Some(0);
        Ok(draws)
    }

    /// The service form of [`VirtualShadowMap::drawn`]: `draws` render this frame;
    /// `rest` (whose casters did not fit) stay demand. Each drawn page records the shear
    /// it was drawn under and its heights, and the page table points at it.
    pub(super) fn drawn_sun(
        &mut self,
        gpu: &mut GpuDevice,
        draws: &[PageDraw],
        rest: &[PageDraw],
    ) -> Result<()> {
        let mut s = self.service.take().context("drawn in service mode")?;
        let result = (|| -> Result<()> {
            let k = s.shear.context("pages drawn under a shear")?;
            let y_ref = s.settings.reference_height;
            let fit_static = s.layers[0].min(draws.len());
            s.layers = [fit_static, draws.len() - fit_static];
            s.last_drawn = s.layers;
            s.counters.drawn = [s.layers[0] as u64, s.layers[1] as u64];
            // Pages that did not fit stay queued: their debt counts them already.
            let _ = rest;
            if draws.is_empty() {
                return Ok(());
            }
            let matrices: Vec<[f32; 16]> = draws.iter().map(|d| d.matrix.to_cols_array()).collect();
            gpu.upload_queued(self.matrices, bytemuck::cast_slice(&matrices))?;
            for draw in draws {
                let index = table_index(draw.level, draw.xy);
                let [occupant, _] = self.table_mirror[index];
                if occupant != 0
                    && occupant - 1 != draw.page.slot
                    && let Some(old) = self.slot_keys[(occupant - 1) as usize]
                {
                    self.cache.invalidate([old]);
                    if let Some(r) = s.records.get_mut(&old) {
                        r.drawn = None;
                    }
                }
                self.set_entry(gpu, index, super::table_entry(draw.page.slot, draw.xy))?;
                // The lag bound reads the page's own contents (one texel of slack); the
                // shader's window top covers every blocker whose sample rays can reach a
                // receiver in the page: the content's height span times the shear's
                // spread across the sun disc (|dk/dtheta| <= 1 + |k|^2, the disc at most
                // MAX_SUN_RADIUS).
                let heights =
                    s.heights
                        .page(page_rect(draw.level, draw.xy, 1.), k, y_ref, &mut s.scratch);
                let [lo, hi] = s.heights.span();
                let spread = (hi - lo).max(0.) * MAX_SUN_RADIUS * (1. + k.length_squared());
                let (a, b) = page_rect(draw.level, draw.xy, 1.);
                let top = s
                    .heights
                    .page((a - spread, b + spread), k, y_ref, &mut s.scratch)
                    .map_or(f32::NEG_INFINITY, |h| h.top);
                let record = [k.x, k.y, top, 0.];
                let slot = draw.page.slot as usize;
                if s.slots_mirror[slot] != record {
                    s.slots_mirror[slot] = record;
                    gpu.upload_queued_range(
                        s.slots,
                        (slot * 16) as u64,
                        bytemuck::cast_slice(&[record]),
                    )?;
                }
                if let Some(r) = s.records.get_mut(&draw.page.key) {
                    r.drawn = Some(Drawn { k, heights });
                    r.moved = 0.;
                    r.since = None;
                }
            }
            Ok(())
        })();
        self.service = Some(s);
        result
    }

    /// Gives `page`'s slot to its key (the evicted page leaves the table and the
    /// service's records).
    fn assign_sun(
        &mut self,
        gpu: &mut GpuDevice,
        s: &mut Service,
        page: ara_vsm::DirtyPage,
    ) -> Result<()> {
        if let Some(old) = self.slot_keys[page.slot as usize]
            && old != page.key
        {
            s.records.remove(&old);
        }
        self.assign(gpu, page)
    }

    /// The service's look-record rows (look/frame.rs `vsm*`): x/y/z/w as the control's,
    /// request y the per-slot shear records and w 1 (service), the light space's
    /// columns the shear and reference height, depth `[a, b]` with depth = a y + b.
    pub(super) fn rows_sun(&self, request: Option<usize>) -> Option<super::Rows> {
        let s = self.service.as_ref()?;
        let k = s.shear?;
        let space = self.space?;
        let [lo, hi] = space.depth;
        Some(super::Rows {
            vsm: [
                self.table_index,
                self.atlas_sampled,
                self.matrices_index,
                super::COLUMNS,
            ],
            request: [
                request.map_or(0, |i| self.request_indices[i]),
                s.slots_index,
                self.rows,
                request.map_or(1, |i| 1 | s.tags[i] << 16),
            ],
            space: Mat4::from_cols(
                Vec4::new(k.x, k.y, s.settings.reference_height, 0.),
                Vec4::ZERO,
                Vec4::ZERO,
                Vec4::ZERO,
            ),
            depth: [
                1. / (hi - lo),
                -lo / (hi - lo),
                self.settings.bias,
                super::FIRST_RADIUS,
            ],
        })
    }

    /// The service form of [`VirtualShadowMap::arm`]: the window is centred on the
    /// camera's ground position; bits are cleared and receiver words carry a fresh tag
    /// (all of them cleared only when the 16-bit tag wraps).
    pub(super) fn arm_sun(&mut self, gpu: &mut GpuDevice, camera: Vec3) -> Result<Option<usize>> {
        let i = (self.frame % self.requests.len() as u64) as usize;
        if self.space.is_none() || self.requested[i].is_some() {
            return Ok(None);
        }
        let Some(s) = self.service.as_mut() else {
            return Ok(None);
        };
        if s.shear.is_none() {
            return Ok(None);
        }
        let (tag, wrapped) = s.next_tag();
        s.tags[i] = tag;
        s.camera = Vec2::new(camera.x, camera.z);
        if wrapped {
            for buffer in self.requests {
                gpu.upload_queued(buffer, &vec![0; REQUEST_WORDS * 4])?;
            }
        } else {
            gpu.upload_queued(self.requests[i], &[0; super::REQUEST_WORDS * 4])?;
        }
        self.requested[i] = Some(Requested {
            camera: Vec2::new(camera.x, camera.z),
            revision: self.revision,
            point: None,
        });
        Ok(Some(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rigid_displacement_bounds_every_point_and_is_exact_for_translation() {
        let old = Mat4::from_translation(Vec3::new(1., 0., 2.)) * Mat4::from_rotation_y(0.3);
        let new = Mat4::from_translation(Vec3::new(1.4, 0.2, 1.9))
            * Mat4::from_rotation_y(0.5)
            * Mat4::from_scale(Vec3::splat(1.0));
        // The instance's world box at `new`: a unit cube's corners.
        let corners = (0..8u32).map(|c| {
            Vec3::new(
                (c & 1) as f32 - 0.5,
                ((c >> 1) & 1) as f32 - 0.5,
                ((c >> 2) & 1) as f32 - 0.5,
            )
        });
        let world: Vec<Vec3> = corners.clone().map(|q| new.transform_point3(q)).collect();
        let bounds = world.iter().fold(
            [Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)],
            |[lo, hi], p| [lo.min(*p), hi.max(*p)],
        );
        let d = rigid_displacement(old, new, bounds);
        for q in corners {
            for s in [0.25_f32, 0.5, 1.] {
                let q = q * s;
                let moved = (new.transform_point3(q) - old.transform_point3(q)).length();
                assert!(moved <= d + 1e-5, "{moved} > {d}");
            }
        }
        let shift = Vec3::new(0.3, -0.1, 0.2);
        let exact = rigid_displacement(new, Mat4::from_translation(shift) * new, bounds);
        assert!((exact - shift.length()).abs() < 1e-5);
    }
}
