//! Attribute Camera authoring only to changes made by the native Scene router.
use super::*;

/// Unowned view changes retire Pilot before a native event can absorb them.
pub(super) fn update(
    controller: &controller::Controller,
    editor: &mut Editor,
    shell: &mut Shell,
    native_camera_change: bool,
) -> bool {
    match shell.poll_pilot(editor, controller.camera_navigation_active(), native_camera_change) {
        Ok(changed) => changed,
        Err(error) => {
            editor.report_failure(&*error);
            true
        }
    }
}

/// Escape and native capture loss discard the transient move before ordinary routing.
pub(super) fn before_event(
    event: &Event,
    controller: &mut controller::Controller,
    editor: &mut Editor,
    shell: &mut Shell,
) -> bool {
    let mut changed = update(controller, editor, shell, false);
    if matches!(event, Event::FocusLost | Event::Key { scancode: 41, pressed: true, .. }) {
        match shell.cancel_pilot_navigation(editor) {
            Ok(true) => {
                if let Err(error) = controller.cancel_pointer_gestures(editor, shell) {
                    editor.report_failure(&*error);
                }
                changed = true;
            }
            Ok(false) => {}
            Err(error) => {
                editor.report_failure(&*error);
                changed = true;
            }
        }
    }
    changed
}
