//! Keep frame accounting paired before computing quantiles; missing coverage remains null.
use super::quantiles;
use ara_runtime_render::PresentStats;
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Samples {
    values: BTreeMap<&'static str, Vec<f64>>,
}

impl Samples {
    pub fn record(&mut self, stats: PresentStats, view_pass_ms: f64) {
        let mut push = |name, value| self.values.entry(name).or_default().push(value);
        push("total_ms", stats.gpu_ms);
        push("view_passes_ms", view_pass_ms);
        let Some(gpu) = stats.gpu else {
            return;
        };
        push("outside_view_ms", stats.gpu_ms - view_pass_ms);
        for (name, value) in [
            ("scene_update_ms", gpu.scene_update_ms),
            ("texture_upload_ms", gpu.texture_upload_ms),
            ("effects_ms", gpu.effects_ms),
            ("fidelity_ms", gpu.fidelity_ms),
            ("raster_ms", gpu.raster_ms),
            ("publication_ms", gpu.publication_ms),
            ("accounted_ms", gpu.accounted_ms()),
            ("unaccounted_ms", stats.gpu_ms - gpu.accounted_ms()),
            ("texture_uploaded_bytes", gpu.texture_uploaded_bytes as f64),
            ("texture_pending", gpu.texture_pending as f64),
            ("texture_resident_bytes", gpu.texture_resident_bytes as f64),
        ] {
            push(name, value);
        }
    }

    pub fn report(&self) -> Value {
        let mut report: BTreeMap<_, _> = [
            "scene_update_ms",
            "texture_upload_ms",
            "effects_ms",
            "fidelity_ms",
            "raster_ms",
            "publication_ms",
            "outside_view_ms",
            "accounted_ms",
            "unaccounted_ms",
            "texture_uploaded_bytes",
            "texture_pending",
            "texture_resident_bytes",
        ]
        .into_iter()
        .map(|name| (name, Value::Null))
        .collect();
        for (&name, samples) in &self.values {
            report.insert(name, quantiles(samples));
        }
        json!(report)
    }

    pub fn coverage(&self) -> Value {
        let count = |name| self.values.get(name).map_or(0, Vec::len);
        let native = count("accounted_ms");
        let unavailable = count("total_ms") - native;
        json!({
            "native_component_frames":native,
            "unavailable_component_frames":unavailable,
            "components":if native == 0 {"unavailable"} else if unavailable == 0 {"complete"} else {"partial"},
            "landscape":if native > 0 {"included_in_raster_ms_or_fidelity_ms"} else {"unavailable"},
            "effects_shared_post":if native > 0 {"included_in_raster_ms_or_fidelity_ms"} else {"unavailable"},
            "raster_ms":"native_geometry_look_shared_post; all_presets_including_Low",
            "fidelity_ms":"experimental_RT_only; zero_for_native_raster",
            "total":"presenter_timestamp_sum; legacy_fallback_has_incomplete_coverage"
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_runtime_render::GpuFrameCosts;

    #[test]
    fn accounting_sums_each_frame_before_quantiles_and_preserves_signed_residuals() {
        let mut samples = Samples::default();
        for (view, upload, effects) in [(9., 1., 1.), (1., 9., 1.), (1., 1., 9.)] {
            samples.record(
                PresentStats {
                    gpu_ms: 12.,
                    gpu: Some(GpuFrameCosts {
                        fidelity_ms: view,
                        texture_upload_ms: upload,
                        effects_ms: effects,
                        publication_ms: 1.,
                        texture_uploaded_bytes: 4096,
                        ..GpuFrameCosts::default()
                    }),
                    ..PresentStats::default()
                },
                view,
            );
        }
        let report = samples.report();
        assert_eq!(report["fidelity_ms"]["p50"], 1.);
        assert_eq!(report["texture_upload_ms"]["p50"], 1.);
        assert_eq!(report["effects_ms"]["p50"], 1.);
        assert_eq!(report["accounted_ms"]["p50"], 12.);
        assert_eq!(report["outside_view_ms"]["p50"], 11.);
        assert_eq!(report["unaccounted_ms"]["max"], 0.);
        assert_eq!(report["texture_uploaded_bytes"]["p50"], 4096.);

        let mut mismatch = Samples::default();
        mismatch.record(
            PresentStats {
                gpu_ms: 1.,
                gpu: Some(GpuFrameCosts {
                    fidelity_ms: 2.,
                    ..GpuFrameCosts::default()
                }),
                ..PresentStats::default()
            },
            2.,
        );
        assert_eq!(mismatch.report()["unaccounted_ms"]["p50"], -1.);
    }

    #[test]
    fn legacy_fallback_coverage_is_missing_instead_of_zero() {
        let mut samples = Samples::default();
        samples.record(
            PresentStats {
                gpu_ms: 4.,
                ..PresentStats::default()
            },
            3.,
        );
        let report = samples.report();
        assert!(report["effects_ms"].is_null());
        assert!(report["raster_ms"].is_null());
        assert!(report["unaccounted_ms"].is_null());
        assert!(report["outside_view_ms"].is_null());
        assert_eq!(samples.coverage()["landscape"], "unavailable");
        assert_eq!(samples.coverage()["components"], "unavailable");
        assert_eq!(samples.coverage()["unavailable_component_frames"], 1);
    }

    #[test]
    fn native_raster_components_are_complete_and_accounted_before_quantiles() {
        let mut samples = Samples::default();
        for (raster, upload, effects) in [(9., 1., 1.), (1., 9., 1.), (1., 1., 9.)] {
            samples.record(
                PresentStats {
                    gpu_ms: 12.,
                    gpu: Some(GpuFrameCosts {
                        raster_ms: raster,
                        texture_upload_ms: upload,
                        effects_ms: effects,
                        publication_ms: 1.,
                        ..GpuFrameCosts::default()
                    }),
                    ..PresentStats::default()
                },
                raster,
            );
        }
        let report = samples.report();
        assert_eq!(report["raster_ms"]["p50"], 1.);
        assert_eq!(report["fidelity_ms"]["p50"], 0.);
        assert_eq!(report["accounted_ms"]["p50"], 12.);
        assert_eq!(report["unaccounted_ms"]["max"], 0.);
        assert_eq!(samples.coverage()["components"], "complete");
        assert_eq!(samples.coverage()["native_component_frames"], 3);
        assert_eq!(samples.coverage()["unavailable_component_frames"], 0);
        assert_eq!(
            samples.coverage()["landscape"],
            "included_in_raster_ms_or_fidelity_ms"
        );

        // Presence supplies coverage even when a native frame has zero timings;
        // the preset or a nonzero raster duration must not determine availability.
        samples.record(
            PresentStats {
                gpu: Some(GpuFrameCosts::default()),
                ..PresentStats::default()
            },
            0.,
        );
        assert_eq!(samples.coverage()["components"], "complete");
        assert_eq!(samples.coverage()["native_component_frames"], 4);

        samples.record(PresentStats::default(), 0.);
        assert_eq!(samples.coverage()["components"], "partial");
        assert_eq!(samples.coverage()["unavailable_component_frames"], 1);
        assert_eq!(samples.report()["raster_ms"]["samples"], 4);
        assert_eq!(samples.report()["total_ms"]["samples"], 5);
    }
}
