//! Reflected viewport extraction settings; hosts mutate documents through their
//! existing typed commands, then pass the extracted settings to render views.
use crate::DebugView;
use serde::{Deserialize, Serialize};
/// Stable editor/agent seam for viewport diagnostic menus and capture requests.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ara_schema::Schema)]
#[ara(module = "render")]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct RenderSettings {
    /// Lit, clusters, LOD, overdraw, raster backend, VSM pages or texture residency.
    #[ara(default = "lit")]
    pub debug_view: DebugView,
    /// Maximum projected cluster displacement in pixels.
    #[ara(default = 1, range(min = 0.01, max = 16))]
    pub geometry_error_pixels: f32,
    /// Texture physical-pool budget in MiB, including tile gutters.
    #[ara(default = 64, range(min = 1, max = 4096))]
    pub texture_budget_mib: u32,
    /// Native display output including paper white and peak luminance.
    pub output: ara_color::OutputSettings,
}
impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            debug_view: DebugView::Lit,
            geometry_error_pixels: 1.,
            texture_budget_mib: 64,
            output: Default::default(),
        }
    }
}
/// Registers the complete dependent enum/settings schema for editor inspectors.
pub fn register_schema(db: &mut ara_schema::SchemaDb) {
    db.register::<DebugView>();
    db.register::<ara_color::OutputMode>();
    db.register::<ara_color::OutputSettings>();
    db.register::<RenderSettings>();
}
