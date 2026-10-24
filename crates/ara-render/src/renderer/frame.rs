use super::*;
use crate::scene::Camera;
use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct Frame {
    pub vp: Mat4,
    pub previous_vp: Mat4,
    pub inverse_vp: Mat4,
    pub shadows: [Mat4; 4],
    pub camera: [f32; 4],
    pub forward: [f32; 4],
    pub sun_direction: [f32; 4],
    pub sun_color_exposure: [f32; 4],
    pub splits: [f32; 4],
    pub extent: [u32; 4],
    pub gbuffer: [u32; 4],
    pub extra: [u32; 4],
    pub lighting: [u32; 4],
    pub post: [u32; 4],
    pub temporal: [u32; 4],
    /// x: look constants buffer (look::frame::LookFrame).
    pub look: [u32; 4],
}
impl Renderer {
    pub(super) fn frame_data(
        &self,
        camera: &Camera,
        exposure: f32,
        deterministic: bool,
        mode: crate::capture::ViewMode,
        output_history: usize,
    ) -> Frame {
        let mut projection = camera.projection;
        if !deterministic {
            let sample = self.frame_number % 8 + 1;
            let jitter = glam::Vec2::new(halton(sample, 2) - 0.5, halton(sample, 3) - 0.5);
            projection = Mat4::from_translation(Vec3::new(
                jitter.x * 2. / self.size[0] as f32,
                jitter.y * 2. / self.size[1] as f32,
                0.,
            )) * projection;
        }
        let vp = projection * camera.view;
        let light_count = self.content.lights.len() as u32;
        let splits = [8., 18., 40., 100.];
        let previous = if self.targets.history.is_some() {
            self.previous_vp
        } else {
            vp
        };
        Frame {
            vp,
            previous_vp: previous,
            inverse_vp: vp.inverse(),
            shadows: shadow_cascades(camera, self.content.sun_direction, splits),
            camera: camera.position.extend(0.1).to_array(),
            // w selects the display curve (post::ToneCurve).
            forward: camera
                .forward
                .extend(self.look.look.curve as u32 as f32)
                .to_array(),
            sun_direction: self.content.sun_direction.extend(0.).to_array(),
            sun_color_exposure: self.content.sun_lux.extend(exposure).to_array(),
            splits,
            extent: [self.size[0], self.size[1], light_count, self.frame_number],
            gbuffer: [
                self.targets.sampled[ALBEDO],
                self.targets.sampled[NORMAL],
                self.targets.sampled[MATERIAL],
                self.targets.sampled[WORLD],
            ],
            extra: [
                self.targets.sampled[DEPTH],
                self.targets.sampled[ID],
                self.targets.sampled[SHADOW],
                self.content.environment,
            ],
            lighting: [
                self.content.irradiance,
                self.content.prefilter,
                self.targets.tile_index,
                self.content.light_index,
            ],
            post: [
                self.targets.sampled[HDR],
                self.targets.sampled[self.targets.history.unwrap_or(HDR)],
                self.targets.storage[output_history - HISTORY0],
                self.targets.storage[2],
            ],
            temporal: [
                u32::from(self.targets.history.is_some()),
                u32::from(deterministic),
                mode as u32,
                self.content.material_index,
            ],
            // w: sampled native motion (RG16F, unjittered UV delta); u32::MAX without.
            look: [self.look.frame_index, 0, 0, u32::MAX],
        }
    }
}
fn halton(mut index: u32, base: u32) -> f32 {
    let mut result = 0.;
    let mut f = 1.;
    while index > 0 {
        f /= base as f32;
        result += f * (index % base) as f32;
        index /= base;
    }
    result
}
fn shadow_cascades(camera: &Camera, sun: Vec3, splits: [f32; 4]) -> [Mat4; 4] {
    let inverse = (camera.projection * camera.view).inverse();
    let mut near = 0.1;
    std::array::from_fn(|i| {
        let far = splits[i];
        let mut corners = [Vec3::ZERO; 8];
        let mut index = 0;
        for d in [near, far] {
            for y in [-1., 1.] {
                for x in [-1., 1.] {
                    let p = inverse * glam::Vec4::new(x, y, 0.1 / d, 1.);
                    corners[index] = p.truncate() / p.w;
                    index += 1;
                }
            }
        }
        let centre = corners.iter().copied().sum::<Vec3>() / 8.;
        let radius = (corners
            .iter()
            .map(|p| p.distance(centre))
            .fold(0f32, f32::max)
            * 16.)
            .ceil()
            / 16.;
        let up = if sun.dot(Vec3::Y).abs() > 0.99 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        let view = Mat4::look_at_rh(centre + sun * (radius + 100.), centre, up);
        let light_centre = view.transform_point3(centre);
        let texel = radius * 2. / 512.;
        let x = (light_centre.x / texel).floor() * texel;
        let y = (light_centre.y / texel).floor() * texel;
        let zmin = corners
            .iter()
            .map(|p| view.transform_point3(*p).z)
            .fold(f32::INFINITY, f32::min)
            - 100.;
        let zmax = corners
            .iter()
            .map(|p| view.transform_point3(*p).z)
            .fold(f32::NEG_INFINITY, f32::max)
            + 100.;
        // Swapped near/far produces a 1->0 reversed-Z orthographic depth.
        let projection =
            Mat4::orthographic_rh(x - radius, x + radius, y - radius, y + radius, -zmin, -zmax);
        near = far;
        projection * view
    })
}
