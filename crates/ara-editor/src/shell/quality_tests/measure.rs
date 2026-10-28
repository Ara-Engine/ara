use super::*;

// Hover discovery starts on the shell's monotonic clock. A whole-editor sweep can
// outlive a fixed synthetic timestamp, which correctly restarts HoverDelay.
fn finish_hover_delay(shell: &mut Shell) {
    let now = u64::try_from(shell.tooltip_clock.elapsed().as_millis())
        .unwrap_or(u64::MAX)
        .saturating_add(ara_ui::popover::SHOW_DELAY_MS + 1);
    shell.advance_tooltip_at(now).unwrap();
}

#[derive(Default)]
pub(super) struct Sweep {
    pub measurements: Vec<Value>,
    pub violations: Vec<String>,
    pub contexts: Vec<String>,
    identity_audits: Vec<Value>,
    measured_tooltips: usize,
    not_hoverable: Vec<Value>,
}

impl Sweep {
    pub fn context(
        &mut self,
        shell: &mut Shell,
        editor: &mut Editor,
        name: &str,
        capture: &mut impl FnMut(&mut Shell, &str),
    ) {
        shell.frame().unwrap();
        self.contexts.push(name.into());
        let identities = ara_ui_test::audit_attached_ids(&shell.ui);
        for finding in &identities {
            self.violations
                .push(format!("{name}: {}: {}", finding.code, finding.message));
        }
        self.identity_audits.push(json!({
            "context":name,"surface":shell.current(),"diagnostics":identities,
        }));
        let elements: Vec<_> = shell
            .ui
            .nodes
            .iter()
            .map(|node| node.element.clone())
            .collect();
        for violation in ara_ui::controls::audit(&elements) {
            self.violations
                .push(format!("{name}: {}: {}", violation.id, violation.reason));
        }
        if shell.panel_failure_count() > 0 {
            self.violations
                .push(format!("{name}: panel boundary caught a failed render"));
        }
        self.overlays(shell, name);
        capture(shell, name);
        let descriptions: Vec<_> = shell
            .ui
            .nodes
            .iter()
            .enumerate()
            .filter(|(i, node)| {
                shell.ui.shown(*i)
                    && node
                        .element
                        .attrs
                        .get("description")
                        .is_some_and(|s| !s.is_empty())
            })
            .map(|(_, node)| node.element.id.clone())
            .collect();
        let mut widest: Option<(f32, String)> = None;
        for id in descriptions {
            let Some(index) = shell.ui.find(&id) else {
                continue;
            };
            shell.ui.scroll_into_view(index).unwrap();
            shell.sync_scroll_views().unwrap();
            shell.frame().unwrap();
            let Some(index) = shell.ui.find(&id) else {
                continue;
            };
            let node = &shell.ui.nodes[index];
            let left = node.rect.x.max(node.clip.x).max(0.);
            let top = node.rect.y.max(node.clip.y).max(0.);
            let right = (node.rect.x + node.rect.w)
                .min(node.clip.x + node.clip.w)
                .min(shell.ui.size[0]);
            let bottom = (node.rect.y + node.rect.h)
                .min(node.clip.y + node.clip.h)
                .min(shell.ui.size[1]);
            if right - left <= 0.5 || bottom - top <= 0.5 {
                self.not_hoverable
                    .push(json!({"context":name,"id":id,"reason":"outside scroll viewport"}));
                continue;
            }
            let dx = ((right - left) * 0.2).min(1.);
            let dy = ((bottom - top) * 0.2).min(1.);
            let mut matched = false;
            for point in [
                [(left + right) * 0.5, (top + bottom) * 0.5],
                [left + dx, top + dy],
                [right - dx, top + dy],
                [left + dx, bottom - dy],
                [right - dx, bottom - dy],
            ] {
                shell.pointer = point;
                shell.hover_tooltip().unwrap();
                if shell.tooltip_target.as_ref().map(|(target, _)| *target)
                    == Some(ara_ui::source::stable_id(&id))
                {
                    matched = true;
                    break;
                }
            }
            if !matched {
                self.not_hoverable.push(json!({"context":name,"id":id,"reason":"description shadowed by a child control"}));
                continue;
            }
            finish_hover_delay(shell);
            shell.frame().unwrap();
            if shell.tooltip_text.is_none() {
                self.violations.push(format!(
                    "{name}: {id}: hovered description did not show its tooltip"
                ));
                continue;
            }
            self.measured_tooltips += 1;
            self.overlays(shell, &format!("{name}/tooltip/{id}"));
            let width = shell
                .ui
                .find("tooltip")
                .map_or(0., |i| shell.ui.nodes[i].rect.w);
            if widest.as_ref().is_none_or(|(old, _)| width > *old) {
                widest = Some((width, id));
            }
        }
        if let Some((_, id)) = widest {
            // Repeat the widest real hover for a readable full-editor evidence image.
            if let Some(index) = shell.ui.find(&id) {
                shell.ui.scroll_into_view(index).unwrap();
                shell.frame().unwrap();
                let node = &shell.ui.nodes[index];
                shell.pointer = [
                    node.rect.x + node.rect.w * 0.5,
                    node.rect.y + node.rect.h * 0.5,
                ];
                shell.hover_tooltip().unwrap();
                finish_hover_delay(shell);
                shell.frame().unwrap();
                capture(shell, &format!("{name}-tooltip"));
            }
        }
        let _ = editor;
    }

    fn overlays(&mut self, shell: &Shell, context: &str) {
        for (index, node) in shell.ui.nodes.iter().enumerate() {
            if !shell.ui.shown(index)
                || !matches!(node.element.tag.as_str(), "popover" | "tooltip" | "popup")
            {
                continue;
            }
            let rect = node.rect;
            if rect.w <= 0. || rect.h <= 0. {
                continue;
            }
            let max = if node.element.id == "palette" {
                640.
            } else {
                ara_ui::popover::MAX_WIDTH
            };
            if rect.w > max + 0.75 {
                self.violations.push(format!(
                    "{context}: {} too wide: {} > {max}",
                    node.element.id, rect.w
                ));
            }
            if rect.x < -0.75
                || rect.y < -0.75
                || rect.x + rect.w > shell.ui.size[0] + 0.75
                || rect.y + rect.h > shell.ui.size[1] + 0.75
            {
                self.violations.push(format!(
                    "{context}: {} escapes viewport: {rect:?}",
                    node.element.id
                ));
            }
            for (child_index, child) in shell.ui.nodes.iter().enumerate() {
                let mut parent = child.element.parent;
                let mut inside = false;
                while let Some(ancestor) = parent {
                    if ancestor == index {
                        inside = true;
                        break;
                    }
                    parent = shell.ui.nodes[ancestor].element.parent;
                }
                if !inside || !shell.ui.shown(child_index) {
                    continue;
                }
                if let Some(text) = &child.text {
                    let layout = shell.ui.tree.layout(child.layout_id).unwrap();
                    let available = layout.size.width
                        - layout.padding.left
                        - layout.padding.right
                        - layout.border.left
                        - layout.border.right;
                    if text.layout.width() > available + 1. {
                        self.violations.push(format!(
                            "{context}: {} text clips horizontally ({} > {available})",
                            child.element.id,
                            text.layout.width()
                        ));
                    }
                    if node.element.id == "tooltip" && text.layout.height() > child.rect.h + 1. {
                        self.violations.push(format!(
                            "{context}: {} tooltip text clips vertically",
                            child.element.id
                        ));
                    }
                    if node.element.id == "tooltip" {
                        let outer = shell.ui.tree.layout(node.layout_id).unwrap();
                        let required = text.layout.width()
                            + outer.padding.left
                            + outer.padding.right
                            + outer.border.left
                            + outer.border.right;
                        if rect.w > required.max(ara_ui::popover::MIN_WIDTH) + 16. {
                            self.violations.push(format!("{context}: tooltip has excessive empty width ({} for {required} content)",rect.w));
                        }
                    }
                }
            }
            self.measurements.push(json!({"context":context,"overlay":node.element.id,"box":rect,"viewport":shell.ui.size,"maxWidth":max}));
        }
    }

    pub fn write(&self, output: &std::path::Path) {
        std::fs::create_dir_all(output).unwrap();
        std::fs::write(output.join("quality-measurements.json"),ara_json::to_canonical_string(&json!({
            "contexts":self.contexts,"identityAudits":self.identity_audits,"tooltipCount":self.measured_tooltips,"notHoverableDescriptions":self.not_hoverable,"overlays":self.measurements,"violations":self.violations,
        }))).unwrap();
    }
}
