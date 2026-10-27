//! Scene capabilities select only the cooked pipelines needed by the current view.
use crate::{FidelityView, Tier};
use anyhow::{Result, ensure};
use ara_gpu::GpuDevice;

pub(crate) enum Transport {
    Primary,
    Probes,
    Gather,
    Reference,
}
impl FidelityView {
    /// Prewarms the selected transport at a content/settings boundary. Unchanged
    /// frames perform map lookups only; unused reference and material variants
    /// never enter the driver's expensive pipeline compiler.
    pub fn prepare_transport(&mut self, gpu: &mut GpuDevice) -> Result<()> {
        self.ensure_pipeline(gpu, self.transport_pipeline(Transport::Primary))?;
        if matches!(self.settings.mode, crate::Mode::Reference { .. }) {
            return self.ensure_pipeline(gpu, self.transport_pipeline(Transport::Reference));
        }
        self.ensure_pipeline(gpu, self.transport_pipeline(Transport::Probes))?;
        if self.reservoirs.is_none() {
            return self.ensure_pipeline(gpu, self.transport_pipeline(Transport::Gather));
        }
        let names = if self.atmosphere.is_some() {
            [
                "reservoir_init_atmosphere",
                "reservoir_trace_atmosphere",
                "reservoir_spatial_atmosphere",
                "reservoir_shade_atmosphere",
                "reservoir_specular_atmosphere",
            ]
        } else if self.scene.has_layers {
            [
                "reservoir_init_layers",
                "reservoir_trace_layers",
                "reservoir_spatial_layers",
                "reservoir_shade_layers",
                "reservoir_specular_layers",
            ]
        } else {
            [
                "reservoir_init",
                "reservoir_trace",
                "reservoir_spatial",
                "reservoir_shade",
                "reservoir_specular",
            ]
        };
        for name in names {
            self.ensure_pipeline(gpu, name)?;
        }
        Ok(())
    }

    pub(crate) fn ensure_pipeline(
        &mut self,
        gpu: &mut GpuDevice,
        name: &'static str,
    ) -> Result<()> {
        if self.pipelines.contains_key(name) {
            return Ok(());
        }
        let artifacts = ara_gpu::cooked::resource_dir(
            env!("CARGO_PKG_NAME"),
            concat!(env!("CARGO_MANIFEST_DIR"), "/cooked"),
        );
        let cooked: ara_gpu::cooked::CookedPipeline =
            serde_json::from_slice(&std::fs::read(artifacts.join(format!("{name}.json")))?)?;
        ensure!(
            cooked.key == self.shader_revision,
            "Mixed fidelity shader cohorts"
        );
        self.pipelines
            .insert(name, gpu.create_pipeline(&cooked, &artifacts)?);
        Ok(())
    }

    pub(crate) fn transport_pipeline(&self, kind: Transport) -> &'static str {
        if self.settings.tier != Tier::Rt {
            return match kind {
                Transport::Primary => "primary_raster",
                _ => "hybrid_raster",
            };
        }
        if matches!(kind, Transport::Primary) && self.reservoirs.is_some() {
            return if self.atmosphere.is_some() {
                "primary_reservoir_atmosphere"
            } else if self.scene.has_layers {
                "primary_reservoir_layers"
            } else {
                "primary_reservoir"
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
