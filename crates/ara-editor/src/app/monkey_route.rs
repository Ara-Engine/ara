//! Route durable monkey actions through the same native input owners as SDL events.
use super::*;

#[allow(
    clippy::too_many_arguments,
    reason = "borrows the existing native event-loop owners"
)]
pub(super) fn step(
    runner: &mut monkey::Monkey,
    adapter: &mut monkey::native::Adapter,
    dpi: f32,
    window: &Window,
    floats: &mut windows::Floats,
    controller: &mut controller::Controller,
    editor: &mut Editor,
    shell: &mut Shell,
    renderer: &mut Renderer,
    gpu: &NativeUi,
    view: &crate::world::DocumentView,
    scene_view: &viewport::SceneView,
    import_drops: &mut import_drops::ImportDrops,
) -> ara_ui::Result<bool> {
    let setup_at = Instant::now();
    let main = monkey::native::Surface {
        id: None,
        sdl: window.id(),
        points_to_logical: crate::native::points_to_logical(window.raw(), dpi),
        desktop: window.rect()?.map(|v| v as f32),
    };
    let surfaces = floats
        .windows
        .iter()
        .map(|float| {
            let scale = float.window.display_scale().max(0.25) * windows::ui_scale(editor);
            monkey::native::Surface {
                id: Some(float.id.clone()),
                sdl: float.sdl,
                points_to_logical: crate::native::points_to_logical(float.window.raw(), scale),
                desktop: float.rect.map(|v| v as f32),
            }
        })
        .collect::<Vec<_>>();
    let surface_setup = setup_at.elapsed();
    let mut routed_events = 0;
    let result = runner.step_with(editor, shell, |action, editor, shell, clipboard| {
        let result = (|| {
            let Some(input) = adapter.events(action, shell, &main, &surfaces)? else {
                return action.run(editor, shell, clipboard);
            };
            shell.desktop_pointer = input.desktop_pointer;
            for (sdl, event) in input.events {
                // Count only events actually offered to the production owner.
                routed_events += 1;
                // Cleanup owns termination and drains only this run's resources.
                let _ = route_event(
                    sdl,
                    event,
                    window.id(),
                    dpi,
                    window,
                    floats,
                    controller,
                    editor,
                    shell,
                    renderer,
                    gpu,
                    view,
                    scene_view,
                    clipboard,
                    import_drops,
                )?;
            }
            Ok(())
        })();
        // A rejected floating event must not strand the next action's surface.
        shell.leave();
        // Registered Copy and native Paste must use the same isolated buffer.
        let copied = shell
            .take_clipboard()
            .map_or(Ok(()), |text| clipboard.write(&text));
        result.and(copied)
    });
    runner.record_surface_setup(surface_setup);
    runner.record_native_events(routed_events);
    result
}
