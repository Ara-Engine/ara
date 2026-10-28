//! Exact scoped retirement in Problems, followed by every retained Console row.
use super::*;
impl ScriptCheck {
    pub(super) fn history(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if !self.fixed_runtime {
            self.lifecycle.capture(
                editor,
                shell,
                self.source.as_deref().ok_or("J10 source missing")?,
            )?;
            if self
                .proof
                .as_mut()
                .ok_or("J10 runtime proof missing")?
                .run(Stage::Fixed, editor)?
            {
                self.fixed_runtime = true;
                self.phase = 1;
                self.started = Instant::now();
            }
            return Ok(());
        }
        let projection = shell.compile_problem_state();
        if !self.lifecycle.assess(&projection) {
            return Ok(());
        }
        if !self.lifecycle.console_active {
            return self.fixed_problems(editor, shell, window, scale, events);
        }
        if !self.palette(shell, window, scale, "window.console", events)? {
            return Ok(());
        }
        let seq = self
            .lifecycle
            .sequence()
            .ok_or("Fixed J10 lost a scoped historical diagnostic sequence")?;
        let id = format!("log-{seq}");
        match self.phase {
            4 => {
                let Some(index) = shell.shown_node("console-errors") else {
                    return Ok(());
                };
                if !shell.ui.nodes[index]
                    .element
                    .attrs
                    .get("class")
                    .is_some_and(|class| class.contains("segment-on"))
                {
                    self.click(shell, window, scale, "console-errors", events)?;
                }
                self.next();
            }
            5 => {
                let Some(index) = shell.shown_node("console-collapse") else {
                    return Ok(());
                };
                if shell.ui.nodes[index]
                    .element
                    .attrs
                    .get("class")
                    .is_some_and(|class| class.split_whitespace().any(|class| class == "selected"))
                {
                    self.click(shell, window, scale, "console-collapse", events)?;
                }
                self.next();
            }
            6 => {
                self.click(shell, window, scale, "search-console", events)?;
                native::shortcut(window, 4, events)?;
                native::text(window, "ARA_J10_INVALID_SOURCE", events)?;
                self.next();
            }
            7 => {
                let row = format!("{id}-row");
                if self.reveal(shell, window, scale, &row, events)? {
                    self.click(shell, window, scale, &row, events)?;
                    self.next();
                }
            }
            _ => {
                if shell.shown_node(&format!("{id}-details")).is_none() {
                    return Ok(());
                }
                let message = text(shell, &format!("{id}-text"));
                let source = text(shell, &format!("{id}-source-label"));
                if !message.contains("ARA_J10_INVALID_SOURCE") || !source.contains("Spinner.cs:") {
                    return Err("Native Console did not retain the exact historical compiler diagnostic and source link".into());
                }
                let proof = self.proof.as_ref().ok_or("J10 proof missing")?.report();
                if proof["fixedSourceProved"] != true
                    || proof["currentCompileErrorCount"] != 0
                    || proof["historicalErrorRetained"] != true
                {
                    return Err("Fixed J10 lacks successful module state, runtime effect, or historical diagnostic evidence".into());
                }
                self.lifecycle.record_console(message, source)?;
                if self.lifecycle.sequence().is_some() {
                    self.phase = 7;
                    self.started = Instant::now();
                    return Ok(());
                }
                if !self.lifecycle.refresh_history(editor, &projection)? {
                    self.phase = 1;
                    self.started = Instant::now();
                    return Ok(());
                }
                if !self.lifecycle.complete() {
                    return Err(
                        "J10 native history is visible without complete scoped Problems retirement"
                            .into(),
                    );
                }
                self.done(json!({"runtimeEffect":proof,"historyExpandedViaNativePointer":true,
                    "ingress":"Protocol source.replace and ECS fixed-tick proof, then SDL Problems and Console palette/filter/history pointers",
                    "compileProjection":projection,"problemRetirementEvidence":self.lifecycle.report(),
                    "currentProblemsCleared":true,"currentProblemsScope":"Only the captured exact Spinner compiler scopes; unrelated and runtime errors are preserved independently"}));
            }
        }
        Ok(())
    }

    fn fixed_problems(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if !self.palette(shell, window, scale, "window.problems", events)? {
            return Ok(());
        }
        match self.phase {
            4 => {
                let Some(index) = shell.shown_node("problems-errors") else {
                    return Ok(());
                };
                if !shell.ui.nodes[index]
                    .element
                    .attrs
                    .get("class")
                    .is_some_and(|class| class.contains("segment-on"))
                {
                    self.click(shell, window, scale, "problems-errors", events)?;
                }
                self.next();
            }
            5 => {
                self.click(shell, window, scale, "search-problems", events)?;
                native::shortcut(window, 4, events)?;
                native::text(window, "ARA_J10_INVALID_SOURCE", events)?;
                self.next();
            }
            _ => {
                let Some(errors) = shell.shown_node("problems-errors") else {
                    return Ok(());
                };
                let query = match shell
                    .ui
                    .widgets
                    .get(&ara_ui::source::stable_id("search-problems"))
                {
                    Some(ara_ui::widgets::Widget::Input(text)) => Some(text.value()),
                    _ => None,
                };
                if !shell.ui.nodes[errors]
                    .element
                    .attrs
                    .get("class")
                    .is_some_and(|class| class.contains("segment-on"))
                    || query != Some("ARA_J10_INVALID_SOURCE")
                {
                    return Ok(());
                }
                let projection = shell.compile_problem_state();
                if !self.lifecycle.refresh_history(editor, &projection)? {
                    return Ok(());
                }
                if self
                    .lifecycle
                    .absent_ids()
                    .iter()
                    .any(|seq| shell.ui.find(&format!("problem-{seq}")).is_some())
                {
                    return Ok(());
                }
                self.lifecycle.mark_problems(&projection);
                self.pending_capture = Some("resolved-problems");
                self.lifecycle.console_active = true;
                self.phase = 1;
                self.started = Instant::now();
            }
        }
        Ok(())
    }
}
fn text(shell: &Shell, id: &str) -> String {
    shell
        .ui
        .find(id)
        .and_then(|index| shell.ui.nodes[index].text.as_ref())
        .map_or(String::new(), |text| text.value.clone())
}
