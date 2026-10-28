//! Before/after images require the caller's actual presented current Scene gate.
use super::*;
impl ProbeCheck {
    pub fn end(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
        scene_ready: bool,
    ) -> ara_ui::Result<()> {
        if self.finished() {
            return Ok(());
        }
        let result = self.end_presented(editor, shell, gpu, scene_ready);
        if let Err(error) = &result
            && self.error.is_none()
        {
            self.fail(editor, shell, &error.to_string())?;
        }
        result
    }
    fn end_presented(
        &mut self,
        editor: &Editor,
        shell: &Shell,
        gpu: &mut NativeUi,
        scene_ready: bool,
    ) -> ara_ui::Result<()> {
        if !scene_ready {
            if self.case_started.elapsed() >= PATIENCE {
                return Err("Native probe Scene presentation readiness timed out".into());
            }
            return Ok(());
        }
        if let Some(stage) = self.pending_capture.take() {
            self.capture(shell, gpu, stage)?;
        }
        if let Some(effect) = self.complete.take() {
            self.capture(shell, gpu, "after")?;
            self.results.push(json!({"journey":CASES[self.index],"ok":true,"nativeEvents":self.case_events,
                "before":self.before,"observed":effect,"after":self.evidence(editor,shell),"trace":self.trace,
                "seconds":self.case_started.elapsed().as_secs_f64(),"sceneReadyForImages":true}));
            self.index += 1;
            self.phase = 0;
            self.case_events = 0;
            self.trace.clear();
            self.case_started = Instant::now();
            self.write()?;
        }
        Ok(())
    }
    fn capture(&self, shell: &Shell, gpu: &mut NativeUi, stage: &str) -> ara_ui::Result<()> {
        gpu.select_target(0);
        let stem = format!("probe-{:02}-{}-{stage}", self.index + 1, CASES[self.index]);
        gpu.pixels()?
            .save(self.output.join(format!("{stem}.png")))?;
        std::fs::write(
            self.output.join(format!("{stem}.layout.json")),
            ara_json::to_canonical_string(&shell.layout_report()),
        )?;
        Ok(())
    }
    pub(super) fn evidence(&self, editor: &Editor, shell: &Shell) -> Value {
        json!({"revision":editor.host.store().revision(),"doc":editor.state.doc,"selection":editor.state.selection,
            "records":fixture::records(editor).map_err(|error|error.to_string()),"snapshot":self.snapshot,
            "ownedProbeJobs":shell.probe_report(),"jobStatus":self.job_status,"viewportDiagnostics":shell.layout_report()["viewportDiagnostics"],
            "rendererView":self.renderer,"stateTransitions":self.observations})
    }
    pub fn write(&self) -> ara_ui::Result<()> {
        std::fs::write(
            self.output.join("probes-and-debug-views.json"),
            ara_json::to_canonical_string(&json!({
                "completed":self.index==CASES.len() && self.restored,"restored":self.restored,"expected":CASES.len(),
                "passed":self.results.iter().filter(|row|row["ok"]==true).count(),"failed":self.failures().len(),"error":self.error,
                "elapsedSeconds":self.started.elapsed().as_secs_f64(),"nativeEvents":self.native_events,"journeys":self.results,
                "current":{"journey":CASES.get(self.index),"phase":self.phase,"seconds":self.case_started.elapsed().as_secs_f64(),
                    "job":self.job,"jobStatus":self.job_status,"snapshot":self.snapshot,"renderer":self.renderer},
                "scope":{"fixture":"Typed isolated Scene/procedural mesh/light creation, High quality and explicit baseline/navigation setup",
                    "measured":"Actual SDL pointer/keyboard through Window::poll_tagged and production dispatch",
                    "captureObservations":"Read-only render.probes.query/jobs.status; bounded worker observes cache metadata after the real published-image/hash receipt",
                    "viewModes":"Requires caller-supplied current completed renderer provenance; state flags alone never pass"},
                "timingLabel":"informal (shared GPU)","timingAcceptance":false,"boundedWaitSeconds":PATIENCE.as_secs(),
                "rendererMetadataAdapter":{"observed":!self.renderer.is_null(),"contract":"Caller-supplied actual current completed view-mode/overdraw metadata; null is not evidence"},
                "limits":["No proof is established until this leaf runs on its matching VQ build.",
                    "Cancellation and stale-source cases require an actually observed queued/active generation; finishing too early is a failed proof, not a pass.",
                    "Published sourceHash/captureHash are renderer receipts; cache existence and byte count do not independently prove every cubemap texel."]
            })),
        )?;
        Ok(())
    }
}
