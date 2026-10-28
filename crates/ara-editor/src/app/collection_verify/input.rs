//! Measured actions enter only through raw SDL, including the stationary held edge.
use super::super::input_verify::native;
use super::*;

impl CollectionCheck {
    pub(super) fn input(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        let act = ACTS[self.index];
        if self.phase == 0 {
            native::focus_lost(window, events)?;
            return self.setup(act, editor, shell);
        }
        if self.phase == 1 {
            let Some(rect) = proof::scroll_rect(shell, act) else {
                let index = shell
                    .shown_node("inspector-scroll")
                    .ok_or("Native collection Inspector is not mounted")?;
                let rect = shell.ui.nodes[index]
                    .rect
                    .intersection(shell.ui.nodes[index].clip);
                native::motion(window, proof::centre(rect), scale, events)?;
                native::wheel(window, 8., events)?;
                self.waiting = true;
                return Ok(());
            };
            if proof::offset(shell, act) > 0.1 {
                native::motion(window, proof::centre(rect), scale, events)?;
                native::wheel(window, 8., events)?;
                self.waiting = true;
            } else if proof::row(shell, act, 0, true).is_none() {
                self.waiting = true;
            }
            return Ok(());
        }
        match self.phase {
            2 if act.drag() => {
                let index = shell
                    .collection_item_node(act.field(), "", &act.key(0), true)
                    .ok_or("Native collection source grip is not mounted")?;
                let node = &shell.ui.nodes[index];
                let rect = node.rect.intersection(node.clip);
                if rect.w < 2. || rect.h < 2. {
                    return Err("Native collection source grip is clipped".into());
                }
                native::motion(window, proof::centre(rect), scale, events)?;
                native::button(window, true, events)?;
            }
            2 => {
                let rect = proof::scroll_rect(shell, act)
                    .ok_or("Native editable collection is clipped")?;
                native::motion(window, proof::centre(rect), scale, events)?;
                native::wheel(window, -1., events)?;
            }
            3 if act.drag() => {
                let rect = proof::scroll_rect(shell, act)
                    .ok_or("Native captured collection is clipped")?;
                self.start_offset = proof::offset(shell, act);
                self.edge = [rect.x + rect.w * 0.6, rect.y + rect.h + 8.];
                native::motion(window, self.edge, scale, events)?;
            }
            3 => {
                if proof::row(shell, act, 100, true).is_none() {
                    native::wheel(window, -1., events)?;
                    self.waiting = true;
                }
            }
            4 if act.drag() => {
                // Deliberately inject nothing. The normal per-frame drag adapter
                // must scroll with capture held and a stationary pointer.
                self.held_frames += 1;
                self.hold_motion_events += events
                    .iter()
                    .filter(|(id, event)| {
                        *id == window.id() && matches!(event, Event::MouseMotion { .. })
                    })
                    .count() as u64;
            }
            4 => {
                let index = proof::edit_node(shell, act)
                    .ok_or("Native item-100 value field is not mounted")?;
                let node = &shell.ui.nodes[index];
                let rect = node.rect.intersection(node.clip);
                if rect.w < 8. || rect.h < 8. {
                    return Err("Native item-100 value field is clipped".into());
                }
                native::motion(window, proof::centre(rect), scale, events)?;
                native::button(window, true, events)?;
                native::button(window, false, events)?;
            }
            5 if act.drag() => {
                let row = proof::row(shell, act, 100, true)
                    .ok_or("Native drag destination item100 is no longer visible")?;
                native::motion(
                    window,
                    [row.x + row.w * 0.6, row.y + row.h - 1.],
                    scale,
                    events,
                )?;
            }
            5 => {
                native::shortcut(window, 4, events)?;
                native::text(
                    window,
                    if act == Act::MapEdit { "777" } else { EDIT },
                    events,
                )?;
            }
            6 if act.drag() => native::button(window, false, events)?,
            6 => native::tap(window, 40, events)?,
            8 => native::shortcut(window, 29, events)?,
            10 => native::shortcut(window, 28, events)?,
            _ => {}
        }
        Ok(())
    }
}
