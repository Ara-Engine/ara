//! Exact compiler diagnostic source coordinates and its real expanded Why UI.
use super::*;
impl ScriptCheck {
    pub(super) fn problems(
        &mut self,
        editor: &Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if !self.palette(shell, window, scale, "window.problems", events)? {
            return Ok(());
        }
        let seq = self
            .proof
            .as_ref()
            .ok_or("J10 compiler evidence is missing")?
            .error_sequence()
            .ok_or("J10 compiler diagnostic has no sequence")?;
        let id = format!("problem-{seq}");
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
            6 => {
                let go = format!("{id}-go");
                if self.reveal(shell, window, scale, &go, events)? {
                    self.click(shell, window, scale, &go, events)?;
                    self.next();
                }
            }
            7 => {
                let report = self.proof.as_ref().ok_or("J10 proof missing")?.report();
                let location = &report["compileErrorHistory"]["diagnostic"]["location"];
                let suffix = format!(
                    "Spinner.cs:{}:{}",
                    location["line"].as_u64().unwrap_or(1).max(1),
                    location["column"].as_u64().unwrap_or(1).max(1)
                );
                if !shell
                    .status_note
                    .as_deref()
                    .is_some_and(|note| note.ends_with(&suffix))
                {
                    return Err(format!(
                        "Native compiler Go did not open exact coordinates {suffix}: {:?}",
                        shell.status_note
                    )
                    .into());
                }
                self.ide_location(
                    editor,
                    shell,
                    location["line"].as_u64().unwrap_or(1).max(1),
                    location["column"].as_u64().unwrap_or(1).max(1),
                )?;
                if editor.state.doc != self.doc {
                    return Err("Compiler source link silently switched the authored scene".into());
                }
                self.click(shell, window, scale, &format!("{id}-why"), events)?;
                self.next();
            }
            _ => {
                if !shell.explained_problems.contains(&seq)
                    || shell.shown_node(&format!("{id}-details")).is_none()
                {
                    return Ok(());
                }
                if !self.reveal(shell, window, scale, &format!("{id}-title"), events)? {
                    return Ok(());
                }
                let title = text(shell, &format!("{id}-title"));
                let explanation = text(shell, &format!("{id}-explain"));
                if title.is_empty() || explanation.is_empty() {
                    return Err(
                        "Compiler Why opened without its actual diagnostic explanation".into(),
                    );
                }
                let diagnostic =
                    self.proof.as_ref().ok_or("J10 proof missing")?.report()["compileErrorHistory"]
                        .clone();
                let location = &diagnostic["diagnostic"]["location"];
                let invocation = self.ide_location(
                    editor,
                    shell,
                    location["line"].as_u64().unwrap_or(1).max(1),
                    location["column"].as_u64().unwrap_or(1).max(1),
                )?;
                self.done(json!({"sequence":seq,"sourceStatus":shell.status_note,"whyTitle":title,"whyExplanation":explanation,
                    "diagnostic":diagnostic,"ideRequest":invocation,"externalIDELaunched":false,
                    "ingress":"SDL palette, filter text, source Go and Why pointers; literal captured IDE argv validated under suppression"}));
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
