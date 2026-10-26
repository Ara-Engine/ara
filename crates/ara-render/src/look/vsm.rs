//! Virtual shadow maps for the sun ([`Quality::virtual_shadows`]): Unreal-style
//! clipmaps of 16k virtual texels per level (radius 2^level metres), split into
//! 128-texel pages that are rendered only where visible pixels need them and kept
//! across frames in world space, so camera motion does not invalidate static pages.
//!
//! Each frame `VSM.Mark` records the pages lit pixels sample (level by texel
//! footprint) into a bitmask; once that frame completes, its marks reach the
//! deterministic [`ara_vsm::Cache`], which allocates physical pages (LRU). Dirty
//! pages render under a per-frame budget into one D32 atlas through the level's
//! light view cropped to the page; moved casters invalidate the pages their old and
//! new bounds touch. `VSM.Project` looks each G-buffer pixel's level up in a
//! toroidal page table (`look-vsm.slangh`, falling back to the next three coarser
//! levels) into a render-size mask lighting reads, which uses the cascades and far
//! map where it has no page; GI relight compares its hits in its own pipeline. No
//! other shader inlines page lookups: their registers would cost every preset.
//!
//! Conventions: light space is the sun's view rotation (no translation); world pages
//! are `floor(light xy / step)` with `step` the level's page width; atlas rows run
//! from light +y down (shaders are cooked with `-fvk-invert-y`); depth is one fixed
//! mapping over the content's light-axis extent for every level, so cached pages
//! stay comparable as the camera moves.
//!
//! [`Quality::virtual_shadows`]: super::Quality::virtual_shadows
use crate::graph::State;
use anyhow::{Context, Result};
use ara_gpu::{
    Access, Buffer, Device, Image, Queue, Resource, TimelinePoint, vk, vulkan::VulkanDevice,
};
use ara_vsm::{DirtyPage, PAGE_SIDE, PAGE_SIZE, PageKey, Projection};
use glam::{IVec2, Mat4, Vec2, Vec3};
use std::collections::{BTreeSet, VecDeque};

/// Clipmap levels: level `l` covers a radius of `2^l` metres around the camera.
pub(crate) const LEVELS: usize = 16;
/// Radius of level zero, in metres.
pub(crate) const FIRST_RADIUS: f32 = 1.;
/// Bits in a frame's page requests: one per level and toroidal page.
pub(crate) const REQUEST_WORDS: usize = LEVELS * (PAGE_SIDE * PAGE_SIDE) as usize / 32;
/// Shadow draws of this frame's page `i` carry root `cascade` `PAGE_DRAWS + i`
/// (native-raster.slang `shadowMatrix`).
pub(crate) const PAGE_DRAWS: u32 = 1024;
/// Light-axis padding of the shared depth range, in metres: casters may move this
/// far past the content before every page re-keys.
const DEPTH_PAD: f32 = 64.;

/// The sun's light space: a fixed view rotation and the depth range every page
/// shares (the content's extent along the light, padded).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LightSpace {
    pub rotation: Mat4,
    /// Light-space z range `[near, far]` (view space looks along -z).
    pub depth: [f32; 2],
}

impl LightSpace {
    /// The light space of a sun towards `sun` (unit, towards the light) over world
    /// bounds `bounds`.
    pub(crate) fn new(sun: Vec3, bounds: [Vec3; 2]) -> Self {
        let travel = -sun;
        let up = if travel.y.abs() > 0.99 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        let rotation = Mat4::look_at_rh(Vec3::ZERO, travel, up);
        let [lo, hi] = bounds;
        let (mut near, mut far) = (f32::INFINITY, f32::NEG_INFINITY);
        for corner in 0..8u32 {
            let pick = |bit: u32, axis: usize| {
                if corner >> bit & 1 == 0 {
                    lo[axis]
                } else {
                    hi[axis]
                }
            };
            let z = rotation
                .transform_point3(Vec3::new(pick(0, 0), pick(1, 1), pick(2, 2)))
                .z;
            (near, far) = (near.min(z), far.max(z));
        }
        // Light view space looks along -z: the top (nearest the sun) is the largest z.
        Self {
            rotation,
            depth: [near - DEPTH_PAD, far + DEPTH_PAD],
        }
    }

    /// Whether this space still serves `fresh` (same sun, its unpadded depth range
    /// inside this padded one), so cached pages stay comparable.
    fn holds(&self, fresh: &Self) -> bool {
        self.rotation == fresh.rotation
            && self.depth[0] <= fresh.depth[0] + DEPTH_PAD
            && fresh.depth[1] - DEPTH_PAD <= self.depth[1]
    }

    /// Light-space position of world `p`.
    pub(crate) fn project(&self, p: Vec3) -> Vec3 {
        self.rotation.transform_point3(p)
    }
}

/// Page width of level `level`, in metres.
pub(crate) fn page_step(level: usize) -> f32 {
    2. * FIRST_RADIUS * (1u32 << level) as f32 / PAGE_SIDE as f32
}

/// The world page holding light-space `xy` at `level`.
pub(crate) fn world_page(xy: Vec2, level: usize) -> IVec2 {
    (xy / page_step(level)).floor().as_ivec2()
}

/// The page key of world page `page` at `level` for light `light`.
pub(crate) fn key(light: u64, level: usize, page: IVec2) -> PageKey {
    PageKey {
        light,
        projection: Projection::Directional(level as u8),
        xy: page.to_array(),
    }
}

/// Toroidal page-table index of world page `page` at `level`.
pub(crate) fn table_index(level: usize, page: IVec2) -> usize {
    let side = PAGE_SIDE as i32;
    level * (side * side) as usize
        + (page.y.rem_euclid(side) * side + page.x.rem_euclid(side)) as usize
}

/// A page-table entry: physical slot plus one (zero: none) and the world page's
/// high bits (`page >> 7`, two signed 16-bit halves), which shading compares.
pub(crate) fn table_entry(slot: u32, page: IVec2) -> [u32; 2] {
    let high = |v: i32| ((v >> 7) as u32) & 0xffff;
    [slot + 1, high(page.x) | high(page.y) << 16]
}

/// The light view of world page `page` at `level`: its light-space square to the
/// whole clip square, depth over the shared range (reversed-Z: the sun side is
/// one). Atlas rows run from the page's +y edge.
pub(crate) fn page_matrix(space: &LightSpace, level: usize, page: IVec2) -> Mat4 {
    let step = page_step(level);
    let lo = page.as_vec2() * step;
    let [near, far] = space.depth;
    // depth = (z + n) / (n - f): n = -near puts the deepest point at zero, f = -far
    // the sunward one at one (reversed-Z).
    Mat4::orthographic_rh(lo.x, lo.x + step, lo.y, lo.y + step, -near, -far) * space.rotation
}

/// Physical atlas origin (texels) of slot `slot` in an atlas `columns` pages wide.
pub(crate) fn slot_origin(slot: u32, columns: u32) -> [u32; 2] {
    [slot % columns * PAGE_SIZE, slot / columns * PAGE_SIZE]
}

/// The world pages a frame's request bits name: bit `(level, toroidal index)` with
/// the frame's camera at light-space `camera` resolves to the page of that index in
/// the level's 128-page window around the camera.
pub(crate) fn requested_pages(words: &[u32], camera: Vec2, mut visit: impl FnMut(usize, IVec2)) {
    let side = PAGE_SIDE as i32;
    for (w, &word) in words.iter().enumerate() {
        let mut bits = word;
        while bits != 0 {
            let bit = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let index = w * 32 + bit;
            let level = index / (side * side) as usize;
            let local = (index % (side * side) as usize) as i32;
            let (tx, ty) = (local % side, local / side);
            // The window's first page, then the page there with this toroidal index.
            let first = world_page(camera, level) - IVec2::splat(side / 2);
            let wrap = |t: i32, f: i32| f + (t - f).rem_euclid(side);
            visit(level, IVec2::new(wrap(tx, first.x), wrap(ty, first.y)));
        }
    }
}

/// Light identity of the sun in the page cache.
const SUN: u64 = 1;
/// Atlas pages per row.
const COLUMNS: u32 = 32;
/// Caster rows a page may use on average (the rows buffer holds the budget's).
const ROWS_PER_PAGE: usize = 512;
/// A moved caster touching more pages than this per level re-keys every page.
const MOVED_PAGES: i32 = 16;

/// What a request buffer holds marks for: the frame's camera in light space, the
/// light space's revision then and the frame's submission (`None` until submitted).
#[derive(Clone, Copy)]
struct Requested {
    camera: Vec2,
    revision: u64,
    point: Option<TimelinePoint>,
}

/// The look record's VSM rows (look/frame.rs `vsm*`).
pub(crate) struct Rows {
    /// x page table, y atlas (sampled), z page matrices, w atlas pages per row.
    pub vsm: [u32; 4],
    /// x this frame's page-request buffer (zero: no marks), z atlas page rows.
    pub request: [u32; 4],
    /// The light rotation.
    pub space: Mat4,
    /// depth = x * light z + y; z level bias; w level zero's radius (m).
    pub depth: [f32; 4],
}

/// A page drawn this frame: its cache assignment, level, world page and light view.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PageDraw {
    pub page: DirtyPage,
    pub level: usize,
    pub xy: IVec2,
    pub matrix: Mat4,
}

impl PageDraw {
    /// The page's light-space square (min, max).
    pub(crate) fn rect(&self) -> (Vec2, Vec2) {
        let step = page_step(self.level);
        let lo = self.xy.as_vec2() * step;
        (lo, lo + Vec2::splat(step))
    }
}

/// The sun's virtual shadow map: physical atlas, toroidal page table, the ring of
/// page-request bitmasks and the page cache (look::vsm).
pub(crate) struct VirtualShadowMap {
    pub settings: super::VirtualShadows,
    pub atlas: Image,
    pub atlas_sampled: u32,
    pub atlas_state: State,
    /// Atlas page rows (`COLUMNS` pages per row).
    pub rows: u32,
    pub table: Buffer,
    pub table_index: u32,
    table_mirror: Vec<[u32; 2]>,
    /// This frame's page light views, one 64-byte row each.
    pub matrices: Buffer,
    pub matrices_index: u32,
    /// Packed caster rows of this frame's pages and their capacity.
    pub page_rows: Buffer,
    pub page_rows_index: u32,
    pub page_rows_capacity: usize,
    /// Page-request bitmasks, one per frame in flight.
    pub requests: [Buffer; 3],
    pub request_indices: [u32; 3],
    requested: [Option<Requested>; 3],
    cache: ara_vsm::Cache,
    slot_keys: Vec<Option<PageKey>>,
    pub space: Option<LightSpace>,
    revision: u64,
    /// Dirty pages waiting for the budget, and their keys.
    queue: VecDeque<DirtyPage>,
    queued: BTreeSet<PageKey>,
    frame: u64,
}

impl VirtualShadowMap {
    pub(crate) fn new(gpu: &mut VulkanDevice, settings: super::VirtualShadows) -> Result<Self> {
        let rows = settings.pages.div_ceil(COLUMNS);
        let mut owned: Vec<Resource> = Vec::new();
        let result = (|| {
            let atlas = gpu.create_image(
                COLUMNS * PAGE_SIZE,
                rows * PAGE_SIZE,
                vk::Format::D32_SFLOAT,
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
            )?;
            owned.push(Resource::Image(atlas));
            let mut buffer = |gpu: &mut VulkanDevice, bytes: usize| -> Result<(Buffer, u32)> {
                let buffer = gpu.create_buffer(
                    bytes.max(16) as u64,
                    vk::BufferUsageFlags::STORAGE_BUFFER,
                    true,
                )?;
                owned.push(Resource::Buffer(buffer));
                gpu.upload(buffer, &vec![0; bytes.max(16)])?;
                Ok((buffer, gpu.storage_buffer(buffer)?.index))
            };
            let entries = LEVELS * (PAGE_SIDE * PAGE_SIDE) as usize;
            let (table, table_index) = buffer(gpu, entries * 8)?;
            let (matrices, matrices_index) = buffer(gpu, settings.budget as usize * 64)?;
            let capacity = settings.budget as usize * ROWS_PER_PAGE;
            let (page_rows, page_rows_index) = buffer(gpu, capacity * 8)?;
            let ring = [
                buffer(gpu, REQUEST_WORDS * 4)?,
                buffer(gpu, REQUEST_WORDS * 4)?,
                buffer(gpu, REQUEST_WORDS * 4)?,
            ];
            let (requests, request_indices) = (ring.map(|r| r.0), ring.map(|r| r.1));
            let atlas_sampled = gpu.sampled_image(atlas, true)?.index;
            Ok::<_, anyhow::Error>(Self {
                settings,
                atlas,
                atlas_sampled,
                atlas_state: State {
                    access: Access::Undefined,
                    queue: Queue::Graphics,
                },
                rows,
                table,
                table_index,
                table_mirror: vec![[0; 2]; entries],
                matrices,
                matrices_index,
                page_rows,
                page_rows_index,
                page_rows_capacity: capacity,
                requests,
                request_indices,
                requested: [None; 3],
                cache: ara_vsm::Cache::new(settings.pages)?,
                slot_keys: vec![None; settings.pages as usize],
                space: None,
                revision: 0,
                queue: VecDeque::new(),
                queued: BTreeSet::new(),
                frame: 0,
            })
        })();
        if result.is_err() {
            for resource in owned {
                gpu.retire(resource)?;
            }
            gpu.collect()?;
        }
        result
    }

    fn set_entry(&mut self, gpu: &mut VulkanDevice, index: usize, entry: [u32; 2]) -> Result<()> {
        if self.table_mirror[index] != entry {
            self.table_mirror[index] = entry;
            gpu.upload_queued_range(
                self.table,
                (index * 8) as u64,
                bytemuck::cast_slice(&[entry]),
            )?;
        }
        Ok(())
    }

    /// Gives `page`'s slot to its key: the slot's previous page leaves the table
    /// (unless another page has taken its entry since).
    fn assign(&mut self, gpu: &mut VulkanDevice, page: DirtyPage) -> Result<()> {
        let slot = page.slot as usize;
        if let Some(old) = self.slot_keys[slot]
            && old != page.key
            && let Projection::Directional(level) = old.projection
        {
            let index = table_index(level as usize, IVec2::from_array(old.xy));
            if self.table_mirror[index][0] == page.slot + 1 {
                self.set_entry(gpu, index, [0; 2])?;
            }
        }
        self.slot_keys[slot] = Some(page.key);
        Ok(())
    }

    /// Queues dirty `page` unless it already waits.
    fn enqueue(&mut self, gpu: &mut VulkanDevice, page: DirtyPage) -> Result<()> {
        if self.queued.insert(page.key) {
            self.assign(gpu, page)?;
            self.queue.push_back(page);
        }
        Ok(())
    }

    /// This frame's page work for a sun towards `sun` over content `bounds` and
    /// caster changes (`all`: new content; `moved`: old and new bounds): consumes
    /// completed requests (`settle`: waits for them, so deterministic frames see
    /// pages at a fixed latency), invalidates the resident pages moved casters touch
    /// (they redraw first), and returns up to the budget of dirty pages, coarser
    /// levels before finer. Cull them, then `drawn` those that fit.
    pub(crate) fn plan(
        &mut self,
        gpu: &mut VulkanDevice,
        sun: Vec3,
        bounds: [Vec3; 2],
        (all, moved): (bool, &[[Vec3; 2]]),
        settle: bool,
    ) -> Result<Vec<PageDraw>> {
        self.frame += 1;
        let fresh = LightSpace::new(sun, bounds);
        if all || !self.space.is_some_and(|s| s.holds(&fresh)) {
            // A new sun, new content or casters past the depth range: every page
            // is re-keyed.
            self.space = Some(fresh);
            self.revision += 1;
            self.queue.clear();
            self.queued.clear();
            self.table_mirror.fill([0; 2]);
            gpu.upload_queued(self.table, bytemuck::cast_slice(&self.table_mirror))?;
        }
        let space = self.space.context("light space set above")?;
        self.cache.begin(
            self.frame,
            &[ara_vsm::Light {
                id: SUN,
                projection: Projection::Directional(0),
                vp: space.rotation,
                revision: self.revision,
            }],
        )?;
        // Marks of completed frames.
        let completed = gpu.completed()?;
        for i in 0..self.requests.len() {
            let Some(requested) = self.requested[i] else {
                continue;
            };
            // Armed by a frame that never submitted: nothing was marked.
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
            let mut keys = Vec::new();
            requested_pages(
                bytemuck::cast_slice(&bytes),
                requested.camera,
                |level, xy| keys.push(key_of(level, xy)),
            );
            let mut dirty = self.cache.request(keys)?;
            // Coarser levels first: they are what finer pages fall back to.
            dirty.sort_by_key(|p| std::cmp::Reverse(p.key.projection));
            for page in dirty {
                self.enqueue(gpu, page)?;
            }
        }
        // Moved casters: the resident pages their bounds touch redraw first.
        if !all {
            for &[lo, hi] in moved {
                let (min, max) = light_rect(&space, lo, hi);
                for level in 0..LEVELS {
                    let (a, b) = (world_page(min, level), world_page(max, level));
                    if (b - a).max_element() >= MOVED_PAGES {
                        continue;
                    }
                    for y in a.y..=b.y {
                        for x in a.x..=b.x {
                            let k = key_of(level, IVec2::new(x, y));
                            if self.cache.resident(k).is_none() {
                                continue;
                            }
                            self.cache.invalidate([k]);
                            for page in self.cache.request([k])? {
                                self.queue.retain(|p| p.key != page.key);
                                self.queue.push_front(page);
                                self.queued.insert(page.key);
                            }
                        }
                    }
                }
            }
        }
        let mut draws = Vec::new();
        while draws.len() < self.settings.budget as usize {
            let Some(page) = self.queue.pop_front() else {
                break;
            };
            self.queued.remove(&page.key);
            // Evicted since it was queued: its slot belongs to another page now.
            if self.slot_keys[page.slot as usize] != Some(page.key) {
                continue;
            }
            let Projection::Directional(level) = page.key.projection else {
                continue;
            };
            let (level, xy) = (level as usize, IVec2::from_array(page.key.xy));
            draws.push(PageDraw {
                page,
                level,
                xy,
                matrix: page_matrix(&space, level, xy),
            });
        }
        Ok(draws)
    }

    /// `draws` render this frame (in order; the rest of the plan waits for the next):
    /// their light views are uploaded and the page table points at them.
    pub(crate) fn drawn(
        &mut self,
        gpu: &mut VulkanDevice,
        draws: &[PageDraw],
        rest: &[PageDraw],
    ) -> Result<()> {
        for draw in rest.iter().rev() {
            self.queue.push_front(draw.page);
            self.queued.insert(draw.page.key);
        }
        if draws.is_empty() {
            return Ok(());
        }
        let matrices: Vec<[f32; 16]> = draws.iter().map(|d| d.matrix.to_cols_array()).collect();
        gpu.upload_queued(self.matrices, bytemuck::cast_slice(&matrices))?;
        for draw in draws {
            let index = table_index(draw.level, draw.xy);
            // The entry's previous page (128 pages away) stays resident but can no
            // longer be found: invalid, so a request for it redraws it.
            let [occupant, _] = self.table_mirror[index];
            if occupant != 0
                && occupant - 1 != draw.page.slot
                && let Some(key) = self.slot_keys[(occupant - 1) as usize]
            {
                self.cache.invalidate([key]);
            }
            self.set_entry(gpu, index, table_entry(draw.page.slot, draw.xy))?;
        }
        Ok(())
    }

    /// The frame that drew `draws` submitted, marking into request `request` (`arm`)
    /// at `point`: the cache publishes the pages (later frames sample after them in
    /// queue order), and the marks become readable once `point` completes.
    pub(crate) fn submitted(
        &mut self,
        draws: &[PageDraw],
        request: Option<usize>,
        point: TimelinePoint,
    ) {
        for draw in draws {
            self.cache.complete(draw.page);
        }
        if let Some(requested) = request.and_then(|i| self.requested[i].as_mut()) {
            requested.point = Some(point);
        }
    }

    /// Arms this frame's request buffer for a camera at `camera` (the frame record's):
    /// cleared (GPU-ordered) and remembered; `None` while its last marks are unread
    /// (no marks this frame).
    pub(crate) fn arm(&mut self, gpu: &mut VulkanDevice, camera: Vec3) -> Result<Option<usize>> {
        let i = (self.frame % self.requests.len() as u64) as usize;
        let Some(space) = self.space else {
            return Ok(None);
        };
        if self.requested[i].is_some() {
            return Ok(None);
        }
        gpu.upload_queued(self.requests[i], &[0; REQUEST_WORDS * 4])?;
        self.requested[i] = Some(Requested {
            camera: space.project(camera).truncate(),
            revision: self.revision,
            point: None,
        });
        Ok(Some(i))
    }

    /// Atlas texel x/y origin of `slot`.
    pub(crate) fn origin(&self, slot: u32) -> [u32; 2] {
        slot_origin(slot, COLUMNS)
    }

    /// The atlas's size in texels.
    pub(crate) fn size(&self) -> [u32; 2] {
        [COLUMNS * PAGE_SIZE, self.rows * PAGE_SIZE]
    }

    /// Whether the atlas holds a page (shading may sample it).
    pub(crate) fn written(&self) -> bool {
        self.atlas_state.access != Access::Undefined
    }

    /// The look record's VSM rows for a frame that marks into request buffer
    /// `request` (`arm`).
    pub(crate) fn rows(&self, request: Option<usize>) -> Option<Rows> {
        let space = self.space?;
        let [near, far] = space.depth;
        Some(Rows {
            vsm: [
                self.table_index,
                self.atlas_sampled,
                self.matrices_index,
                COLUMNS,
            ],
            request: [
                request.map_or(0, |i| self.request_indices[i]),
                0,
                self.rows,
                0,
            ],
            space: space.rotation,
            depth: [
                1. / (far - near),
                -near / (far - near),
                self.settings.bias,
                FIRST_RADIUS,
            ],
        })
    }

    pub(crate) fn destroy(self, gpu: &mut VulkanDevice) -> Result<()> {
        gpu.retire(Resource::Image(self.atlas))?;
        for buffer in [self.table, self.matrices, self.page_rows]
            .into_iter()
            .chain(self.requests)
        {
            gpu.retire(Resource::Buffer(buffer))?;
        }
        gpu.collect()?;
        Ok(())
    }
}

/// The sun's page key of world page `xy` at `level`.
fn key_of(level: usize, xy: IVec2) -> PageKey {
    key(SUN, level, xy)
}

/// Light-space x/y rectangle (min, max) of world bounds `lo..hi`.
fn light_rect(space: &LightSpace, lo: Vec3, hi: Vec3) -> (Vec2, Vec2) {
    let (mut min, mut max) = (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
    for corner in 0..8u32 {
        let pick = |bit: u32, axis: usize| {
            if corner >> bit & 1 == 0 {
                lo[axis]
            } else {
                hi[axis]
            }
        };
        let p = space
            .project(Vec3::new(pick(0, 0), pick(1, 1), pick(2, 2)))
            .truncate();
        (min, max) = (min.min(p), max.max(p));
    }
    (min, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space() -> LightSpace {
        LightSpace::new(
            Vec3::new(-0.6, 0.6, 0.5).normalize(),
            [Vec3::splat(-100.), Vec3::splat(100.)],
        )
    }

    #[test]
    fn a_page_matrix_maps_its_square_and_the_shared_depth() {
        let space = space();
        let level = 5;
        let p = Vec3::new(12.3, 4.5, -7.8);
        let light = space.project(p);
        let page = world_page(light.truncate(), level);
        let m = page_matrix(&space, level, page);
        let clip = m.project_point3(p);
        assert!(clip.x.abs() <= 1. && clip.y.abs() <= 1., "{clip}");
        assert!((0.0..=1.0).contains(&clip.z), "{clip}");
        // The same point through another page's matrix keeps its depth.
        let other = page_matrix(&space, level + 2, world_page(light.truncate(), level + 2));
        assert!((other.project_point3(p).z - clip.z).abs() < 1e-5);
        // Higher (sunward) points are nearer: larger reversed depth.
        let higher = m.project_point3(p + Vec3::new(-0.6, 0.6, 0.5).normalize());
        assert!(higher.z > clip.z);
    }

    #[test]
    fn request_bits_resolve_to_the_window_around_the_camera() {
        let level = 3;
        let camera = Vec2::new(1000.5, -321.25);
        let page = world_page(camera, level) + IVec2::new(17, -40);
        let index = table_index(level, page);
        let mut words = vec![0u32; REQUEST_WORDS];
        words[index / 32] |= 1 << (index % 32);
        let mut seen = Vec::new();
        requested_pages(&words, camera, |l, p| seen.push((l, p)));
        assert_eq!(seen, [(level, page)]);
    }
}
