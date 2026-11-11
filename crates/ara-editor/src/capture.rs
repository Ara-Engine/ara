//! Captures run as jobs at a render boundary; protocol IO never owns the GPU.
use ara_diag::Diagnostic;
use ara_host::{Cx, Method, Service};
use ara_schema::{Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::mpsc::{Receiver, Sender};

/// Parameters of an editor or viewport screenshot job.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CaptureParams {
    /// Live protocol session.
    #[expect(
        dead_code,
        reason = "Host validates the session before service dispatch"
    )]
    pub session: String,
    /// `viewport`, `editor`, `layout`, or `window` (the editor window with its Scene in it).
    pub target: Option<String>,
    /// Relative output PNG path under the project (default `.ara/captures/<job>.png`).
    pub path: Option<String>,
    /// Numbered viewport marks and legend.
    pub marks: Option<u32>,
}
pub(crate) struct CaptureTask {
    pub job: String,
    pub params: CaptureParams,
}
pub(crate) struct CaptureCompletion {
    pub job: String,
    pub result: Result<Value, Vec<Diagnostic>>,
}
pub(crate) struct CaptureService {
    tasks: Sender<CaptureTask>,
    results: Receiver<CaptureCompletion>,
}
impl CaptureService {
    pub fn new(tasks: Sender<CaptureTask>, results: Receiver<CaptureCompletion>) -> Self {
        Self { tasks, results }
    }
}
impl Service for CaptureService {
    fn name(&self) -> &'static str {
        "capture"
    }
    fn methods(&self, db: &mut SchemaDb) -> Vec<Method> {
        vec![Method::new::<CaptureParams>(db,"capture","Capture the viewport with entity marks, the editor UI, its layout report, or the whole window with the Scene in it (target window); returns a job.").tool("ara_capture").read_only()]
    }
    fn call(&mut self, cx: &mut Cx<'_>, _: &str, params: Value) -> Result<Value, Vec<Diagnostic>> {
        let params: CaptureParams = ara_host::parse(params)?;
        if !matches!(
            params.target.as_deref().unwrap_or("viewport"),
            "viewport" | "editor" | "layout" | "window"
        ) {
            return Err(vec![crate::invalid(
                "capture target must be viewport, editor, layout or window",
            )]);
        }
        let job = cx.start_job("capture");
        cx.job_progress(&job, 0., "Waiting for render boundary");
        self.tasks
            .send(CaptureTask {
                job: job.clone(),
                params,
            })
            .map_err(|_| vec![crate::invalid("editor renderer has closed")])?;
        Ok(json!({"job":job,"status":"running"}))
    }
    fn tick(&mut self, cx: &mut Cx<'_>) {
        while let Ok(completion) = self.results.try_recv() {
            if !cx.job_cancelled(&completion.job) {
                cx.finish_job(&completion.job, completion.result);
            }
        }
    }
}

/// What a capture job writes, prepared on the window thread (GPU readback); [`Work::write`]
/// encodes and saves it on a worker.
pub(crate) enum Work {
    /// The Scene with numbered entity marks, and the render-id to entity map.
    Viewport(Box<ara_frame::capture::Capture>, std::path::PathBuf, Value),
    /// The editor window as a PNG, and its layout report.
    Editor(Vec<u8>, std::path::PathBuf, Value),
    /// Only the layout report.
    Layout(std::path::PathBuf, Value),
    /// The editor window with its Scene composited in, and its layout report.
    Window(image::RgbaImage, std::path::PathBuf, Value),
}
impl Work {
    /// Writes the files and gives the job's result.
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
            Self::Editor(png, path, report) => {
                std::fs::write(&path, png)?;
                let layout = path.with_extension("layout.json");
                std::fs::write(&layout, ara_json::to_canonical_string(&report))?;
                Ok(json!({"path":path,"layout":layout,"target":"editor",
                    "note":"The Scene, Game and graph views are native GPU layers: this PNG shows their place as a hole, not their picture. Capture target `viewport` for the Scene."}))
            }
            Self::Layout(path, report) => {
                std::fs::write(&path, ara_json::to_canonical_string(&report))?;
                Ok(json!({"path":path,"target":"layout"}))
            }
            Self::Window(image, path, report) => {
                image.save(&path)?;
                let layout = path.with_extension("layout.json");
                std::fs::write(&layout, ara_json::to_canonical_string(&report))?;
                Ok(json!({"path":path,"layout":layout,"target":"window"}))
            }
        }
    }
    /// Finishes `prepared` for `job` on a worker thread and reports it to the host. A panic
    /// in the worker (an encoder, the disk) fails the job instead of leaving it pending.
    pub fn finish(
        prepared: anyhow::Result<Self>,
        job: String,
        completed: Sender<CaptureCompletion>,
    ) {
        match prepared {
            Ok(work) => {
                std::thread::spawn(move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        work.write()
                            .map_err(|e| vec![crate::invalid(e.to_string())])
                    }))
                    .unwrap_or_else(|_| Err(vec![crate::invalid("the capture worker panicked")]));
                    let _ = completed.send(CaptureCompletion { job, result });
                });
            }
            Err(error) => {
                let _ = completed.send(CaptureCompletion {
                    job,
                    result: Err(vec![crate::invalid(error.to_string())]),
                });
            }
        }
    }
}

/// `relative` under the project `root`, its folder created: no absolute path, no `..`, and
/// no symlinked folder leading outside the project.
pub(crate) fn output_path(
    root: &std::path::Path,
    relative: &str,
) -> anyhow::Result<std::path::PathBuf> {
    use std::path::{Component, Path};
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
