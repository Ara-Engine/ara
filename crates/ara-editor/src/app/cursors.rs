//! Resolve the surface under the mouse every frame, including captured dock drags.
use super::*;
use ara_platform::SystemCursor;
use ara_ui::cursor::Cursor;

pub(super) fn update(
    main: &mut Window,
    floats: &mut windows::Floats,
    shell: &mut Shell,
    gizmo: overlay::GizmoState,
) -> ara_ui::Result<()> {
    if shell.input.captured().is_some() || shell.input.scrolling() {
        main.set_cursor(resolve(shell, &gizmo))?;
        return Ok(());
    }
    let global = desktop_pointer(shell);
    for capture in [true, false] {
        for float in &mut floats.windows {
            let owns = if capture {
                shell
                    .floats
                    .iter()
                    .find(|s| s.id == float.id)
                    .is_some_and(|s| s.input.captured().is_some() || s.input.scrolling())
            } else {
                let r = float.window.rect()?;
                global[0] >= r[0] as f32
                    && global[1] >= r[1] as f32
                    && global[0] < (r[0] + r[2]) as f32
                    && global[1] < (r[1] + r[3]) as f32
            };
            if owns && shell.enter(&float.id) {
                let cursor = resolve(shell, &gizmo);
                shell.leave();
                float.window.set_cursor(cursor)?;
                return Ok(());
            }
        }
    }
    main.set_cursor(resolve(shell, &gizmo))?;
    Ok(())
}
fn resolve(shell: &Shell, gizmo: &overlay::GizmoState) -> SystemCursor {
    if shell.tab_dragging() {
        return SystemCursor::Grabbing;
    }
    if shell.dragged_tab().is_some() {
        return SystemCursor::Grab;
    }
    if shell.resizing_column() {
        return SystemCursor::ResizeEw;
    }
    if gizmo.active.is_some()
        || (gizmo.hover.is_some()
            && shell
                .ui
                .hit_test(shell.pointer[0], shell.pointer[1])
                .is_none()
            && shell
                .viewport()
                .is_some_and(|r| r.contains(shell.pointer[0], shell.pointer[1])))
    {
        return SystemCursor::Move;
    }
    match shell.ui.cursor(&shell.input, shell.pointer) {
        Cursor::Arrow => SystemCursor::Arrow,
        Cursor::Text => SystemCursor::Text,
        Cursor::Pointer => SystemCursor::Hand,
        Cursor::ResizeEw => SystemCursor::ResizeEw,
        Cursor::ResizeNs => SystemCursor::ResizeNs,
        Cursor::ResizeNwse => SystemCursor::ResizeNwse,
        Cursor::ResizeNesw => SystemCursor::ResizeNesw,
        Cursor::Move => SystemCursor::Move,
        Cursor::NotAllowed => SystemCursor::NotAllowed,
        Cursor::Grab => SystemCursor::Grab,
        Cursor::Grabbing => SystemCursor::Grabbing,
    }
}
