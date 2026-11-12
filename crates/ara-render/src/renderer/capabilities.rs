//! What this renderer can show beyond Lit ([`RenderCapabilities`]), with the reason for
//! anything it cannot, so hosts disable a debug view or a probe capture truthfully
//! instead of drawing Lit under its name.
use super::*;
use ara_frame::capture::{RenderCapabilities, ViewMode};

impl Renderer {
    /// Reflection probes, GI diffuse and overdraw availability at the current preset
    /// and readiness, with a reason for each that is unavailable. Reflection probes
    /// shade in the High/Epic PSOs of the native raster renderer only.
    pub fn capabilities(&self) -> RenderCapabilities {
        let mut capabilities = RenderCapabilities::default();
        let mut deny = |key: &str, reason: &str| {
            capabilities
                .reasons
                .insert(key.to_owned(), reason.to_owned());
        };
        let quality = &self.look.quality;
        if !self.is_native() {
            deny(
                "reflectionProbes",
                "reflection probes shade on the native raster renderer only",
            );
        } else if !quality.high_tier || quality.reflection_captures.is_none() {
            deny(
                "reflectionProbes",
                "reflection probes shade at the High and Epic presets; the current preset has no reflection captures",
            );
        } else {
            capabilities.reflection_probes = true;
        }
        if self.is_native() {
            capabilities.gi_diffuse = true;
        } else {
            deny(
                "giDiffuse",
                "the gi_diffuse view shades on the native raster renderer only",
            );
        }
        if self.is_native() {
            capabilities.overdraw = true;
        } else {
            deny(
                "overdraw",
                "the overdraw view counts the native raster renderer's draws only",
            );
        }
        if self.is_native() {
            capabilities.vertex_colors = true;
        } else {
            deny(
                "vertexColors",
                "the vertex-colour views draw the native raster renderer's geometry only",
            );
        }
        if self.is_native() {
            capabilities.decals = true;
        } else {
            deny(
                "decals",
                "deferred decals blend into the native raster renderer's G-buffer only",
            );
        }
        capabilities
    }

    /// Why `mode` cannot be drawn now (`ARA_VIEW_MODE_UNAVAILABLE`), or `None` when it
    /// can. Modes 0-8 are always available.
    pub fn view_mode_unavailable(&self, mode: ViewMode) -> Option<String> {
        let key = match mode {
            ViewMode::GiDiffuse => "giDiffuse",
            ViewMode::Overdraw => "overdraw",
            ViewMode::DecalCoverage => "decals",
            _ if mode.vertex_color().is_some() => "vertexColors",
            _ => return None,
        };
        let capabilities = self.capabilities();
        let available = match mode {
            ViewMode::GiDiffuse => capabilities.gi_diffuse,
            ViewMode::Overdraw => capabilities.overdraw,
            ViewMode::DecalCoverage => capabilities.decals,
            _ => capabilities.vertex_colors,
        };
        (!available).then(|| {
            let reason = capabilities.reasons.get(key).cloned().unwrap_or_default();
            format!(
                "ARA_VIEW_MODE_UNAVAILABLE: {} view unavailable: {reason}",
                mode.name()
            )
        })
    }
}
