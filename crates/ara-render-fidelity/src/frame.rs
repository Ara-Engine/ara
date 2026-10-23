use crate::{settings::*, view::FidelityView};
use anyhow::{Result, ensure};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct GpuFrame {
    inverse_vp: Mat4,
    vp: Mat4,
    previous_vp: Mat4,
    camera: [f32; 4],
    previous_camera: [f32; 4],
    sizes: [u32; 4],
    scene: [u32; 4],
    origin: [f32; 4],
    spacing: [f32; 4],
    counts: [u32; 4],
    probes: [u32; 4],
    sun_direction: [f32; 4],
    sun_lux: [f32; 4],
    pub(crate) temporal: [f32; 4],
    post: [f32; 4],
    metering: [f32; 4],
    display: [u32; 4],
    lens: [f32; 4],
    pub(crate) control: [u32; 4],
    pub(crate) transport: [u32; 4],
    pub(crate) jitter: [f32; 4],
    pub(crate) read: [[u32; 4]; 6],
    write: [[u32; 4]; 6],
}
impl FidelityView {
    pub(crate) fn frame(&self, input: &FrameInput<'_>) -> Result<(GpuFrame, Mat4, Vec3)> {
        ensure!(
            input.delta_seconds.is_finite() && (0.0..=1.0).contains(&input.delta_seconds),
            "Invalid frame delta"
        );
        let s = self.settings;
        let camera = input.camera.resolve(s.render_size)?;
        let mut projection = camera.projection;
        let mut jitter = glam::Vec2::ZERO;
        if !input.deterministic && matches!(s.mode, Mode::Hybrid) {
            jitter = glam::Vec2::new(
                halton(input.tick % 256 + 1, 2) - 0.5,
                halton(input.tick % 256 + 1, 3) - 0.5,
            );
            projection = Mat4::from_translation(Vec3::new(
                jitter.x * 2. / s.render_size[0] as f32,
                -jitter.y * 2. / s.render_size[1] as f32,
                0.,
            )) * projection;
        }
        let vp = projection * camera.view;
        let unjittered = camera.projection * camera.view;
        let reset = input.reset || !self.history;
        let p = s.post;
        let exposure = p
            .ev100
            .map(|ev| exposure_from_ev100(ev, p.compensation))
            .unwrap_or(-2.0_f32.powf(p.compensation));
        let (reference, batch, bounces) = match s.mode {
            Mode::Hybrid => (
                0,
                1,
                u32::from(
                    self.scene.motion_pending && self.scene.acceleration_update().tlas_rebuilt,
                ) | (self.gather_events << 1)
                    | (u32::from(self.transport_stride == 4 || self.gather_events == 1) << 5),
            ),
            Mode::Reference { samples, bounces } => (
                1,
                samples
                    .saturating_sub(if reset { 0 } else { self.samples })
                    .min(8),
                bounces,
            ),
        };
        let f = GpuFrame {
            inverse_vp: vp.inverse(),
            vp: unjittered,
            previous_vp: if reset { unjittered } else { self.previous_vp },
            camera: camera.position.extend(0.).to_array(),
            previous_camera: if reset {
                camera.position
            } else {
                self.previous_camera
            }
            .extend(0.)
            .to_array(),
            sizes: [
                s.render_size[0],
                s.render_size[1],
                s.display_size[0],
                s.display_size[1],
            ],
            scene: [
                self.scene.table_index,
                self.scene.tlas_index,
                self.scene.environment,
                if self.scene.motion_pending {
                    self.scene
                        .previous_table
                        .map(|(_, index)| index)
                        .unwrap_or(self.scene.table_index)
                } else {
                    self.scene.table_index
                },
            ],
            origin: self.volume.origin.extend(0.).to_array(),
            spacing: self.volume.spacing.extend(0.).to_array(),
            counts: [
                self.volume.counts[0],
                self.volume.counts[1],
                self.volume.counts[2],
                self.volume.len() as u32,
            ],
            probes: [
                self.probe_indices[1 - self.probe_slot],
                self.probe_indices[self.probe_slot],
                (s.probe_ray_budget / 64).min(self.volume.len() as u32),
                self.probe_start,
            ],
            sun_direction: self.scene.sun_direction.extend(0.00465).to_array(),
            sun_lux: self
                .scene
                .sun_lux
                .extend(f32::from(
                    input.lighting_changed || self.responsive_frames > 0,
                ))
                .to_array(),
            temporal: [
                f32::from(reset),
                f32::from(input.lighting_changed),
                input.delta_seconds,
                s.reflection_cutoff,
            ],
            post: [exposure, p.adaptation, p.bloom, p.fog_density],
            metering: [
                p.auto_exposure.minimum_ev100,
                p.auto_exposure.maximum_ev100,
                p.auto_exposure.low_percentile,
                p.auto_exposure.high_percentile,
            ],
            display: [p.tone_mapper as u32, 0, 0, 0],
            lens: [
                p.focus_distance,
                p.aperture,
                p.shutter,
                if p.quality == Quality::Cinematic && s.tier != Tier::Mobile {
                    1.
                } else {
                    0.
                },
            ],
            control: [
                self.slot as u32,
                self.exposure_index,
                self.lut_index,
                input.tick,
            ],
            transport: [
                reference,
                batch,
                bounces,
                if reference == 0 {
                    u32::from(
                        !input.deterministic
                            && !reset
                            && !self.scene.motion_pending
                            && unjittered == self.previous_vp,
                    )
                } else if reset {
                    0
                } else {
                    self.samples
                },
            ],
            jitter: [
                jitter.x,
                jitter.y,
                self.previous_jitter.x,
                self.previous_jitter.y,
            ],
            read: self.targets.sampled.as_chunks::<4>().0.try_into()?,
            write: self.targets.storage.as_chunks::<4>().0.try_into()?,
        };
        Ok((f, unjittered, camera.position))
    }
}
fn halton(mut n: u32, base: u32) -> f32 {
    let mut value = 0.;
    let mut f = 1.;
    while n > 0 {
        f /= base as f32;
        value += f * (n % base) as f32;
        n /= base;
    }
    value
}
