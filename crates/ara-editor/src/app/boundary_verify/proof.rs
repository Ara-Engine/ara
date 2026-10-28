use super::*;

fn text<'a>(shell: &'a Shell, id: &str) -> Option<&'a str> {
    shell
        .shown_node(id)
        .and_then(|index| shell.ui.nodes[index].text.as_ref())
        .map(|text| text.value.as_str())
}
fn normal(shell: &Shell, panel: &str) -> bool {
    let id = if panel == "assets" {
        "search-assets"
    } else {
        "entity-name"
    };
    shell.shown_node(id).is_some_and(|index| {
        let rect = shell.ui.nodes[index]
            .rect
            .intersection(shell.ui.nodes[index].clip);
        rect.w > 1. && rect.h > 1.
    })
}

impl BoundaryCheck {
    pub(super) fn capture(
        &mut self,
        case: Case,
        stage: &str,
        editor: &Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
    ) -> ara_ui::Result<()> {
        shell.leave();
        gpu.select_target(0);
        std::fs::create_dir_all(&self.output)?;
        let stem = format!("boundary-{:02}-{}-{stage}", self.index + 1, case.label);
        gpu.pixels()?
            .save(self.output.join(format!("{stem}.png")))?;
        std::fs::write(
            self.output.join(format!("{stem}.layout.json")),
            ara_json::to_canonical_string(&shell.layout_report()),
        )?;
        let failure = shell
            .shown_node(&format!("panel-error-{}", case.panel))
            .map(|index| shell.ui.nodes[index].rect);
        let retry = shell
            .shown_node(&format!("panel-error-{}-retry", case.panel))
            .map(|index| {
                shell.ui.nodes[index]
                    .rect
                    .intersection(shell.ui.nodes[index].clip)
            });
        let menu=shell.menu.as_ref().map(|menu|json!({"source":format!("{:?}",menu.source),"active":menu.active,"surface":menu.surface}));
        self.trace.push(json!({"stage":stage,"revision":editor.host.store().revision(),"records":records(editor)?,"selection":editor.state.selection,"failedPanel":case.panel,"failureCount":shell.panel_failure_count(),"error":failure,"detail":text(shell,&format!("panel-error-{}-detail",case.panel)),"retry":retry,"normalControls":normal(shell,case.panel),"healthyHierarchy":shell.shown_node("hierarchy-rows").is_some(),"hierarchySearch":editor.state.searches.get("hierarchy"),"menu":menu,"focus":shell.input.focused(),"probeArmed":shell.panel_probe_armed()}));
        Ok(())
    }
    pub(super) fn result(
        &self,
        case: Case,
        editor: &Editor,
        shell: &Shell,
    ) -> ara_ui::Result<(bool, Value)> {
        let stage = |name: &str| self.trace.iter().find(|trace| trace["stage"] == name);
        let before = stage("before");
        let failed = stage("error");
        let healthy = stage("healthy-input");
        let menu = stage("menu");
        let recovered = stage("recovered");
        let diagnostic = self.diagnostics.len() == 1
            && self.diagnostics[0]["diagnostic"]["code"] == "ARA4500"
            && self.diagnostics[0]["diagnostic"]["message"]
                .as_str()
                .is_some_and(|message| {
                    message.contains(&self.correlation)
                        && message.to_lowercase().contains(case.panel)
                        && message.contains(if matches!(case.kind, ProbeFailure::Panic) {
                            "panicked"
                        } else {
                            "failed"
                        })
                })
            && self.diagnostics[0]["count"].as_u64().unwrap_or(1) == 1;
        let mounted = before
            .is_some_and(|trace| trace["normalControls"] == true && trace["failureCount"] == 0);
        let inline = failed.is_some_and(|trace| {
            trace["failureCount"] == 1
                && trace["error"].is_object()
                && trace["normalControls"] == false
                && trace["detail"]
                    .as_str()
                    .is_some_and(|text| text.contains(&self.correlation))
                && trace["retry"]["w"].as_f64().is_some_and(|width| width > 1.)
                && trace["retry"]["h"]
                    .as_f64()
                    .is_some_and(|height| height > 1.)
                && trace["probeArmed"] == false
        });
        let continued = healthy.is_some_and(|trace| {
            trace["healthyHierarchy"] == true
                && trace["selection"] == json!([B])
                && trace["hierarchySearch"] == "Boundary recovery B"
                && trace["failureCount"] == 1
        });
        let keyboard_menu = menu.is_some_and(|trace| {
            trace["menu"]["source"] == "Bar(\"Help\")"
                && trace["menu"]["active"].is_number()
                && trace["failureCount"] == 1
        });
        let retried = recovered.is_some_and(|trace| {
            trace["normalControls"] == true
                && trace["failureCount"] == 0
                && trace["error"].is_null()
                && trace["probeArmed"] == false
        });
        let unchanged = editor.host.store().revision() == self.revision
            && records(editor)? == self.before
            && self
                .trace
                .iter()
                .all(|trace| trace["revision"] == self.revision && trace["records"] == self.before);
        let ok = mounted
            && inline
            && diagnostic
            && continued
            && keyboard_menu
            && retried
            && unchanged
            && shell.panel_failure_count() == 0
            && !shell.panel_probe_armed();
        Ok((
            ok,
            json!({"realPanelInitiallyMounted":mounted,"inlineFailureAndReachableRetry":inline,"oneCorrelatedARA4500":diagnostic,"healthyPanelSelectionAndTextInput":continued,"keyboardMenuContinues":keyboard_menu,"nativeRetryRestoresNormalPanel":retried,"authoringUnchanged":unchanged,"recoveredPanel":case.panel,"expectedCaughtPanic":matches!(case.kind,ProbeFailure::Panic)}),
        ))
    }
}
