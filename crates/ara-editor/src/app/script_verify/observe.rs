//! Small per-case evidence; authoring records and the ECS proof are never conflated.
use super::*;
impl ScriptCheck {
    pub fn end(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
    ) -> ara_ui::Result<()> {
        if self.finished() {
            return Ok(());
        }
        let result = self.end_step(editor, shell, gpu);
        if let Err(error) = &result {
            self.fail(editor, shell, &error.to_string())?;
        }
        result
    }
    fn end_step(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
    ) -> ara_ui::Result<()> {
        if let Some(stage) = self.pending_capture.take() {
            self.capture(shell, gpu, stage)?;
        }
        if let Some(observed) = self.complete.take() {
            self.capture(shell, gpu, "after")?;
            self.results.push(
                json!({"journey":CASES[self.index],"ok":true,"nativeEvents":self.case_events,
                "observed":observed,"after":self.snapshot(editor,shell)}),
            );
            self.index += 1;
            self.phase = 0;
            self.started = Instant::now();
            self.case_events = 0;
            if self.finished() {
                self.pointer = None;
                shell.desktop_pointer = None;
            }
            self.write()?;
        }
        Ok(())
    }
    pub(super) fn snapshot(&self, editor: &Editor, shell: &Shell) -> Value {
        let authored = match crate::world::resolve(editor.host.store(), &self.doc) {
            Ok(view) => view.get(ENTITY).map_or(Value::Null, |entity| {
                json!({"name":entity.record.get("name"),
                "Transform":entity.record.get("Transform"),"Spinner":entity.record.get("Spinner")})
            }),
            Err(problems) => json!({"resolutionError":format!("{problems:?}")}),
        };
        json!({"doc":editor.state.doc,"selection":editor.state.selection,"resource":editor.state.resource,
            "asset":editor.state.asset_path,"world":editor.play_world,"gameRunning":shell.game_running,"gameLive":shell.game_live,
            "modal":shell.modal_active(),"status":shell.status_note,"actor":authored,"revision":editor.host.store().revision(),
            "scriptModuleInventory":shell.script_module_job_report(),"moduleCompileProof":self.proof.as_ref().map(Proof::report),
            "compileProjection":shell.compile_problem_state()})
    }
    fn capture(&self, shell: &Shell, gpu: &mut NativeUi, stage: &str) -> ara_ui::Result<()> {
        gpu.select_target(0);
        let stem = format!("script-{:02}-{}-{stage}", self.index + 1, CASES[self.index]);
        gpu.pixels()?
            .save(self.output.join(format!("{stem}.png")))?;
        std::fs::write(
            self.output.join(format!("{stem}.layout.json")),
            ara_json::to_canonical_string(&shell.layout_report()),
        )?;
        Ok(())
    }
    pub fn write(&self) -> ara_ui::Result<()> {
        let retirement = self
            .results
            .iter()
            .find(|result| result["journey"] == CASES[6])
            .map_or(Value::Null, |result| {
                result["observed"]["problemRetirementEvidence"].clone()
            });
        std::fs::write(
            self.output.join("script-behaviours.json"),
            ara_json::to_canonical_string(&json!({
                "completed":self.index==CASES.len(),"passed":self.results.iter().filter(|result|result["ok"]==true).count(),
                "failed":self.failures().len(),"error":self.error,"nativeEvents":self.native_events,
                "ingress":"SDL_PushEvent -> Window::poll_tagged -> production route for all measured controls; typed source.replace and background ECS observations are separately labeled protocol",
                "fixtures":self.fixture,"journeys":self.results,"nativeTrace":self.trace,
                "runtimeEffect":self.proof.as_ref().map(Proof::report),"currentProblemsCleared":Value::Null,
                "problemRetirementEvidence":retirement,
                "limits":["The current compile projection is recorded. Exact compiler lifecycle metadata and native post-fix Problems verification are required before claiming retirement; module Reload.errors alone is insufficient.",
                    "The offscreen IDE guard is exercised; this journey intentionally does not launch or inspect an external IDE.",
                    "Runtime rates use authoritative ECS tick deltas, not shared-GPU wall-clock frame rates."],
                "current":{"journey":CASES.get(self.index),"phase":self.phase,"phaseSeconds":self.started.elapsed().as_secs_f64()}
            })),
        )?;
        Ok(())
    }
}
