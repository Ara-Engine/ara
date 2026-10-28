//! Click and marquee gestures commit selection only when the native pointer releases.
use super::*;

pub(super) struct Press {
    from: [f32; 2],
    click: Option<String>,
    initial: Vec<String>,
    modifiers: [bool; 3],
    active: bool,
    surface: Option<String>,
}

impl Controller {
    pub(super) fn begin_marquee(&mut self, editor: &Editor, shell: &Shell, click: Option<String>) {
        self.marquee = Some(Press {
            from: shell.pointer,
            click,
            initial: editor.state.selection.clone(),
            modifiers: shell.modifiers,
            active: false,
            surface: shell.current().map(str::to_owned),
        });
    }

    pub(super) fn move_marquee(&mut self, shell: &mut Shell) -> bool {
        let Some(press) = self.marquee.as_mut() else {
            return false;
        };
        if press.surface.as_deref() != shell.current() {
            return true;
        }
        let distance = Vec2::from(shell.pointer).distance(Vec2::from(press.from));
        press.active |= distance >= ara_ui::drag_drop::DRAG_THRESHOLD;
        if press.active {
            let rect = shell.viewport().and_then(|viewport| {
                viewport::selection::marquee_rect(press.from, shell.pointer, viewport)
            });
            shell.set_marquee(rect);
        }
        true
    }

    pub(super) fn finish_marquee(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        scene: &viewport::SceneView,
    ) -> ara_ui::Result<bool> {
        let Some(press) = self.marquee.take() else {
            return Ok(false);
        };
        shell.set_marquee(None);
        if press.surface.as_deref() != shell.current() {
            return Ok(true);
        }
        if press.active {
            let Some(viewport) = shell.viewport() else {
                return Ok(true);
            };
            let Some(rect) = viewport::selection::marquee_rect(press.from, shell.pointer, viewport)
            else {
                return Ok(true);
            };
            let hits = scene.marquee(
                &editor.state.camera,
                viewport,
                rect,
                &shell.outlines.channels,
                &editor.state.hidden,
                &editor.state.locked,
                false,
            );
            let selected = selection_result(press.initial, hits, press.modifiers);
            editor.action("editor.select", json!({"entities":selected}))?;
        } else {
            editor.action("editor.select", json!({"entities":press.click.into_iter().collect::<Vec<_>>(),"toggle":press.modifiers[1],"additive":press.modifiers[0]}))?;
        }
        Ok(true)
    }

    /// Alt-drag uses the same staged preview as a move, then copies and moves in one batch.
    pub(super) fn finish_gizmo(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        let Some(drag) = self.gizmo.take() else {
            return Ok(());
        };
        if !drag.duplicate {
            editor.finish_gesture(false)?;
            return Ok(());
        }
        let Some(gesture) = editor.gesture.take() else {
            return Ok(());
        };
        let poses: Vec<_> = gesture.current.iter().map(|(entity, matrix)| {
            let (scale, rotation, position) = matrix.to_scale_rotation_translation();
            let (x, y, z) = rotation.to_euler(glam::EulerRot::XYZ);
            json!({"entity":entity,"position":position.to_array(),"rotation":[x.to_degrees(),y.to_degrees(),z.to_degrees()],"scale":scale.to_array()})
        }).collect();
        let result = editor.apply(vec![json!({"cmd":"editor.entities.duplicate","args":{"entities":gesture.initial.keys().collect::<Vec<_>>(),"poses":poses}})], "Duplicate and transform", Some(gesture.base));
        editor.end_field_gestures()?;
        let result = result?;
        if let Some(roots) = result["outputs"][0]["roots"].as_array() {
            editor.action("editor.select", json!({"entities":roots}))?;
        }
        Ok(())
    }
}

fn selection_result(
    mut initial: Vec<String>,
    hits: Vec<String>,
    modifiers: [bool; 3],
) -> Vec<String> {
    if modifiers[1] {
        let subtract: BTreeSet<_> = hits.into_iter().collect();
        initial.retain(|id| !subtract.contains(id));
        initial
    } else if modifiers[0] {
        for hit in hits {
            if !initial.contains(&hit) {
                initial.push(hit);
            }
        }
        initial
    } else {
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replace_add_and_subtract_keep_stable_selection_order() {
        let initial = vec!["b".into(), "a".into()];
        let hits = vec!["a".into(), "c".into()];
        assert_eq!(
            selection_result(initial.clone(), hits.clone(), [false; 3]),
            ["a", "c"]
        );
        assert_eq!(
            selection_result(initial.clone(), hits.clone(), [true, false, false]),
            ["b", "a", "c"]
        );
        assert_eq!(selection_result(initial, hits, [false, true, false]), ["b"]);
    }
}
