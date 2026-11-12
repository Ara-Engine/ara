//! Diagnostic switches a host sets on a presenter (the editor's View > Rendering debug,
//! `ara capture --view`, `play.renderDebug`). The presenter itself reads the exposure
//! lock, the debug view and the reference request; the other switches name effects an
//! external renderer is told it cannot turn off one at a time (`ExternalView::debug_switches`).
use serde::{Deserialize, Serialize};

/// Effects a debug view may turn off. The default keeps everything on.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RenderDebug {
    /// Screen-space ambient occlusion.
    pub ambient_occlusion: bool,
    /// Sun shadows (cascades, far map, virtual shadow maps, terrain horizon) and
    /// local-light shadows.
    pub shadows: bool,
    /// Soft shadows (`shaders/look-shadow.slangh`): penumbrae sized by the light's
    /// shape; off, every light shadows as a point (the narrowest filter), to see
    /// what the penumbrae add.
    pub soft_shadows: bool,
    /// The near froxel volume of volumetric fog (`Quality::volumetric_fog`): off, fog
    /// falls back to the coarse 4 km grid alone, as on Low.
    pub volumetric_fog: bool,
    /// The temporal sub-pixel jitter that temporal anti-aliasing and upscaling
    /// accumulate; off, edges alias but hold still.
    pub temporal_jitter: bool,
    /// Render at display resolution instead of the preset's upscaled render scale.
    /// Applied where the render size is chosen (`ara-runtime-render`'s presenter).
    pub full_resolution: bool,
    /// What the lit image shows of reflections.
    pub reflections: ReflectionView,
    /// Screen-space reflections; off, reflections come from captures, probes and sky.
    pub screen_reflections: bool,
    /// Reflection captures (authored and automatic); off, probes and sky reflect.
    pub reflection_captures: bool,
    /// Probe global illumination (bounce light and probe reflections); off, the sky
    /// lights everything, as with no probe volume. The probes keep updating (the
    /// frame record's shading weight is zero), so switching back is instant.
    pub global_illumination: bool,
    /// Screen probe GI (`Quality::screen_gi`, High/Epic); off, the probe volume and
    /// screen-space ambient occlusion light everything, as on Medium.
    pub screen_gi: bool,
    /// Render the ground truth instead: the path tracer (ara-render-fidelity
    /// `Mode::Reference`), unbiased and undenoised, on GPUs with hardware ray
    /// tracing whatever the project's ray-tracing opt-in. For judging every real-time
    /// approximation against; never for shipping frames.
    pub reference: Option<ReferenceTrace>,
    /// Lock exposure at this EV100 (manual exposure, no compensation), overriding
    /// every post-process volume, so images from different renderers and presets
    /// compare at the same brightness. Applied where hosts resolve the frame's look
    /// (`ara_runtime_render::environment_locked`).
    pub exposure: Option<f32>,
    /// The debug view presenters draw instead of Lit (live windows and captures):
    /// `ara capture --view` and `play.renderDebug`.
    #[serde(default = "lit")]
    pub view: ara_frame::capture::ViewMode,
}

fn lit() -> ara_frame::capture::ViewMode {
    ara_frame::capture::ViewMode::Lit
}

/// The path-traced reference's budget ([`RenderDebug::reference`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceTrace {
    /// Independent samples per pixel the image accumulates (8 per frame).
    pub samples: u32,
    /// Scattering events per path, 1 to 16.
    pub bounces: u32,
}

impl Default for ReferenceTrace {
    fn default() -> Self {
        Self {
            samples: 1024,
            bounces: 8,
        }
    }
}

/// The reflection debug views: the lit image, the reflected light alone, or where
/// each pixel's reflection comes from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReflectionView {
    /// The lit image.
    #[default]
    Lit,
    /// Only the reflected (indirect specular) light, exposed like the lit image.
    Reflections,
    /// Each pixel coloured by the share of its reflection each source resolved:
    /// on-screen trace green, capture trace blue, screen probes yellow, box-projected
    /// captures cyan, probes orange, sky magenta; surfaces too rough to trace dimmed.
    Sources,
}

impl Default for RenderDebug {
    fn default() -> Self {
        Self {
            ambient_occlusion: true,
            shadows: true,
            soft_shadows: true,
            volumetric_fog: true,
            temporal_jitter: true,
            full_resolution: false,
            reflections: ReflectionView::Lit,
            screen_reflections: true,
            reflection_captures: true,
            global_illumination: true,
            screen_gi: true,
            reference: None,
            exposure: None,
            view: ara_frame::capture::ViewMode::Lit,
        }
    }
}
