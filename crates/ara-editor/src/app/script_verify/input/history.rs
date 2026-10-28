//! Fixed source clears the module failure; its Console history remains reachable.
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
        if !self.palette(shell, window, scale, "window.console", events)? {
            return Ok(());
        }
        let seq = self
            .proof
            .as_ref()
            .and_then(Proof::error_sequence)
            .ok_or("Fixed J10 lost its historical diagnostic sequence")?;
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
                let projection = shell.compile_problem_state();
                let retirement = retirement_evidence(&proof, &projection);
                self.done(json!({"runtimeEffect":proof,"historicalConsoleSequence":seq,"historicalConsoleMessage":message,
                    "historicalConsoleSource":source,"historyExpandedViaNativePointer":true,
                    "ingress":"Protocol source.replace and ECS fixed-tick proof, followed by SDL Console palette/filter/history pointers",
                    "compileProjection":projection,"problemRetirementEvidence":retirement,
                    "currentProblemsCleared":Value::Null,"currentProblemsScope":"The projection is observed separately; exact lifecycle metadata and a native post-fix Problems check are still required before asserting retirement"}));
            }
        }
        Ok(())
    }
}
// Observe exact provenance without turning an absent or partial compiler contract
// into a passing retirement assertion. Console history is proved independently.
fn retirement_evidence(proof: &Value, projection: &Value) -> Value {
    let data = &proof["compileErrorHistory"]["diagnostic"]["data"];
    let compile = &data["compile"];
    let tagged = compile["schema"] == 1
        && data["module"].is_string()
        && compile["producer"].is_string()
        && compile["profile"].is_string();
    let scope = tagged
        .then(|| {
            projection["scopes"].as_array().and_then(|scopes| {
                scopes.iter().find(|scope| {
                    let key = &scope["key"];
                    key["module"] == data["module"]
                        && key["producer"] == compile["producer"]
                        && key["profile"] == compile["profile"]
                        && key["world"] == data["play"]["world"]
                        && key["player"] == data["play"]["player"]
                })
            })
        })
        .flatten();
    json!({"state":if !tagged {"pending-compiler-metadata"} else if scope.is_none() {"pending-matching-projection-scope"} else {"projection-observed-native-problems-check-pending"},
        "diagnosticMetadataTagged":tagged,"matchingProjectionScope":scope,
        "retirementVerified":Value::Null,
        "scope":"Read-only exact module/producer/profile/world/player correlation, not a native Problems retirement pass",
        "requiredNextCheck":"Newer bound attempt with verified current-module-inputs, complete coverage and zero active scoped errors; then native Problems excludes the resolved diagnostic while native Console retains its exact history"})
}
fn text(shell: &Shell, id: &str) -> String {
    shell
        .ui
        .find(id)
        .and_then(|index| shell.ui.nodes[index].text.as_ref())
        .map_or(String::new(), |text| text.value.clone())
}
