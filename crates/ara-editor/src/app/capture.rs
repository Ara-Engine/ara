//! GPU readback stays on the window thread; encoding and disk IO finish in a worker job.
use super::*;
use crate::capture::{Work, output_path};
pub(super) fn prepare(
    task: &crate::capture::CaptureTask,
    renderer: &mut Renderer,
    gpu: &mut NativeUi,
    shell: &Shell,
    editor: &Editor,
    scene_view: &viewport::SceneView,
) -> anyhow::Result<Work> {
    let target = task.params.target.as_deref().unwrap_or("viewport");
    let relative = task
        .params
        .path
        .as_deref()
        .map(str::to_owned)
        .unwrap_or_else(|| format!(".ara/captures/{}.png", task.job));
    let path = output_path(editor.host.root(), &relative)?;
    if target == "layout" {
        return Ok(Work::Layout(
            path.with_extension("json"),
            json!({"layout":editor.state.layout,"ui":shell.layout_report()}),
        ));
    }
    if target == "editor" {
        let mut png = Vec::new();
        gpu.pixels()?
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
        return Ok(Work::Editor(png, path, shell.layout_report()));
    }
    let capture = scene_view.capture(
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
    let entities = editor
        .view()?
        .entities()
        .keys()
        .map(|entity| json!({"entity":entity,"renderEntity":crate::world::render_id(entity)}))
        .collect::<Vec<_>>();
    Ok(Work::Viewport(Box::new(capture), path, json!(entities)))
}
