use super::super::input_verify::native;
use super::*;
fn click(
    shell: &Shell,
    window: &Window,
    scale: f32,
    id: &str,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<()> {
    let index = shell
        .shown_node(id)
        .ok_or_else(|| format!("History control is not visible: {id}"))?;
    let rect = shell.ui.nodes[index]
        .rect
        .intersection(shell.ui.nodes[index].clip);
    if rect.w <= 1. || rect.h <= 1. {
        return Err(format!("History control has no usable hit box: {id}").into());
    }
    native::motion(
        window,
        [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5],
        scale,
        events,
    )?;
    native::button(window, true, events)?;
    native::button(window, false, events)
}
impl HistoryCheck {
    pub(super) fn input(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        match self.age {
            3 => click(
                shell,
                window,
                scale,
                &format!("history-row-{}", self.txn),
                events,
            )?,
            4 => click(shell, window, scale, "history-copy", events)?,
            8 => click(shell, window, scale, "history-undo", events)?,
            12 => native::shortcut(window, 28, events)?,
            16 => click(shell, window, scale, "search-history", events)?,
            17 => {
                native::shortcut(window, 4, events)?;
                native::text(window, &self.txn, events)?;
            }
            21 => {
                editor.action("editor.layout.reset", json!({}))?;
                shell.rebuild(editor)?;
            }
            _ => {}
        }
        Ok(())
    }
}
