//! Agents' `capture` jobs (`ara_capture`), served by the studio's window loop: the Scene
//! with numbered entity marks, the editor window as it shows, or its layout report. The
//! GPU readback happens here; encoding and writing finish on a worker
//! (`crate::capture::Work`).
use ara_render::Renderer;
use ara_render::capture::{CaptureRequest, ViewMode};
use serde_json::{Value, json};
use stillkit::el::Runtime;

use super::Studio;
use crate::capture::{CaptureTask, Work, output_path};
use crate::studio::viewport::SceneView;
use crate::world::EditWorldView as _;

/// Serves the next queued capture job, if there is one. `scene` is the renderer and the
/// Scene view it presents, for viewport captures; none while the Scene is not shown, which
/// fails a `viewport` capture with a message saying so.
pub(super) fn serve(rt: &mut Runtime<Studio>, scene: Option<(&mut Renderer, &SceneView)>) {
    let Ok(task) = rt.state().editor.captures.try_recv() else {
        return;
    };
    let prepared = prepare(&task, rt, scene);
    let completed = rt.state().editor.completed.clone();
    Work::finish(prepared, task.job, completed);
}

/// What `task` asks for, read from the window and the presented Scene.
fn prepare(
    task: &CaptureTask,
    rt: &mut Runtime<Studio>,
    scene: Option<(&mut Renderer, &SceneView)>,
) -> anyhow::Result<Work> {
    let editor = &rt.state().editor;
    let target = task.params.target.as_deref().unwrap_or("viewport");
    let relative = task
        .params
        .path
        .clone()
        .unwrap_or_else(|| format!(".ara/captures/{}.png", task.job));
    let path = output_path(editor.host.root(), &relative)?;
    match target {
        "layout" => {
            let report = json!({"layout": editor.state.layout, "ui": layout(rt)});
            Ok(Work::Layout(path.with_extension("json"), report))
        }
        "editor" => {
            let report = layout(rt);
            let png = ara_kit::capture::png(&rt.snap()?)?;
            Ok(Work::Editor(png, path, report))
        }
        _ => {
            let (renderer, scene) = scene.ok_or_else(|| {
                anyhow::anyhow!(
                    "the Scene view is not shown (another page is in front, the window is \
                     hidden, or the renderer has not started): show the Scene, or capture \
                     `editor` or `layout`"
                )
            })?;
            let capture = scene.capture(
                renderer,
                &CaptureRequest {
                    camera: editor.state.camera.spec(),
                    size: renderer.size(),
                    mode: ViewMode::Lit,
                    deterministic: true,
                    exposure: 0.0015,
                    settle_frames: 1,
                    tick: 0,
                    seed: 7,
                    marks: Some(task.params.marks.unwrap_or(100).min(1000) as usize),
                },
            )?;
            let entities: Vec<Value> = editor
                .view()?
                .entities()
                .keys()
                .map(|e| json!({"entity": e, "renderEntity": crate::world::render_id(e)}))
                .collect();
            Ok(Work::Viewport(Box::new(capture), path, json!(entities)))
        }
    }
}

/// The window's keyed elements as shown: where each is (device px), what it says and
/// whether it has focus; enough for an agent to find a control on a capture.
fn layout(rt: &Runtime<Studio>) -> Value {
    let shown: Vec<Value> = rt
        .tree()
        .into_iter()
        .filter_map(|n| {
            let key = n.key?;
            Some(json!({
                "key": key.to_string(),
                "label": n.label.as_deref(),
                "text": n.text.as_deref(),
                "focused": n.focused,
                "rect": [n.rect.left, n.rect.top, n.rect.width(), n.rect.height()],
            }))
        })
        .collect();
    json!({"scale": rt.scale(), "elements": shown})
}
