//! Atomic complete/failed reports preserve whole-run and arrival evidence.
use super::*;
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Outcome {
    Ready,
    TimedOut,
    Failed,
}
impl Window {
    pub(super) fn publish(
        &self,
        presenter: &Presenter,
        phases: &BTreeMap<&str, u128>,
        camera: Value,
        state: Outcome,
    ) -> Result<()> {
        let output = self
            .options
            .perf_output
            .as_ref()
            .context("metrics enabled")?;
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut cpu: BTreeMap<_, _> = self
            .frames
            .iter()
            .map(|(name, samples)| (name.to_owned(), quantiles(samples)))
            .collect();
        cpu.extend(self.cpu.report());
        let passes: BTreeMap<_, _> = self
            .passes
            .iter()
            .map(|(name, samples)| (name, quantiles(samples)))
            .collect();
        if state == Outcome::Ready {
            ensure!(
                !passes.is_empty(),
                "perf window did not receive GPU timestamp samples"
            );
        }
        let mut report = json!({"format":"ara.player.perf/1","pid":std::process::id(),"quality":presenter.quality().map(|q|q.name()),"presentation":if self.offscreen {"hiddenWindow"} else {"window"},"validation":cfg!(debug_assertions),"startup":phases,"camera":camera,"frames":self.measured,"warmup":self.options.perf_warmup,"cpu":cpu,"gpu_passes":passes,"gpu_other":self.gpu.report(),"gpu_coverage":self.gpu.coverage(),"shaders":presenter.shader_status()});
        report
            .as_object_mut()
            .context("performance report object")?
            .extend(self.span.report());
        report["gpu_timestamp_clock"] = presenter.renderer().map_or(Value::Null, |renderer| {
            let info = &renderer.gpu().context().info;
            json!({"extension":info.timestamp_clock,"period_ns":info.timestamp_period_ns,"valid_bits":info.timestamp_valid_bits})
        });
        report["completed"] = json!(state == Outcome::Ready);
        report["run"] = json!({"definition":"All game frames in this measurement request, including readiness and warmup; loading-view latency is in startup.","cpu":self.run_cpu.report()});
        report["arrival"] = self.arrival.report();
        if self.options.perf_wait_ready {
            let current = presenter.readiness();
            let status = self.ready_status.unwrap_or(current);
            let mut ready = self.boundary.report();
            ready["required"] = json!(true);
            ready["state"] = json!(state);
            ready["time_origin"] = json!(if self.from_launch {
                "launch"
            } else {
                "measurement_request"
            });
            ready["renderer"] = json!(status);
            ready["current_renderer"] = json!(current);
            ready["renderer_ready"] = json!(status.is_ready());
            ready["presentation_error"] = json!(presenter.presentation_error());
            report["readiness"] = ready;
        }
        let temporary = output.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(&report)?)?;
        std::fs::rename(temporary, output)?;
        eprintln!("Perf window published: {}", output.display());
        Ok(())
    }
}
