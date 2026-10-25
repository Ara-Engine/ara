//! Cascaded sun shadow maps: one D32 image per cascade, so each is redrawn only
//! when what it shows changed: its light matrix (the camera moved past its texel
//! snapping, or the sun turned), casters inside its light volume (their old or
//! new bounds), or new content. A still camera over a still scene draws none.
//! At most one coarse cascade redraws per frame (`Cascades::plan`), so a frame
//! never pays for every cascade at once.
use crate::graph::State;
use anyhow::Result;
use ara_gpu::{Access, Device, Image, Queue, Resource, vk, vulkan::VulkanDevice};
use glam::{Mat4, Vec3};

pub(crate) struct Cascades {
    pub images: Vec<Image>,
    pub sampled: Vec<u32>,
    pub states: Vec<State>,
    /// Texels per side.
    pub resolution: u32,
    /// The matrix each image was drawn with; `None` before its first draw.
    drawn: Vec<Option<Mat4>>,
    /// Changed casters reach the drawn light volume: the map is out of date.
    stale: Vec<bool>,
    /// Frames each coarse cascade has waited for a redraw it needed.
    waiting: Vec<u32>,
}

impl Cascades {
    pub(crate) fn new(gpu: &mut VulkanDevice, count: u32, resolution: u32) -> Result<Self> {
        let mut images = Vec::new();
        let mut sampled = Vec::new();
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
            }
            Ok::<_, anyhow::Error>(())
        })();
        if let Err(error) = result {
            for image in images {
                gpu.retire(Resource::Image(image))?;
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
            drawn: vec![None; n],
            stale: vec![false; n],
            waiting: vec![0; n],
        })
    }

    /// Forgets every cascade whose drawn light volume holds changed casters
    /// (`bounds`: their old and new world bounds), or all of them for `all`.
    pub(crate) fn observe(&mut self, all: bool, bounds: &[[Vec3; 2]]) {
        for (drawn, stale) in self.drawn.iter().zip(&mut self.stale) {
            if let Some(matrix) = *drawn
                && (all
                    || bounds
                        .iter()
                        .any(|&[lo, hi]| super::far::overlaps(matrix, lo, hi)))
            {
                *stale = true;
            }
        }
    }

    /// Whether cascade `cascade` must be drawn for `matrix` this frame.
    pub(crate) fn needs_draw(&self, cascade: usize, matrix: Mat4) -> bool {
        self.drawn
            .get(cascade)
            .is_none_or(|drawn| *drawn != Some(matrix) || self.stale[cascade])
    }

    /// This frame's redraws: the first cascade whenever it needs one, and at
    /// most one coarse cascade. Coarse cascades whose drawn map no longer covers
    /// their view slice (`slices`, or that were never drawn) redraw regardless;
    /// otherwise the longest-waiting one (nearest on ties) takes the frame's
    /// coarse redraw. The rest keep sampling the matrix they were drawn with:
    /// `matrices` (the frame's) gets it back, and their coarse snapping margin
    /// covers the slice meanwhile.
    pub(crate) fn plan(
        &mut self,
        matrices: &mut [Mat4; 4],
        slices: &[(Vec3, f32); 4],
    ) -> Vec<usize> {
        let mut draws = Vec::new();
        let mut waiting = Vec::new();
        for i in 0..self.drawn.len().min(4) {
            if !self.needs_draw(i, matrices[i]) {
                continue;
            }
            match self.drawn[i] {
                Some(held) if i > 0 && covers(held, slices[i]) => waiting.push(i),
                _ => draws.push(i),
            }
        }
        if !draws.iter().any(|&i| i > 0)
            && let Some(&next) = waiting
                .iter()
                .max_by_key(|&&i| (self.waiting[i], std::cmp::Reverse(i)))
        {
            draws.push(next);
            waiting.retain(|&i| i != next);
        }
        for i in waiting {
            self.waiting[i] += 1;
            if let Some(held) = self.drawn[i] {
                matrices[i] = held;
            }
        }
        draws.sort_unstable();
        draws
    }

    /// Cascade `cascade` now holds its casters for `matrix`.
    pub(crate) fn drawn(&mut self, cascade: usize, matrix: Mat4) {
        if let Some(drawn) = self.drawn.get_mut(cascade) {
            *drawn = Some(matrix);
            self.stale[cascade] = false;
            self.waiting[cascade] = 0;
        }
    }

    pub(crate) fn destroy(self, gpu: &mut VulkanDevice) -> Result<()> {
        for image in self.images {
            gpu.retire(Resource::Image(image))?;
        }
        gpu.collect()?;
        Ok(())
    }
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

    #[test]
    fn a_cascade_redraws_only_for_its_matrix_or_casters_inside_it() {
        // A 20 m box around the origin, as a cascade's light volume.
        let matrix = Mat4::orthographic_rh(-10., 10., -10., 10., 10., -10.);
        let mut cascades = Cascades {
            images: Vec::new(),
            sampled: Vec::new(),
            states: Vec::new(),
            resolution: 1024,
            drawn: vec![None],
            stale: vec![false],
            waiting: vec![0],
        };
        assert!(cascades.needs_draw(0, matrix), "never drawn");
        cascades.drawn(0, matrix);
        assert!(!cascades.needs_draw(0, matrix));
        assert!(cascades.needs_draw(0, matrix * Mat4::from_translation(Vec3::X)));
        cascades.observe(false, &[[Vec3::splat(50.), Vec3::splat(51.)]]);
        assert!(!cascades.needs_draw(0, matrix), "a caster elsewhere");
        cascades.observe(false, &[[Vec3::splat(-1.), Vec3::splat(1.)]]);
        assert!(cascades.needs_draw(0, matrix), "a caster inside");
        cascades.drawn(0, matrix);
        cascades.observe(true, &[]);
        assert!(cascades.needs_draw(0, matrix), "new content");
    }

    #[test]
    fn coarse_cascades_redraw_one_per_frame_while_their_maps_still_cover() {
        // Four cascades, each a 20 m box around the origin with a 1 m slice in it.
        let box_at = |x: f32| {
            Mat4::orthographic_rh(-10., 10., -10., 10., 10., -10.)
                * Mat4::from_translation(Vec3::new(-x, 0., 0.))
        };
        let mut cascades = Cascades {
            images: Vec::new(),
            sampled: Vec::new(),
            states: Vec::new(),
            resolution: 1024,
            drawn: vec![None; 4],
            stale: vec![false; 4],
            waiting: vec![0; 4],
        };
        let slices = [(Vec3::ZERO, 1.); 4];
        let mut matrices = [box_at(0.); 4];
        assert_eq!(
            cascades.plan(&mut matrices, &slices),
            [0, 1, 2, 3],
            "never drawn"
        );
        for (i, &matrix) in matrices.iter().enumerate() {
            cascades.drawn(i, matrix);
        }
        // Every matrix moves half a metre: the first redraws, one coarse cascade
        // redraws, and the other two keep their drawn matrices, which still cover.
        let moved = [box_at(0.5); 4];
        let mut matrices = moved;
        let draws = cascades.plan(&mut matrices, &slices);
        assert_eq!(draws, [0, 1]);
        assert_eq!(
            matrices[2],
            box_at(0.),
            "a waiting cascade samples its drawn map"
        );
        for &i in &draws {
            cascades.drawn(i, matrices[i]);
        }
        // Next frame the longest-waiting coarse cascade goes first.
        let mut matrices = moved;
        assert_eq!(cascades.plan(&mut matrices, &slices), [2]);
        // A slice its drawn map no longer covers redraws at once, beyond the cap.
        let far_slices = [(Vec3::new(9.8, 0., 0.), 1.); 4];
        let mut matrices = [box_at(1.); 4];
        let draws = cascades.plan(&mut matrices, &far_slices);
        assert!(draws.contains(&2) && draws.contains(&3), "{draws:?}");
    }
}
