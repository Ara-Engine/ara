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
}

impl Default for RenderDebug {
    fn default() -> Self {
        Self {
            ambient_occlusion: true,
            shadows: true,
            temporal_jitter: true,
            full_resolution: false,
        }
    }
}
