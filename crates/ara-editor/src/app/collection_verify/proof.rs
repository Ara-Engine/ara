//! Exact state checks and compact, reproducible native evidence.
use super::*;
use crate::shell::Binding;
use ara_ui::{
    paint::{Box2, Draw},
    source::stable_id,
};

fn scroll_id(act: Act) -> String {
    format!(
        "collection-{:016x}",
        stable_id(&format!("{}#", act.field()))
    )
}
pub(super) fn centre(rect: Box2) -> [f32; 2] {
    [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5]
}
pub(super) fn scroll_rect(shell: &Shell, act: Act) -> Option<Box2> {
    let index = shell.shown_node(&scroll_id(act))?;
    let node = &shell.ui.nodes[index];
    let rect = node.rect.intersection(node.clip);
    (rect.w > 100. && rect.h >= node.rect.h - 1. && rect.h >= 48.).then_some(rect)
}
pub(super) fn offset(shell: &Shell, act: Act) -> f32 {
    shell
        .ui
        .find(&scroll_id(act))
        .and_then(|index| shell.ui.scroll_position(index))
        .map_or(0., |position| position[1])
}
pub(super) fn row(shell: &Shell, act: Act, index: usize, full: bool) -> Option<Box2> {
    let index = shell.collection_item_node(act.field(), "", &act.key(index), false)?;
    let node = &shell.ui.nodes[index];
    let visible = node.rect.intersection(node.clip);
    (visible.w > 40. && visible.h >= if full { node.rect.h - 1. } else { 8. }).then_some(node.rect)
}
pub(super) fn pool(shell: &Shell, act: Act) -> usize {
    shell.ui.find(&scroll_id(act)).map_or(0, |index| {
        shell.ui.nodes[index]
            .element
            .children
            .iter()
            .filter(|child| {
                shell.ui.nodes[**child]
                    .element
                    .attrs
                    .get("class")
                    .is_some_and(|class| class.split_whitespace().any(|class| class == "prop-row"))
            })
            .count()
    })
}
pub(super) fn edit_node(shell: &Shell, act: Act) -> Option<usize> {
    let pointer = if act == Act::MapEdit { "/k100" } else { "/100" };
    let target = format!("{}#{pointer}", act.field());
    shell.ui.paint_order.iter().copied().find(|index| {
        let node = &shell.ui.nodes[*index];
        node.element.id.starts_with("nest-edit-") && shell.ui.shown(*index)
            && matches!(shell.bindings.get(&stable_id(&node.element.id)),Some(Binding::Nested(path,..)) if path==&target)
    })
}
fn require(condition: bool, message: &str) -> ara_ui::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(message.to_owned().into())
    }
}

impl CollectionCheck {
    pub(super) fn checkpoint(
        &mut self,
        act: Act,
        editor: &Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
    ) -> ara_ui::Result<()> {
        if self.phase <= 5 {
            require(
                editor.host.store().revision() == self.revision,
                "Collection preview changed authoring revision",
            )?;
            require(
                fixture::records(editor)? == self.before,
                "Collection preview changed a fixture record",
            )?;
        }
        match self.phase {
            1 => {
                require(
                    row(shell, act, 100, true).is_none(),
                    "Item100 must begin outside the bounded visible row window",
                )?;
                require(
                    pool(shell, act) > 0,
                    "Collection proof mounted no collection rows",
                )?;
                self.capture("before", editor, shell, gpu)?;
            }
            3 if act.drag() => {
                require(
                    shell.content_dragging(),
                    "Native collection grip did not start shared drag capture",
                )?;
                self.capture("edge-preview", editor, shell, gpu)?;
            }
            4 if act.drag() => {
                require(
                    shell.content_dragging(),
                    "Collection capture was lost while rows recycled",
                )?;
                require(
                    shell
                        .pointer
                        .iter()
                        .zip(self.edge)
                        .all(|(actual, expected)| (*actual - expected).abs() < 0.01),
                    "Held collection edge pointer moved without native motion",
                )?;
                if !self.edge_captured && offset(shell, act) > self.start_offset + 100. {
                    self.capture("stationary-edge-scroll", editor, shell, gpu)?;
                    self.edge_captured = true;
                }
                if row(shell, act, 100, true).is_none() {
                    self.waiting = true;
                } else {
                    require(
                        self.held_frames >= 2,
                        "Collection item100 was reached without sustained held-edge scrolling",
                    )?;
                    require(
                        self.hold_motion_events == 0,
                        "Stationary collection edge proof received additional native pointer motion",
                    )?;
                    require(
                        offset(shell, act) > self.start_offset + 2000.,
                        "Collection item100 was not reached through the real scroll offset",
                    )?;
                    self.capture("scrolled-item100", editor, shell, gpu)?;
                }
            }
            5 if act.drag() => {
                let report = shell.content_drag_report();
                require(
                    report["feedback"]["valid"] == true
                        && report["feedback"]["target"]["kind"] == "collection"
                        && report["feedback"]["target"]["after"] == true,
                    "Item100 must have an accepted after-row drop indicator",
                )?;
                let target =
                    row(shell, act, 100, true).ok_or("Collection target clipped before release")?;
                let indicator = &report["feedback"]["rect"];
                require(
                    indicator["y"]
                        .as_f64()
                        .is_some_and(|y| (y - f64::from(target.y + target.h - 1.)).abs() <= 2.)
                        && indicator["h"]
                            .as_f64()
                            .is_some_and(|h| (0. ..=3.).contains(&h)),
                    "Collection insertion indicator does not match the visible destination row",
                )?;
                require(
                    shell.shown_node("drop-label").is_some(),
                    "Collection drag ghost is not visible",
                )?;
                require(shell.ui.display.commands.iter().any(|draw| matches!(draw,Draw::Rect{node,..} if *node==stable_id("content-drop-preview"))),"Collection insertion indicator is absent from native display output")?;
                self.capture("accepted-item100-preview", editor, shell, gpu)?;
            }
            7 => {
                require(
                    fixture::records(editor)? == self.expected,
                    "Collection commit did not preserve the exact per-object list/map/keyed values",
                )?;
                require(
                    editor.host.store().revision() == self.revision + 1,
                    "Collection gesture did not create exactly one transaction",
                )?;
                require(
                    !shell.content_pending(),
                    "Collection release left drag capture active",
                )?;
                self.capture("after", editor, shell, gpu)?;
            }
            9 => {
                require(
                    fixture::records(editor)? == self.before,
                    "One native Undo did not restore every selected object's exact collection values",
                )?;
                require(
                    editor.host.store().revision() == self.revision + 2,
                    "Native collection Undo did not create exactly one inverse transaction",
                )?;
                self.capture("undo", editor, shell, gpu)?;
            }
            11 => {
                require(
                    fixture::records(editor)? == self.expected,
                    "One native Redo did not restore the exact collection gesture",
                )?;
                require(
                    editor.host.store().revision() == self.revision + 3,
                    "Native collection Redo did not create exactly one transaction",
                )?;
                self.capture("redo", editor, shell, gpu)?;
            }
            _ => {}
        }
        Ok(())
    }
    pub(super) fn capture(
        &mut self,
        stage: &str,
        editor: &Editor,
        shell: &Shell,
        gpu: &mut NativeUi,
    ) -> ara_ui::Result<()> {
        std::fs::create_dir_all(&self.output)?;
        let act = ACTS[self.index];
        let prefix = format!("collection-{:02}-{}-{stage}", self.index + 1, act.label());
        gpu.pixels()?
            .save(self.output.join(format!("{prefix}.png")))?;
        std::fs::write(
            self.output.join(format!("{prefix}.layout.json")),
            ara_json::to_canonical_string(&shell.layout_report()),
        )?;
        self.trace.push(json!({"stage":stage,"prefix":prefix,"phase":self.phase,"revision":editor.host.store().revision(),"selection":editor.state.selection,
            "records":fixture::signature(&fixture::records(editor)?),"field":act.field(),"scrollViewport":scroll_rect(shell,act),"scrollOffset":offset(shell,act),
            "mountedRows":pool(shell,act),"maxMountedRows":self.max_rows,"heldFramesWithoutMotion":self.held_frames,"heldMotionEvents":self.hold_motion_events,
            "pointer":shell.pointer,"drag":shell.content_drag_report(),"nativeEvents":self.native_events,"pointsToLogical":self.input_scale,"logicalExtent":shell.ui.size}));
        Ok(())
    }
}
