//! Agents' `capture` jobs (`ara_capture`), served by the studio's window loop: the Scene
//! with numbered entity marks, the editor window as it shows, its layout report, or the
//! whole window with the Scene in it. The GPU readback happens here; encoding and writing
//! finish on a worker (`crate::capture::Work`). `--screenshot` takes the `window` picture
//! through the same code ([`screenshot`]).
use ara_render::Renderer;
use ara_frame::capture::{CaptureRequest, ViewMode};
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
        "window" => {
            let (image, report) = window(rt, scene)?;
            Ok(Work::Window(image, path, report))
        }
        _ => {
            let (renderer, scene) = scene.ok_or_else(not_shown)?;
            let capture = scene.capture(
                renderer,
                &request(
                    editor,
                    renderer,
                    Some(task.params.marks.unwrap_or(100).min(1000) as usize),
                ),
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

/// Why a capture of the Scene cannot be served.
fn not_shown() -> anyhow::Error {
    anyhow::anyhow!(
        "the Scene view is not shown (another page is in front, the window is hidden, or the \
         renderer has not started): show the Scene, or capture `editor` or `layout`"
    )
}

/// The Scene capture the editor takes: its camera at the renderer's size, with `marks`.
fn request(editor: &crate::Editor, renderer: &Renderer, marks: Option<usize>) -> CaptureRequest {
    CaptureRequest {
        camera: editor.state.camera.spec(),
        size: renderer.size(),
        mode: ViewMode::Lit,
        deterministic: true,
        exposure: 0.0015,
        settle_frames: 1,
        tick: 0,
        seed: 7,
        marks,
    }
}

/// The editor window with its Scene in it: the Scene's capture laid into the Scene view's
/// rectangle and the UI composited over it, as the window composites its native layer
/// under the UI. The report is the window's layout.
fn window(
    rt: &mut Runtime<Studio>,
    scene: Option<(&mut Renderer, &SceneView)>,
) -> anyhow::Result<(image::RgbaImage, Value)> {
    use image::imageops::{FilterType, overlay, replace, resize};
    let (renderer, scene) = scene.ok_or_else(not_shown)?;
    let ui = rt.snap()?;
    let report = layout(rt);
    let view = rt
        .tree()
        .into_iter()
        .find(|n| {
            n.key
                .as_ref()
                .is_some_and(|k| k.to_string() == "scene/view")
        })
        .ok_or_else(not_shown)?;
    let capture = scene.capture(renderer, &request(&rt.state().editor, renderer, None))?;
    let (x, y) = (view.rect.left.round() as i64, view.rect.top.round() as i64);
    let (w, h) = (
        view.rect.width().round().max(1.0) as u32,
        view.rect.height().round().max(1.0) as u32,
    );
    let scene_image = if capture.image.dimensions() == (w, h) {
        capture.image
    } else {
        resize(&capture.image, w, h, FilterType::Triangle)
    };
    let mut canvas = image::RgbaImage::from_pixel(ui.width, ui.height, image::Rgba([0, 0, 0, 255]));
    replace(&mut canvas, &scene_image, x, y);
    let ui = image::RgbaImage::from_raw(ui.width, ui.height, ara_kit::capture::rgba(&ui))
        .ok_or_else(|| anyhow::anyhow!("the window snapshot has the wrong size"))?;
    overlay(&mut canvas, &ui, 0, 0);
    Ok((canvas, report))
}

/// `--screenshot`: the `window` picture as `editor.png` (and `editor.layout.json`) in `dir`,
/// written now. The same preparation as the capture service's `window` target, without the
/// project-folder sandbox that gates an agent's output path.
pub(super) fn screenshot(
    rt: &mut Runtime<Studio>,
    scene: Option<(&mut Renderer, &SceneView)>,
    dir: &std::path::Path,
) -> anyhow::Result<std::path::PathBuf> {
    let (image, report) = window(rt, scene)?;
    std::fs::create_dir_all(dir)?;
    let path = dir.join("editor.png");
    let done = Work::Window(image, path.clone(), report).write()?;
    eprintln!("ara-editor: screenshot written: {}", done["path"]);
    Ok(path)
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
