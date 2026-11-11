//! What the host loop does for agents and the Lighting page every pass, whether or not the
//! Scene view is on screen: Scene renders a probe bake needs (a bake advances only through
//! them), the probe owner's poll, and queued captures.
use ara_render::Renderer;
use stillkit::el::Runtime;

use super::{Studio, capture, gizmo_state, probes};
#[cfg(any(windows, target_os = "linux"))]
use crate::Editor;
use crate::studio::viewport::SceneView;
use crate::world::DocumentView;

/// Whether the loop must render the Scene with nothing presented: a probe bake or its
/// retirement is under way and the Scene has rendered before (`shown_extent` is its size).
/// A Scene that never rendered has nothing a bake could read, so nothing is waiting on it.
pub(super) fn needs_unpresented_render(
    probe_owner: &probes::Owner,
    shown_extent: [u32; 2],
) -> bool {
    probe_owner.busy() && shown_extent != [0, 0]
}

/// One Scene render at the renderer's current size. A probe bake reading the Scene holds it
/// until its work retires (`probes::Owner::prepare_scene_change`), so a change that would
/// replace what it reads waits. Failures go to the log.
pub(super) fn render(
    rt: &mut Runtime<Studio>,
    probe_owner: &mut probes::Owner,
    scene_view: &mut SceneView,
    renderer: &mut Renderer,
    view: &DocumentView,
) {
    let result = rt.touch(|s| -> ara_ui::Result<()> {
        let mut safe = probe_owner.prepare_scene_change(&mut s.editor, scene_view, renderer)?;
        if safe
            && probe_owner.busy()
            && scene_view.probe_replacement_required(renderer, &s.editor)?
        {
            safe = probe_owner.prepare_replacement(&mut s.editor);
        }
        if !safe {
            return Ok(());
        }
        // Frames carry the drag pose they show (`SceneView::shown_frame`).
        scene_view.gizmo_offset = gizmo_state(s).offset;
        scene_view.render(renderer, &s.editor, view, 1)
    });
    if let Err(error) = result {
        rt.touch(|s| s.editor.report_failure(&*error));
    }
}

/// Probe jobs advance and the next queued capture is served. Runs every pass: a recapture
/// started from the Lighting page (the Scene hidden behind it) and a capture of the editor
/// window or its layout complete the same as with the Scene in front. `presented` says the
/// Scene is on screen; a `viewport` capture without it finishes with an error saying so.
/// Returns whether the Scene needs another frame.
pub(super) fn serve(
    rt: &mut Runtime<Studio>,
    probe_owner: &mut probes::Owner,
    scene_view: &mut SceneView,
    renderer: &mut Renderer,
    presented: bool,
) -> bool {
    let mut scene_dirty = false;
    match rt.touch(|s| probe_owner.poll(&mut s.editor, scene_view, renderer)) {
        Ok(probe) => {
            scene_dirty = probe.scene_dirty;
            if probe.changed {
                rt.update(|_| {});
            }
        }
        Err(error) => rt.touch(|s| s.editor.report_failure(&*error)),
    }
    capture::serve(rt, presented.then_some((renderer, &*scene_view)));
    scene_dirty
}

/// Cancellations reach a running bake even while the loop is not rendering (the Windows
/// gate that waits for the Scene's GPU reads to finish): requests are pumped and the owner
/// told. This submits no GPU work.
#[cfg(any(windows, target_os = "linux"))]
pub(super) fn deliver_cancellations(
    editor: &mut Editor,
    probe_owner: &mut probes::Owner,
    scene_view: &mut SceneView,
) {
    editor.tick();
    probe_owner.deliver_cancellations(editor, scene_view);
}
