//! What the host loop does for agents every pass, whether or not the Scene view is on screen:
//! the queued captures.
use stillkit::el::Runtime;

use super::{Studio, capture, device::SceneDevice, gizmo_state};
use crate::studio::viewport::SceneView;
use crate::world::DocumentView;

/// One Scene pass at the device's current size. Failures go to the log.
pub(super) fn render(
    rt: &mut Runtime<Studio>,
    scene_view: &mut SceneView,
    device: &mut SceneDevice,
    view: &DocumentView,
) {
    let result = rt.touch(|s| -> ara_ui::Result<()> {
        scene_view.gizmo_offset = gizmo_state(s).offset;
        scene_view.render(device, &s.editor, view)
    });
    if let Err(error) = result {
        rt.touch(|s| s.editor.report_failure(&*error));
    }
}

/// The next queued capture is served. Runs every pass: a
/// capture of the editor window or its layout completes the same as with the Scene in front.
/// `presented` says the Scene is on screen; a `viewport` capture without it finishes with an
/// error saying so.
pub(super) fn serve(
    rt: &mut Runtime<Studio>,
    scene_view: &mut SceneView,
    device: &mut SceneDevice,
    presented: bool,
) {
    capture::serve(rt, presented.then_some((device, &*scene_view)));
}
