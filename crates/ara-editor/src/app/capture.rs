//! GPU readback stays on the window thread; encoding and disk IO finish in a worker job.
use super::*;
use std::path::{Component, Path};
pub(super) enum Work {
    Viewport(Box<ara_render::capture::Capture>, PathBuf, Value),
    Editor(image::RgbaImage, PathBuf, Value),
    Layout(PathBuf, Value),
}
impl Work {
    pub fn write(self) -> anyhow::Result<Value> {
        match self {
            Self::Viewport(capture, path, entities) => {
                capture.write(&path)?;
                let mapping = path.with_extension("entities.json");
                std::fs::write(&mapping, ara_json::to_canonical_string(&entities))?;
                Ok(
                    json!({"path":path,"marks":path.with_extension("marks.png"),"legend":path.with_extension("legend.json"),"metadata":path.with_extension("json"),"entities":mapping,"target":"viewport"}),
                )
            }
            Self::Editor(image, path, report) => {
                image.save(&path)?;
                let layout = path.with_extension("layout.json");
                std::fs::write(&layout, ara_json::to_canonical_string(&report))?;
                Ok(json!({"path":path,"layout":layout,"target":"editor"}))
            }
            Self::Layout(path, report) => {
                std::fs::write(&path, ara_json::to_canonical_string(&report))?;
                Ok(json!({"path":path,"target":"layout"}))
            }
        }
    }
}
pub(super) fn prepare(
    task: &crate::capture::CaptureTask,
    renderer: &mut Renderer,
    gpu: &mut NativeUi,
    shell: &Shell,
    editor: &Editor,
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
        return Ok(Work::Editor(gpu.pixels()?, path, shell.layout_report()));
    }
    let capture = renderer.capture(&CaptureRequest {
        camera: editor.state.camera.spec(),
        size: renderer.size(),
        mode: ViewMode::Lit,
        deterministic: true,
        exposure: 0.0015,
        settle_frames: 1,
        tick: 0,
        seed: 7,
        marks: Some(task.params.marks.unwrap_or(100).min(1000) as usize),
    })?;
    let entities = editor
        .view()?
        .entities()
        .keys()
        .map(|entity| json!({"entity":entity,"renderEntity":crate::world::render_id(entity)}))
        .collect::<Vec<_>>();
    Ok(Work::Viewport(Box::new(capture), path, json!(entities)))
}
fn output_path(root: &Path, relative: &str) -> anyhow::Result<PathBuf> {
    let relative = Path::new(relative);
    anyhow::ensure!(
        !relative.as_os_str().is_empty()
            && relative
                .components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
        "capture path must stay under the project"
    );
    let path = root.join(relative);
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("capture path has no parent"))?;
    // Check existing ancestors before creating directories, including symlink targets.
    let existing = parent
        .ancestors()
        .find(|p| p.exists())
        .ok_or_else(|| anyhow::anyhow!("capture parent is missing"))?;
    anyhow::ensure!(
        std::fs::canonicalize(existing)?.starts_with(std::fs::canonicalize(root)?),
        "capture path escapes the project"
    );
    std::fs::create_dir_all(parent)?;
    anyhow::ensure!(
        std::fs::canonicalize(parent)?.starts_with(std::fs::canonicalize(root)?),
        "capture parent escapes project"
    );
    Ok(path)
}
