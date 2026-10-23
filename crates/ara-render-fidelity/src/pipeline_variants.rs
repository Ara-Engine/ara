//! Prewarmed scene capabilities keep unused atmosphere and layered BRDFs out of hot shaders.
use crate::{FidelityView, Tier};

pub(crate) enum Transport {
    Primary,
    Probes,
    Gather,
    Reference,
}
impl FidelityView {
    pub(crate) fn transport_pipeline(&self, kind: Transport) -> &'static str {
        if self.settings.tier != Tier::Rt {
            return match kind {
                Transport::Primary => "primary_raster",
                _ => "hybrid_raster",
            };
        }
        let names = if self.atmosphere.is_some() {
            [
                "primary_atmosphere",
                "ddgi_atmosphere",
                "hybrid_atmosphere",
                "reference_atmosphere",
            ]
        } else if self.scene.has_layers {
            [
                "primary_layers",
                "ddgi_layers",
                "hybrid_layers",
                "reference_layers",
            ]
        } else if self.scene.has_emitters || self.scene.many_local_lights {
            [
                "primary_emissive",
                "ddgi_emissive",
                "hybrid_emissive",
                "reference_emissive",
            ]
        } else {
            ["primary", "ddgi", "hybrid", "reference"]
        };
        names[kind as usize]
    }
}
