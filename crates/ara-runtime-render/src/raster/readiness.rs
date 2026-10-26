use super::*;
use crate::PresentationReadiness;

impl RasterView {
    pub(crate) fn readiness(&self, renderer: &Renderer) -> PresentationReadiness {
        let textures = renderer.native_texture_stats();
        let gi_enabled = self.gi.is_some() && renderer.quality().gi_budget > 0;
        PresentationReadiness {
            raster: true,
            generation: self.key,
            content_prepared: self.prepared.is_some(),
            unresolved_assets: self
                .prepared
                .as_ref()
                .map_or(0, |p| p.unpublished_materials.len()),
            model_fades_pending: self.fades.is_active(),
            emitter_search_pending: self.emitter_search.is_busy(),
            textures_pending: textures.map(|t| t.pending_textures),
            texture_uploaded_bytes: textures.map_or(0, |t| t.uploaded_bytes),
            gi_enabled,
            gi_jobs_pending: gi_enabled
                && (self.pending_gi
                    || self
                        .gi
                        .as_ref()
                        .is_some_and(crate::gi::Acquisition::is_pending)),
            gi_records_pending: self
                .gi_updates
                .iter()
                .map(|(update, start)| update.len() - start)
                .sum(),
            gi_volume_ready: renderer.probe_gi_ready(),
            gpu_in_flight: renderer.scene_frame_pending(),
            ..Default::default()
        }
    }
}
