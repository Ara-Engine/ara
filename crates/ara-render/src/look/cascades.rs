//! Cascaded sun shadow maps: one D32 image per cascade, so each is redrawn only
//! when what it shows changed: its light matrix (the camera moved past its texel
//! snapping, or the sun turned), casters inside its light volume (their old or
//! new bounds), or new content. A still camera over a still scene draws none.
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
    /// The matrix each image was drawn with; `None` when it must be redrawn.
    drawn: Vec<Option<Mat4>>,
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
        })
    }

    /// Forgets every cascade whose drawn light volume holds changed casters
    /// (`bounds`: their old and new world bounds), or all of them for `all`.
    pub(crate) fn observe(&mut self, all: bool, bounds: &[[Vec3; 2]]) {
        for drawn in &mut self.drawn {
            if let Some(matrix) = *drawn
                && (all
                    || bounds
                        .iter()
                        .any(|&[lo, hi]| super::far::overlaps(matrix, lo, hi)))
            {
                *drawn = None;
            }
        }
    }

    /// Whether cascade `cascade` must be drawn for `matrix` this frame.
    pub(crate) fn needs_draw(&self, cascade: usize, matrix: Mat4) -> bool {
        self.drawn
            .get(cascade)
            .is_none_or(|drawn| *drawn != Some(matrix))
    }

    /// Cascade `cascade` now holds its casters for `matrix`.
    pub(crate) fn drawn(&mut self, cascade: usize, matrix: Mat4) {
        if let Some(drawn) = self.drawn.get_mut(cascade) {
            *drawn = Some(matrix);
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
}
