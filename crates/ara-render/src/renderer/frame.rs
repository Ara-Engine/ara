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
        let shadows = self.look.quality.shadows;
        let splits = cascade_splits(&shadows);
        let previous = if self.targets.history.is_some() {
            self.previous_vp
        } else {
            vp
        };
        Frame {
            vp,
            previous_vp: previous,
            inverse_vp: vp.inverse(),
            shadows: shadow_cascades(
                camera,
                self.content.sun_direction,
                splits,
                shadows.resolution,
            ),
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
                self.targets.sampled[EMISSIVE],
            ],
            extra: [
                self.targets.sampled[DEPTH],
                self.targets.sampled[ID],
                // Unused: the cascade maps live in the look record.
                0,
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
            look: [
                self.look.frame_index,
                shadows.resolution,
                shadows.cascades,
                if self.native.is_some() {
                    self.targets.sampled[MOTION]
                } else {
                    u32::MAX
                },
            ],
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
/// Practical split scheme (Zhang et al.): a 0.75 blend of logarithmic and uniform
/// far distances; unused cascades repeat the shadow distance.
pub(super) fn cascade_splits(shadows: &crate::look::Shadows) -> [f32; 4] {
    let (near, far, count) = (0.1f32, shadows.distance, shadows.cascades.max(1));
    std::array::from_fn(|i| {
        let t = ((i as u32 + 1).min(count)) as f32 / count as f32;
        0.75 * near * (far / near).powf(t) + 0.25 * (near + (far - near) * t)
    })
}
/// Light view-projections of the cascades. The light space has a fixed rotation
/// and world origin, and each cascade's centre snaps to whole texels, so shadows
/// stay still while the camera moves (no swimming edges). Cascades past the first
/// snap coarsely instead (a tenth of their radius, inside a matching margin), so
/// their cached maps (look::cascades) survive camera moves and turns for many
/// frames; the first keeps texel snapping for the sharpest near shadows.
fn shadow_cascades(camera: &Camera, sun: Vec3, splits: [f32; 4], resolution: u32) -> [Mat4; 4] {
    let inverse = (camera.projection * camera.view).inverse();
    let up = if sun.dot(Vec3::Y).abs() > 0.99 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let light = Mat4::look_at_rh(Vec3::ZERO, -sun, up);
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
        near = far;
        let centre = corners.iter().copied().sum::<Vec3>() / 8.;
        // A bounding sphere keeps the extent independent of view rotation.
        let radius = (corners
            .iter()
            .map(|p| p.distance(centre))
            .fold(0f32, f32::max)
            * 16.)
            .ceil()
            / 16.;
        let radius = if i == 0 { radius } else { radius * 1.1 };
        let texel = radius * 2. / resolution as f32;
        let step = if i == 0 {
            texel
        } else {
            ((radius * 0.1 / texel).round() * texel).max(texel)
        };
        let c = light.transform_point3(centre);
        let snap = |v: f32| (v / step).round() * step;
        let (x, y) = (snap(c.x), snap(c.y));
        // Light-space z grows towards the sun: casters up to 200 m sunward of the
        // cascade still shadow it. Coarse cascades snap their depth range too.
        let z = if i == 0 { c.z } else { snap(c.z) };
        let away = z - radius - 10. - (step - texel);
        let sunward = z + radius + 200.;
        // Near at the far side and far at the sunward side: reversed-Z depth (1 is
        // closest to the sun).
        Mat4::orthographic_rh(
            x - radius,
            x + radius,
            y - radius,
            y + radius,
            -away,
            -sunward,
        ) * light
    })
}
