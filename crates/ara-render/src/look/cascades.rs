//! Cascaded sun shadow maps: one D32 image per cascade, so each is redrawn only
//! when what it shows changed: its light matrix (the camera moved past its texel
//! snapping, or the sun turned), casters inside its light volume (their old or
//! new bounds), or new content. A still camera over a still scene draws none.
//! Moved casters patch only the texels they reach (`CascadeDraw::rect`), so an
//! animated character costs a few hundred texels per tick, not whole cascades.
//! At most one coarse cascade redraws whole per frame (`Cascades::plan`), so a
//! frame never pays for every cascade at once.
use crate::graph::State;
use anyhow::Result;
use ara_gpu::{Access, Buffer, Device, GpuDevice, Image, Queue, Resource, vk};
use glam::{Mat4, Vec2, Vec3, Vec4};

/// A patch covers at most 1/`PATCH_AREA` of a map's texels; larger changes
/// redraw the whole map.
const PATCH_AREA: u32 = 4;

/// One cascade redraw this frame: the whole map, or only `rect` (texels
/// `[x0, y0, x1, y1]`, ends exclusive) that changed casters reach.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CascadeDraw {
    pub cascade: usize,
    pub rect: Option<[u32; 4]>,
}

/// A patch redraw as drawn: the square viewport holding its rect and the
/// cascade's light matrix cropped to it. Drawn through the cropped matrix at
/// that viewport, casters land on exactly the texels the whole map gives them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Patch {
    pub origin: [u32; 2],
    pub size: u32,
    pub matrix: Mat4,
}

impl Patch {
    /// The patch for `rect` in a map of `resolution` texels drawn with `matrix`.
    pub(crate) fn new(rect: [u32; 4], resolution: u32, matrix: Mat4) -> Self {
        // Square, so the cropped view keeps the map's texel density (shadow LOD
        // and small-caster culling) on both axes.
        let size = (rect[2] - rect[0]).max(rect[3] - rect[1]).min(resolution);
        let origin = [0, 1].map(|axis| {
            let centre = (rect[axis] + rect[axis + 2]) / 2;
            centre.saturating_sub(size / 2).min(resolution - size)
        });
        let scale = resolution as f32 / size as f32;
        // The patch centre in clip space (`texel`'s inverse) moves to the origin,
        // its extent to [-1, 1].
        let [x, y] = origin.map(|o| (2 * o + size) as f32 / resolution as f32 - 1.);
        let crop = Mat4::from_cols(
            Vec4::new(scale, 0., 0., 0.),
            Vec4::new(0., scale, 0., 0.),
            Vec4::Z,
            Vec4::new(-x * scale, y * scale, 0., 1.),
        );
        Self {
            origin,
            size,
            matrix: crop * matrix,
        }
    }

    /// The patch square in its target's clip space (`resolution` texels a side):
    /// minimum and maximum corners.
    pub(crate) fn clip(&self, resolution: u32) -> [Vec2; 2] {
        let r = resolution as f32;
        let (low, high) = (
            self.origin.map(|o| o as f32),
            self.origin.map(|o| (o + self.size) as f32),
        );
        // Texel rows run down from clip +Y (`texel`).
        [
            Vec2::new(2. * low[0] / r - 1., 1. - 2. * high[1] / r),
            Vec2::new(2. * high[0] / r - 1., 1. - 2. * low[1] / r),
        ]
    }
}

pub(crate) struct Cascades {
    pub images: Vec<Image>,
    pub sampled: Vec<u32>,
    pub states: Vec<State>,
    /// Texels per side.
    pub resolution: u32,
    /// Per cascade, a frame record whose matrix for that cascade is a patch's
    /// cropped one: patch draws read it through root word 0.
    pub frames: Vec<Buffer>,
    pub frame_indices: Vec<u32>,
    /// The matrix each image was drawn with; `None` before its first draw.
    drawn: Vec<Option<Mat4>>,
    /// Texels changed casters reach in the drawn map (all of it for new
    /// content); `Some` means the map is out of date.
    dirty: Vec<Option<[u32; 4]>>,
    /// Frames each coarse cascade has waited for a redraw it needed.
    waiting: Vec<u32>,
    /// Patches drawn since creation (diagnostics and tests).
    pub patched: u64,
}

impl Cascades {
    pub(crate) fn new(
        gpu: &mut GpuDevice,
        count: u32,
        resolution: u32,
        frame_bytes: u64,
    ) -> Result<Self> {
        let mut images = Vec::new();
        let mut sampled = Vec::new();
        let mut frames = Vec::new();
        let mut frame_indices = Vec::new();
        let result = (|| {
            for _ in 0..count {
                let image = gpu.create_image(
                    resolution,
                    resolution,
                    vk::Format::D32_SFLOAT,
                    vk::ImageUsageFlags::SAMPLED
                        | vk::ImageUsageFlags::TRANSFER_SRC
                        | vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
                )?;
                images.push(image);
                sampled.push(gpu.sampled_image(image, true)?.index);
                let frame =
                    gpu.create_buffer(frame_bytes, vk::BufferUsageFlags::STORAGE_BUFFER, true)?;
                frames.push(frame);
                frame_indices.push(gpu.storage_buffer(frame)?.index);
            }
            Ok::<_, anyhow::Error>(())
        })();
        if let Err(error) = result {
            for image in images {
                gpu.retire(Resource::Image(image))?;
            }
            for frame in frames {
                gpu.retire(Resource::Buffer(frame))?;
            }
            gpu.collect()?;
            return Err(error);
        }
        let n = images.len();
        Ok(Self {
            images,
            sampled,
            states: vec![
                State {
                    access: Access::Undefined,
                    queue: Queue::Graphics,
                };
                n
            ],
            resolution,
            frames,
            frame_indices,
            drawn: vec![None; n],
            dirty: vec![None; n],
            waiting: vec![0; n],
            patched: 0,
        })
    }

    /// Marks the texels of every drawn cascade that changed casters reach
    /// (`bounds`: their old and new world bounds), or all of them for `all`.
    pub(crate) fn observe(&mut self, all: bool, bounds: &[[Vec3; 2]]) {
        let full = [0, 0, self.resolution, self.resolution];
        for (drawn, dirty) in self.drawn.iter().zip(&mut self.dirty) {
            let Some(matrix) = *drawn else {
                continue;
            };
            if all {
                *dirty = Some(full);
                continue;
            }
            for &[lo, hi] in bounds {
                if let Some(rect) = texels(matrix, lo, hi, self.resolution) {
                    *dirty = Some(union(*dirty, rect));
                }
            }
        }
    }

    /// Whether cascade `cascade` must be drawn for `matrix` this frame.
    pub(crate) fn needs_draw(&self, cascade: usize, matrix: Mat4) -> bool {
        self.drawn
            .get(cascade)
            .is_none_or(|drawn| *drawn != Some(matrix) || self.dirty[cascade].is_some())
    }

    /// The dirty texels of `cascade` if a patch can redraw them.
    fn patch(&self, cascade: usize) -> Option<[u32; 4]> {
        self.dirty[cascade].filter(|&rect| patchable(rect, self.resolution))
    }

    /// This frame's redraws: casters that moved inside a map that is otherwise
    /// current are patched (`rect`); the first cascade redraws whole whenever it
    /// needs to, and at most one coarse cascade does. Coarse cascades whose drawn
    /// map no longer covers their view slice (`slices`, or that were never drawn)
    /// redraw regardless; otherwise the longest-waiting one (nearest on ties)
    /// takes the frame's coarse redraw. The rest keep sampling the matrix they
    /// were drawn with: `matrices` (the frame's) gets it back, and their coarse
    /// snapping margin covers the slice meanwhile. Their patches wait for that
    /// redraw: casters are culled for the frame's matrices only.
    pub(crate) fn plan(
        &mut self,
        matrices: &mut [Mat4; 4],
        slices: &[(Vec3, f32); 4],
    ) -> Vec<CascadeDraw> {
        let whole = |cascade| CascadeDraw {
            cascade,
            rect: None,
        };
        let mut draws = Vec::new();
        let mut waiting = Vec::new();
        for i in 0..self.drawn.len().min(4) {
            if !self.needs_draw(i, matrices[i]) {
                continue;
            }
            match (self.drawn[i], self.patch(i)) {
                (Some(held), Some(rect)) if held == matrices[i] => draws.push(CascadeDraw {
                    cascade: i,
                    rect: Some(rect),
                }),
                (Some(held), _) if i > 0 && covers(held, slices[i]) => waiting.push(i),
                _ => draws.push(whole(i)),
            }
        }
        if !draws.iter().any(|d| d.cascade > 0 && d.rect.is_none())
            && let Some(&next) = waiting
                .iter()
                .max_by_key(|&&i| (self.waiting[i], std::cmp::Reverse(i)))
        {
            draws.push(whole(next));
            waiting.retain(|&i| i != next);
        }
        for i in waiting {
            self.waiting[i] += 1;
            if let Some(held) = self.drawn[i] {
                matrices[i] = held;
            }
        }
        draws.sort_unstable_by_key(|d| d.cascade);
        draws
    }

    /// `draw` landed with `matrix`: a whole redraw now holds that matrix; a
    /// patch only brings the drawn one up to date.
    pub(crate) fn drawn(&mut self, draw: CascadeDraw, matrix: Mat4) {
        let i = draw.cascade;
        if i >= self.drawn.len() {
            return;
        }
        self.dirty[i] = None;
        if draw.rect.is_none() {
            self.drawn[i] = Some(matrix);
            self.waiting[i] = 0;
        } else {
            self.patched += 1;
        }
    }

    pub(crate) fn destroy(self, gpu: &mut GpuDevice) -> Result<()> {
        for image in self.images {
            gpu.retire(Resource::Image(image))?;
        }
        for frame in self.frames {
            gpu.retire(Resource::Buffer(frame))?;
        }
        gpu.collect()?;
        Ok(())
    }
}

/// The texels of a map drawn with `matrix` (`resolution` per side) that world
/// bounds `lo..hi` reach, with a texel of margin; `None` outside its volume.
fn texels(matrix: Mat4, lo: Vec3, hi: Vec3, resolution: u32) -> Option<[u32; 4]> {
    if !super::far::overlaps(matrix, lo, hi) {
        return None;
    }
    let [min, max] = super::far::projected(matrix, lo, hi);
    texel_rect(min.truncate(), max.truncate(), resolution)
}

/// The texels of a `resolution`-texel target between clip-space `min` and `max`
/// (x, y), with a texel of margin; `None` when empty.
pub(crate) fn texel_rect(min: Vec2, max: Vec2, resolution: u32) -> Option<[u32; 4]> {
    let r = resolution as f32;
    // Clip +Y is the top row, so the clip maximum gives the low texel row.
    let (low, high) = (
        texel(Vec2::new(min.x, max.y), r),
        texel(Vec2::new(max.x, min.y), r),
    );
    let low = (low.floor() - 1.).clamp(Vec2::ZERO, Vec2::splat(r));
    let high = (high.ceil() + 1.).clamp(Vec2::ZERO, Vec2::splat(r));
    let rect = [low.x as u32, low.y as u32, high.x as u32, high.y as u32];
    (rect[2] > rect[0] && rect[3] > rect[1]).then_some(rect)
}

/// Texel rectangle `rect` grown to hold `more` (`rect` empty when `None`).
pub(crate) fn union(rect: Option<[u32; 4]>, more: [u32; 4]) -> [u32; 4] {
    rect.map_or(more, |r| {
        [
            r[0].min(more[0]),
            r[1].min(more[1]),
            r[2].max(more[2]),
            r[3].max(more[3]),
        ]
    })
}

/// Whether a patch should redraw `rect` of a `resolution`-texel target, rather
/// than the whole target.
pub(crate) fn patchable(rect: [u32; 4], resolution: u32) -> bool {
    let side = (rect[2] - rect[0]).max(rect[3] - rect[1]);
    side * side * PATCH_AREA <= resolution * resolution
}

/// The texel position of clip-space `clip` in a `size`-texel render target.
/// Clip +Y is the top row: shaders are cooked with `-fvk-invert-y`.
fn texel(clip: Vec2, size: f32) -> Vec2 {
    Vec2::new(clip.x * 0.5 + 0.5, 0.5 - clip.y * 0.5) * size
}

/// Whether light matrix `matrix` (orthographic) holds the sphere: inside its
/// sides and its depth range.
fn covers(matrix: Mat4, (centre, radius): (Vec3, f32)) -> bool {
    let p = matrix.transform_point3(centre);
    let extent = |row: usize| radius * matrix.row(row).truncate().length();
    p.x.abs() + extent(0) <= 1.
        && p.y.abs() + extent(1) <= 1.
        && p.z - extent(2) >= 0.
        && p.z + extent(2) <= 1.
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `n` never-drawn cascades of 1024 texels without GPU resources.
    fn cascades(n: usize) -> Cascades {
        Cascades {
            images: Vec::new(),
            sampled: Vec::new(),
            states: Vec::new(),
            resolution: 1024,
            frames: Vec::new(),
            frame_indices: Vec::new(),
            drawn: vec![None; n],
            dirty: vec![None; n],
            waiting: vec![0; n],
            patched: 0,
        }
    }

    fn whole(cascade: usize) -> CascadeDraw {
        CascadeDraw {
            cascade,
            rect: None,
        }
    }

    fn indices(draws: &[CascadeDraw]) -> Vec<usize> {
        draws.iter().map(|d| d.cascade).collect()
    }

    /// A 20 m box around `x` on the X axis, as a cascade's light volume.
    fn box_at(x: f32) -> Mat4 {
        Mat4::orthographic_rh(-10., 10., -10., 10., 10., -10.)
            * Mat4::from_translation(Vec3::new(-x, 0., 0.))
    }

    #[test]
    fn a_cascade_redraws_only_for_its_matrix_or_casters_inside_it() {
        let matrix = box_at(0.);
        let mut cascades = cascades(1);
        assert!(cascades.needs_draw(0, matrix), "never drawn");
        cascades.drawn(whole(0), matrix);
        assert!(!cascades.needs_draw(0, matrix));
        assert!(cascades.needs_draw(0, matrix * Mat4::from_translation(Vec3::X)));
        cascades.observe(false, &[[Vec3::splat(50.), Vec3::splat(51.)]]);
        assert!(!cascades.needs_draw(0, matrix), "a caster elsewhere");
        cascades.observe(false, &[[Vec3::splat(-1.), Vec3::splat(1.)]]);
        assert!(cascades.needs_draw(0, matrix), "a caster inside");
        cascades.drawn(whole(0), matrix);
        cascades.observe(true, &[]);
        assert!(cascades.needs_draw(0, matrix), "new content");
    }

    #[test]
    fn coarse_cascades_redraw_one_per_frame_while_their_maps_still_cover() {
        // Four cascades, each a 20 m box around the origin with a 1 m slice in it.
        let mut cascades = cascades(4);
        let slices = [(Vec3::ZERO, 1.); 4];
        let mut matrices = [box_at(0.); 4];
        assert_eq!(
            cascades.plan(&mut matrices, &slices),
            [0, 1, 2, 3].map(whole),
            "never drawn"
        );
        for (i, &matrix) in matrices.iter().enumerate() {
            cascades.drawn(whole(i), matrix);
        }
        // Every matrix moves half a metre: the first redraws, one coarse cascade
        // redraws, and the other two keep their drawn matrices, which still cover.
        let moved = [box_at(0.5); 4];
        let mut matrices = moved;
        let draws = cascades.plan(&mut matrices, &slices);
        assert_eq!(draws, [0, 1].map(whole));
        assert_eq!(
            matrices[2],
            box_at(0.),
            "a waiting cascade samples its drawn map"
        );
        for &draw in &draws {
            cascades.drawn(draw, matrices[draw.cascade]);
        }
        // Next frame the longest-waiting coarse cascade goes first.
        let mut matrices = moved;
        assert_eq!(cascades.plan(&mut matrices, &slices), [whole(2)]);
        // A slice its drawn map no longer covers redraws at once, beyond the cap.
        let far_slices = [(Vec3::new(9.8, 0., 0.), 1.); 4];
        let mut matrices = [box_at(1.); 4];
        let draws = indices(&cascades.plan(&mut matrices, &far_slices));
        assert!(draws.contains(&2) && draws.contains(&3), "{draws:?}");
    }

    #[test]
    fn moved_casters_patch_only_the_texels_they_reach() {
        let slices = [(Vec3::ZERO, 1.); 4];
        let mut cascades = cascades(2);
        let mut matrices = [box_at(0.); 4];
        for draw in cascades.plan(&mut matrices, &slices) {
            cascades.drawn(draw, matrices[draw.cascade]);
        }
        // A 1 m caster moves at the centre: 51.2 texels per metre, plus a margin.
        cascades.observe(false, &[[Vec3::splat(-0.5), Vec3::splat(0.5)]]);
        let draws = cascades.plan(&mut matrices, &slices);
        assert_eq!(indices(&draws), [0, 1], "every cascade it reaches, no cap");
        let rect = draws[0].rect.expect("a patch");
        assert!(
            rect[0] <= 486 && rect[2] >= 538 && rect[2] - rect[0] <= 56,
            "{rect:?}"
        );
        // The cropped matrix at the patch viewport puts every point on the texel
        // the whole map gives it.
        let patch = Patch::new([480, 500, 530, 520], 1024, matrices[0]);
        assert_eq!((patch.origin, patch.size), ([480, 485], 50));
        for p in [Vec3::new(-0.6, 0.3, 0.), Vec3::new(0.4, -0.2, 2.)] {
            let whole = matrices[0].project_point3(p);
            let cropped = patch.matrix.project_point3(p);
            let at = texel(cropped.truncate(), 50.) + Vec2::new(480., 485.);
            assert!((at - texel(whole.truncate(), 1024.)).abs().max_element() < 1e-3);
            assert!((cropped.z - whole.z).abs() < 1e-6);
        }
        for &draw in &draws {
            cascades.drawn(draw, matrices[draw.cascade]);
        }
        assert!(cascades.plan(&mut matrices, &slices).is_empty());
        // Changes reaching more than a quarter of a map redraw it whole.
        cascades.observe(false, &[[Vec3::splat(-6.), Vec3::splat(6.)]]);
        assert_eq!(cascades.plan(&mut matrices, &slices), [0, 1].map(whole));
    }

    #[test]
    fn a_waiting_cascade_takes_moved_casters_with_its_whole_redraw() {
        let slices = [(Vec3::ZERO, 1.); 4];
        let mut cascades = cascades(3);
        let mut matrices = [box_at(0.); 4];
        for draw in cascades.plan(&mut matrices, &slices) {
            cascades.drawn(draw, matrices[draw.cascade]);
        }
        // The camera moves (cascade 2 waits for its whole redraw) as a caster moves.
        cascades.observe(false, &[[Vec3::splat(-0.5), Vec3::splat(0.5)]]);
        let mut matrices = [box_at(0.5); 4];
        let draws = cascades.plan(&mut matrices, &slices);
        assert_eq!(draws, [0, 1].map(whole), "no patch while it waits");
        assert_eq!(matrices[2], box_at(0.), "it samples its drawn map");
        for &draw in &draws {
            cascades.drawn(draw, matrices[draw.cascade]);
        }
        // It still waits for its whole redraw, which takes the next frame.
        let mut matrices = [box_at(0.5); 4];
        assert_eq!(cascades.plan(&mut matrices, &slices), [whole(2)]);
    }
}
