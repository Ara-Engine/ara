//! Rendering diagnosis switches a Play start may carry (the editor's View >
//! Rendering Debug); players apply them to every frame of that world.
use ara_schema::Schema;
use serde::{Deserialize, Serialize};

/// Effects to turn off so one artifact (flicker, aliasing, darkening) can be traced
/// to its source. Omitted fields keep the effect on; full resolution is opt-in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct RenderDebug {
    /// Screen-space ambient occlusion (default true).
    #[ara(default = true)]
    pub ambient_occlusion: bool,
    /// Sun and local-light shadows (default true).
    #[ara(default = true)]
    pub shadows: bool,
    /// The sub-pixel camera jitter temporal anti-aliasing accumulates (default true);
    /// off, edges alias but hold still.
    #[ara(default = true)]
    pub temporal_jitter: bool,
    /// Render at display resolution instead of the preset's upscaled scale (default false).
    pub full_resolution: bool,
    /// What the lit image shows of reflections (default `lit`).
    pub reflections: ReflectionView,
}

/// The reflection debug views.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(module = "play")]
#[serde(rename_all = "camelCase")]
pub enum ReflectionView {
    /// The lit image.
    #[default]
    Lit,
    /// Only the reflected (indirect specular) light, exposed like the lit image.
    Reflections,
    /// Each pixel coloured by where its reflection comes from: on-screen trace green,
    /// capture trace blue, box-projected captures cyan, probes orange, sky magenta;
    /// surfaces too rough to trace dimmed.
    Sources,
}

impl Default for RenderDebug {
    fn default() -> Self {
        Self {
            ambient_occlusion: true,
            shadows: true,
            temporal_jitter: true,
            full_resolution: false,
            reflections: ReflectionView::Lit,
        }
    }
}
