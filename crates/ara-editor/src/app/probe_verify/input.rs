//! All measured controls travel through SDL_PushEvent and Window::poll_tagged.
mod pointer;
use super::*;
impl ProbeCheck {
    pub(super) fn input(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
        scene_ready: bool,
    ) -> ara_ui::Result<()> {
        if self.case_started.elapsed() >= PATIENCE {
            return Err(format!(
                "Probe proof exceeded {} seconds at case {} phase {}; last row {:?}, job {}",
                PATIENCE.as_secs(),
                self.index,
                self.phase,
                self.row().map(|row| (row.generation, row.state, row.stage)),
                self.job_status
            )
            .into());
        }
        if self.index == CASES.len() {
            native::focus_lost(window, events)?;
            return self.restore(editor, shell);
        }
        if self.phase == 0 && !scene_ready {
            return Ok(());
        }
        self.query(editor, shell)?;
        if self.pending_capture.is_some() || self.complete.is_some() {
            return Ok(());
        }
        if self.phase == 0 {
            if !scene_ready {
                return Ok(());
            }
            native::focus_lost(window, events)?;
            return self.prepare(editor, shell);
        }
        if self.phase >= 100 {
            return self.history(editor, shell, window, scale, events);
        }
        match self.index {
            0 => self.create_probe(editor, shell, window, scale, events),
            1 => self.resolution(editor, shell, window, scale, events),
            2 => {
                if !scene_ready {
                    return Ok(());
                }
                let report = shell.layout_report()["viewportDiagnostics"].clone();
                if report["nativeSourceCurrent"] != true {
                    return Ok(());
                }
                if report["worldSegments"] != 40
                    || report["projectedSegments"].as_u64().is_none_or(|n| n < 36)
                {
                    return Err(
                        format!("Distinct probe cages are not all projected: {report}").into(),
                    );
                }
                let now = fixture::snapshot(editor)?;
                proof::unchanged(&self.before, &now)?;
                self.complete = Some(
                    json!({"selectedProbe":self.probe,"viewportDiagnostics":report,
                    "worldWireMeaning":{"influenceEdges":12,"dashedProjectionSegments":24,"captureMarkerAxes":3,"captureOriginConnector":1},
                    "authoredProbe":proof::probe(editor,self.probe.as_deref().ok_or("Probe missing")?)?}),
                );
                Ok(())
            }
            3..=5 => self.capture_input(editor, shell, window, scale, events),
            6..=7 => self.debug_view(editor, shell, window, scale, events, scene_ready),
            _ => Err("Unexpected native probe case".into()),
        }
    }
    fn create_probe(
        &mut self,
        editor: &Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if self.phase <= 3 {
            return self.palette(shell, window, scale, "create.probe", events);
        }
        let records = fixture::records(editor)?;
        let found: Vec<_> = records
            .as_object()
            .ok_or("Fixture records not an object")?
            .iter()
            .filter(|(_, record)| record.get("ReflectionProbe").is_some())
            .map(|(id, _)| id.clone())
            .collect();
        if found.is_empty() {
            return Ok(());
        }
        if found.len() != 1 {
            return Err("Native Create did not produce exactly one ReflectionProbe".into());
        }
        self.probe = found.first().cloned();
        if editor.state.selection != found {
            return Err("Native Create did not select the authored probe".into());
        }
        self.begin_history(
            editor,
            json!({"createdEntity":self.probe,"ingress":"SDL palette row create.probe"}),
        )
    }
    fn resolution(
        &mut self,
        editor: &Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        let id = "enum-ReflectionProbe-captureResolution";
        match self.phase {
            1 => {
                if !self.reveal(shell, id, "inspector-scroll", window, scale, events)? {
                    return Ok(());
                }
                let node = shell.ui.find(id).ok_or("Probe resolution Select missing")?;
                if shell.ui.nodes[node].element.tag != "dropdown" {
                    return Err("Probe resolution is not the shared native Select".into());
                }
                if !self.click(shell, id, window, scale, events)? {
                    return Ok(());
                }
                self.phase = 2;
            }
            2 => {
                native::tap(window, 74, events)?;
                native::tap(window, 81, events)?;
                native::tap(window, 40, events)?;
                self.phase = 3;
            }
            _ => {
                let probe = proof::probe(editor, self.probe.as_deref().ok_or("Probe missing")?)?;
                if probe["captureResolution"] == 64 {
                    return Ok(());
                }
                if probe["captureResolution"] != 128 || !probe["captureResolution"].is_number() {
                    return Err("Native numeric Select did not author numeric128".into());
                }
                self.begin_history(editor,json!({"control":id,"tag":"dropdown","before":64,"after":probe["captureResolution"],"valueIsNumber":true}))?;
            }
        }
        Ok(())
    }
    fn begin_history(&mut self, editor: &Editor, effect: Value) -> ara_ui::Result<()> {
        self.committed = fixture::snapshot(editor)?;
        proof::one_transaction(&self.before, &self.committed)?;
        self.observations["nativeMutation"] = effect;
        self.pending_capture = Some("committed");
        self.phase = 100;
        Ok(())
    }
    fn history(
        &mut self,
        editor: &Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        match self.phase {
            100 => {
                self.blur_scene(shell, window, scale, events)?;
                native::shortcut(window, 29, events)?;
                self.phase = 101;
            }
            101 => {
                let now = fixture::snapshot(editor)?;
                if now["records"] != self.before["records"] {
                    return Ok(());
                }
                proof::revision(&self.before, &now, 2)?;
                self.observations["undo"] = now;
                self.pending_capture = Some("undo");
                self.phase = 102;
            }
            102 => {
                native::shortcut(window, 28, events)?;
                self.phase = 103;
            }
            _ => {
                let now = fixture::snapshot(editor)?;
                if now["records"] != self.committed["records"] {
                    return Ok(());
                }
                proof::revision(&self.before, &now, 3)?;
                self.observations["redo"] = now;
                self.complete = Some(
                    json!({"oneMutationOneUndoRedo":true,"committed":self.committed,"history":self.observations}),
                );
            }
        }
        Ok(())
    }
    fn debug_view(
        &mut self,
        editor: &Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
        scene_ready: bool,
    ) -> ara_ui::Result<()> {
        let (command, mode) = if self.index == 6 {
            ("view.mode.gi_debug", "gi_diffuse")
        } else {
            ("view.mode.overdraw", "overdraw")
        };
        if self.phase <= 3 {
            return self.palette(shell, window, scale, command, events);
        }
        if !scene_ready || editor.state.view_mode != mode {
            return Ok(());
        }
        let evidence = proof::debug_view(&self.renderer, mode)?;
        if evidence.is_null() {
            return Ok(());
        }
        proof::unchanged(&self.before, &fixture::snapshot(editor)?)?;
        self.complete = Some(
            json!({"nativeCommand":command,"requestedViewMode":editor.state.view_mode,"completedRenderer":evidence,"authoringUnchanged":true}),
        );
        Ok(())
    }
}
