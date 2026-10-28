//! Dedicated story controls use SDL injection, drained through Window::poll_tagged.
use super::super::super::super::input_verify::native;
use super::*;

pub(super) fn click(
    shell: &Shell,
    window: &Window,
    id: &str,
    scale: f32,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<()> {
    let point = control(shell, id)
        .ok_or_else(|| format!("Dedicated Play control is not reachable: {id}"))?;
    native::motion(window, point, scale, events)?;
    native::button(window, true, events)?;
    native::button(window, false, events)
}
pub(super) fn select(
    shell: &Shell,
    window: &Window,
    id: &str,
    index: usize,
    scale: f32,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<()> {
    click(shell, window, id, scale, events)?;
    native::tap(window, 74, events)?;
    for _ in 0..index {
        native::tap(window, 81, events)?;
    }
    native::tap(window, 40, events)
}
pub(super) fn replace(
    shell: &Shell,
    window: &Window,
    id: &str,
    text: &str,
    scale: f32,
    events: &mut Vec<(u32, Event)>,
) -> ara_ui::Result<()> {
    click(shell, window, id, scale, events)?;
    native::shortcut(window, 4, events)?;
    native::text(window, text, events)?;
    native::tap(window, 40, events)
}
impl PlayCheck {
    pub(in super::super) fn dedicated_visible(
        &mut self,
        shell: &Shell,
        window: &Window,
        id: &str,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<bool> {
        if control(shell, id).is_some() {
            return Ok(true);
        }
        if self
            .waits
            .record(self.age, self.began.elapsed(), || waits::control(shell, id))
        {
            self.write()?;
        }
        if let (Some(node), Some(scroll)) = (shell.shown_node(id), shell.shown_node("play-options"))
        {
            let rect = shell.ui.nodes[scroll]
                .rect
                .intersection(shell.ui.nodes[scroll].clip);
            if rect.w > 1. && rect.h > 1. {
                native::motion(
                    window,
                    [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5],
                    scale,
                    events,
                )?;
                native::wheel(
                    window,
                    if shell.ui.nodes[node].rect.y < rect.y {
                        4.
                    } else {
                        -4.
                    },
                    events,
                )?;
            }
        }
        self.wait()?;
        Ok(false)
    }
}
