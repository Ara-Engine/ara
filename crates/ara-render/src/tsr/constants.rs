//! The per-frame TSR record (`shaders/tsr/tsr.slangh` `TsrConstants`): FSR 3.1.4's
//! `cbFSR3Upscaler`, `cbRCAS` and `cbSPD` values as `ffx_fsr3upscaler.cpp` computes
//! them, plus Ara's bindless indices. Field order and 16-byte rows match the shader.
use bytemuck::{Pod, Zeroable};
use glam::Mat4;

/// GPU layout of `TsrConstants`; every row is a 4-component vector.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub(crate) struct TsrConstants {
    /// xy this frame's render size, zw the previous frame's.
    pub render_size: [i32; 4],
    /// xy display size, zw the previous frame's.
    pub upscale_size: [i32; 4],
    /// xy allocated render size, zw allocated display size.
    pub max_size: [i32; 4],
    /// FSR `fDeviceToViewDepth`: view depth = y / (device - x); zw inverse x/y
    /// projection scales.
    pub device_to_view_depth: [f32; 4],
    /// xy this frame's FSR jitter (render pixels, `-Frame.upscale.xy`), zw the
    /// previous frame's.
    pub jitter: [f32; 4],
    /// xy render / display, z tan of half the horizontal field of view, w jitter
    /// sequence length (phases).
    pub scale: [f32; 4],
    /// x frame index (0 on a reset), y velocity factor, z reactiveness scale, w
    /// shading change scale.
    pub frame: [f32; 4],
    /// x accumulation added per frame, y minimum disocclusion accumulation, z frame
    /// seconds, w view space to metres.
    pub accumulation: [f32; 4],
    /// x input pre-exposure (0: Ara's applied exposure), y the frame's exposure
    /// argument (the shader's fallback for an unset exposure state).
    pub exposure: [f32; 4],
    /// `FsrRcasCon` output (x sharpness as f32 bits; the packed-half y is unused at
    /// full precision).
    pub rcas: [u32; 4],
    /// Sampled colour, depth, motion (`u32::MAX` without) and the display frame
    /// record (storage buffer).
    pub inputs: [u32; 4],
    /// Exposure state buffer, scratch buffer, output storage image and this frame's
    /// ping-pong slot.
    pub resources: [u32; 4],
    /// Sampled indices of the TSR images (`mod.rs` image order, rows of four).
    pub sampled: [[u32; 4]; 3],
    /// Storage indices of the TSR images, as `sampled`.
    pub storage: [[u32; 4]; 3],
    /// Ara's signals (`shaders/tsr/tsr-signals.slangh`; `u32::MAX` absent): entity
    /// IDs, the opaque-only copy, the composited HDR it compares with, SSR trace
    /// (sampled).
    pub signal_inputs: [u32; 4],
    /// The opaque copy's valid render-pixel rect (x0, y0, x1, y1).
    pub opaque_rect: [u32; 4],
    /// x new-surface disocclusion, y same-surface depth tolerance (times FSR's
    /// required separation; 0 off), z reflection reactive, w composition weight.
    pub signals: [f32; 4],
    /// FSR autogen reactive: x scale, y threshold, z binary value; w parallax in
    /// render pixels for full reactivity.
    pub autogen: [f32; 4],
    /// x the host's reactive coverage (sampled; `u32::MAX` none).
    pub external: [u32; 4],
}

/// FSR 3.1.4 context defaults (`ffxFsr3UpscalerContextCreate`).
pub(crate) const VELOCITY_FACTOR: f32 = 1.;
pub(crate) const REACTIVENESS_SCALE: f32 = 1.;
pub(crate) const SHADING_CHANGE_SCALE: f32 = 1.;
pub(crate) const ACCUMULATION_ADDED_PER_FRAME: f32 = 1. / 3.;
pub(crate) const MIN_DISOCCLUSION_ACCUMULATION: f32 = -1. / 3.;
/// An entity absent from last frame's 3x3 at the reprojected pixel: a new surface,
/// fully disoccluded.
pub(crate) const NEW_SURFACE_DISOCCLUSION: f32 = 1.;
/// The same entity within four times FSR's required depth separation is the same
/// surface (FSR's own test flags 1×; real self-occlusion within one entity is larger).
pub(crate) const SAME_SURFACE_TOLERANCE: f32 = 4.;
/// Reactivity of confident screen-space reflections on a moving surface (light:
/// reflections then accumulate a little less, never reset).
pub(crate) const REFLECTION_REACTIVE: f32 = 0.25;
/// Composition weight where transparency changed the pixel: tighter history
/// clipping and fewer thin-feature locks, without discarding history.
pub(crate) const COMPOSITION: f32 = 0.5;
/// FSR's autogen reactive as AMD's sample dispatches it (scale, cutoff threshold,
/// binary value), then the parallax (render pixels) at which it applies fully.
pub(crate) const AUTOGEN: [f32; 4] = [1., 0.2, 0.9, 1.];
/// Only FSR's own auto exposure reads the frame time; Ara never feeds it a wall
/// clock (deterministic simulation), so it is a nominal 60 Hz.
pub(crate) const FRAME_SECONDS: f32 = 1. / 60.;

/// FSR's `setupDeviceDepthToViewSpaceDepthParams` from the projection matrix itself:
/// for clip z = A·z + B and clip w = -z, view depth = B / (device + A). Infinite
/// reversed-Z (A = 0) takes FSR's FLT_EPSILON so the sky stays finite.
pub(crate) fn device_to_view_depth(projection: Mat4) -> [f32; 4] {
    let a = projection.z_axis.z;
    let a = if a == 0. { f32::EPSILON } else { a };
    [
        -a,
        projection.w_axis.z,
        1. / projection.x_axis.x,
        1. / projection.y_axis.y,
    ]
}

/// `FsrRcasCon` for FSR's `sharpness` (0 none to 1 most): the API remaps it to
/// stops (`2 - 2 × sharpness`), then to a linear factor.
pub(crate) fn rcas(sharpness: f32) -> [u32; 4] {
    let stops = 2. - 2. * sharpness.clamp(0., 1.);
    [(-stops).exp2().to_bits(), 0, 0, 0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_depth_inverts_reversed_infinite_projection() {
        let projection = Mat4::perspective_infinite_reverse_rh(1.0, 16. / 9., 0.1);
        let [x, y, ..] = device_to_view_depth(projection);
        // FSR's FLT_EPSILON offset (finite sky) costs depth × epsilon / near relative.
        for depth in [0.1f32, 1., 10., 1000.] {
            let clip = projection * glam::Vec4::new(0., 0., -depth, 1.);
            let device = clip.z / clip.w;
            let tolerance = 1e-4 + depth * f32::EPSILON / 0.1;
            assert!((y / (device - x) - depth).abs() < depth * tolerance);
        }
    }

    #[test]
    fn record_is_twenty_three_rows() {
        assert_eq!(std::mem::size_of::<TsrConstants>(), 23 * 16);
    }

    #[test]
    fn rcas_maps_sharpness_to_stops() {
        assert_eq!(f32::from_bits(rcas(1.)[0]), 1.);
        assert_eq!(f32::from_bits(rcas(0.5)[0]), 0.5);
    }
}
