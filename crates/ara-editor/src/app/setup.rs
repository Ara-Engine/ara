//! Launch options and typed fixture/resource setup, outside the native frame loop.
use crate::Editor;
use serde_json::{Value, json};
use std::path::PathBuf;
/// Editor launch settings; bounded verification runs use the same production event loop.
pub struct Options {
    /// Render a scene capture without creating a native window, then exit.
    pub headless: bool,
    /// Project directory containing ara.project.json.
    pub project: PathBuf,
    /// Stop after this many event-loop frames; None keeps the editor open.
    pub frames: Option<u64>,
    /// Run the scripted native/protocol verification route and write evidence.
    pub verify: bool,
    /// Populate an empty scene using one typed sample-content batch.
    pub sample: bool,
    /// Screenshots and measurement directory.
    pub output: PathBuf,
    /// Requested native content extent in pixels.
    pub size: [u32; 2],
    /// Optional explicit DPI for reproducible monitor-scale verification.
    pub dpi: Option<f32>,
    /// Open an authoring panel directly; --sample creates its starter asset when absent.
    pub panel: Option<String>,
    /// Save the final composited GPU frame before a bounded run exits.
    pub screenshot: bool,
    /// Run the native authoring panels, import and 500-node navigation suite.
    pub verify_panels: bool,
    /// Run the Editor UX v2 capture route (menus, Settings, floating windows, tabs).
    pub verify_ux: bool,
    /// Scene to open instead of the project's main scene (`levels/hangar.scene`).
    pub scene: Option<String>,
    /// Start Play (in the Game view) once the window is up.
    pub play: bool,
    /// Run the alpha journey route (open, import, place, prefab, C#, Play, failure,
    /// undo/save/reopen, export) and capture every step.
    pub verify_journey: bool,
    /// Measure UI cost and click reliability with input injected into the window
    /// loop (`app/responsiveness.rs`); writes `responsiveness.json`.
    pub verify_responsiveness: bool,
}
impl Options {
    /// Bounded and automated runs (a frame limit, captures, verification routes) keep
    /// every window hidden and capture offscreen.
    pub fn offscreen(&self) -> bool {
        self.frames.is_some()
            || self.screenshot
            || self.verify
            || self.verify_panels
            || self.verify_ux
            || self.verify_journey
            || self.verify_responsiveness
            || self.headless
    }

    /// Bounded runs never touch the owner's profile and start from fresh preferences
    /// (captures must not depend on an earlier run): preferences (`ARA_CONFIG_DIR`)
    /// and crash reports (`ARA_CRASH_DIR`) go to `<output>/.ara-profile`, and the
    /// players and import workers they start inherit both.
    ///
    /// # Safety
    /// Mutates the process environment: call it before any thread exists.
    pub unsafe fn isolate_profile(&self) {
        if !self.offscreen() {
            return;
        }
        let profile = self.output.join(".ara-profile");
        let profile = std::path::absolute(&profile).unwrap_or(profile);
        let _ = std::fs::remove_dir_all(profile.join("config"));
        for (name, dir) in [("ARA_CONFIG_DIR", "config"), ("ARA_CRASH_DIR", "crashes")] {
            // SAFETY: the caller guarantees the process is still single-threaded.
            unsafe { std::env::set_var(name, profile.join(dir)) };
        }
    }
}
impl Default for Options {
    fn default() -> Self {
        Self {
            headless: false,
            project: ".".into(),
            frames: None,
            verify: false,
            sample: false,
            output: PathBuf::from(".ara/captures"),
            size: [1440, 900],
            dpi: None,
            panel: None,
            screenshot: false,
            verify_panels: false,
            verify_ux: false,
            scene: None,
            play: false,
            verify_journey: false,
            verify_responsiveness: false,
        }
    }
}
pub(super) fn prepare(editor: &mut Editor, options: &Options) -> ara_ui::Result<()> {
    if let Some(scene) = &options.scene {
        editor.action("editor.scene.open", json!({"scene": scene}))?;
    }
    if options.play {
        editor
            .queued_commands
            .push(("play.start".into(), crate::shell::commands::Target::None));
    }
    if options.sample {
        sample(editor)?;
    }
    if let Some(panel) = &options.panel {
        if options.sample && matches!(panel.as_str(), "material" | "vfx" | "animation") {
            let found = editor.host.store().workspace().docs().any(|(_, d)| {
                if panel == "animation" {
                    d.kind == "animation"
                } else {
                    d.record("g_meta")
                        .and_then(|r| r.get("domain"))
                        .and_then(Value::as_str)
                        == Some(panel)
                }
            });
            if !found {
                let path = format!(
                    "samples/{panel}.{}",
                    if panel == "animation" {
                        "animation"
                    } else {
                        "graph"
                    }
                );
                let doc = if let Some(id) = editor
                    .host
                    .store()
                    .workspace()
                    .find(&path)
                    .map(ToString::to_string)
                {
                    editor.apply_to(
                        &id,
                        vec![json!({"cmd":"editor.graph.initialize","args":{"domain":panel}})],
                        "Initialize sample asset",
                    )?;
                    id
                } else {
                    editor.action(
                        "editor.resource.create",
                        json!({"path":path,"domain":panel}),
                    )?["doc"]
                        .as_str()
                        .ok_or("resource identity missing")?
                        .to_owned()
                };
                editor.action("editor.workspace", json!({"resource":doc}))?;
            }
        }
        editor.action("editor.focus", json!({"panel":panel}))?;
    }
    Ok(())
}
fn sample(editor: &mut Editor) -> ara_ui::Result<()> {
    let empty = editor
        .host
        .store()
        .workspace()
        .doc(&editor.state.doc)
        .is_some_and(|d| d.records().is_empty());
    if !empty {
        return Ok(());
    }
    let mut commands = Vec::new();
    for (name, position, scale, tint) in [
        (
            "Outpost floor",
            [0., -0.25, 0.],
            [12., 0.5, 10.],
            [0.18, 0.20, 0.23, 1.],
        ),
        (
            "Salvage crate",
            [0., 0.6, 0.],
            [1.2, 1.2, 1.2],
            [0.12, 0.28, 0.42, 1.],
        ),
        (
            "Bulkhead",
            [-3., 1.5, -2.],
            [0.3, 3., 6.],
            [0.25, 0.28, 0.32, 1.],
        ),
        (
            "Signal tower",
            [2.5, 1.5, -2.],
            [0.8, 3., 0.8],
            [0.4, 0.3, 0.14, 1.],
        ),
    ] {
        commands.push(json!({"cmd":"entity.create","args":{"name":name,"components":{"Transform":{"position":position,"scale":scale},"MeshRenderer":{"color":tint}}}}));
    }
    editor.apply(commands, "Create sample Outpost scene", None)?;
    editor.host.flush();
    Ok(())
}
