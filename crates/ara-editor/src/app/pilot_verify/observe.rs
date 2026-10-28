//! Before/after captures and bounded evidence; no external window is opened.
use super::*;
impl PilotCheck {
    pub fn end(
        &mut self,
        editor: &Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
        scene_ready: bool,
    ) -> ara_ui::Result<()> {
        if self.finished() {
            return Ok(());
        }
        if self.holding {
            self.held += 1;
        }
        let result = self.end_step(editor, shell, gpu, scene_ready);
        if let Err(error) = &result {
            self.fail(editor, shell, &error.to_string())?;
        }
        result
    }
    fn end_step(
        &mut self,
        editor: &Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
        scene_ready: bool,
    ) -> ara_ui::Result<()> {
        if !scene_ready {
            return Ok(());
        }
        if let Some(stage) = self.pending_capture.take() {
            self.capture(shell, gpu, stage)?;
        }
        if let Some(value) = self.complete.take() {
            self.capture(shell, gpu, "after")?;
            self.results.push(json!({"journey":CASES[self.index],"ok":true,"nativeEvents":self.case_events,"observed":value,"after":self.snapshot(editor,shell)}));
            self.index += 1;
            self.phase = 0;
            self.prepared = false;
            self.started = Instant::now();
            self.case_events = 0;
            self.held = 0;
            self.pending_capture = None;
            if self.finished() {
                self.pointer = None;
                shell.desktop_pointer = None;
            }
            self.write()?;
        }
        Ok(())
    }
    pub(super) fn snapshot(&self, editor: &Editor, shell: &Shell) -> Value {
        json!({"doc":editor.state.doc,"selection":editor.state.selection,"camera":editor.state.camera,
            "revision":editor.host.store().revision(),"pilot":shell.pilot_report(),"status":shell.status_note,
            "records":fixture::records(editor).map_err(|e|e.to_string()),"viewportDiagnostics":shell.layout_report()["viewportDiagnostics"],
            "heldFrames":self.held,"nativeEvents":self.case_events,"sceneReadyRequiredForCaptures":true})
    }
    fn capture(&self, shell: &Shell, gpu: &mut NativeUi, stage: &str) -> ara_ui::Result<()> {
        gpu.select_target(0);
        let stem = format!("pilot-{:02}-{}-{stage}", self.index + 1, CASES[self.index]);
        gpu.pixels()?
            .save(self.output.join(format!("{stem}.png")))?;
        std::fs::write(
            self.output.join(format!("{stem}.layout.json")),
            ara_json::to_canonical_string(&shell.layout_report()),
        )?;
        Ok(())
    }
    pub fn write(&self) -> ara_ui::Result<()> {
        std::fs::write(
            self.output.join("pilot-and-diagnostics.json"),
            ara_json::to_canonical_string(&json!({
                "completed":self.index==CASES.len(),"passed":self.results.iter().filter(|v|v["ok"]==true).count(),
                "failed":self.failures().len(),"error":self.error,"nativeEvents":self.events,
                "fixture":self.fixture,"journeys":self.results,"nativePointerTrace":self.trace,
                "scope":"SDL native UI; protocol fixture creation, agent presence/deletion and unowned navigation probes are explicitly labeled",
                "timingLabel":"informal (shared GPU)","timingAcceptance":false,
                "current":{"journey":CASES.get(self.index),"phase":self.phase,"phaseSeconds":self.started.elapsed().as_secs_f64()}
            })),
        )?;
        Ok(())
    }
}
