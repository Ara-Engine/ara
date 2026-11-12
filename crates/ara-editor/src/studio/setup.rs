//! Launch options and typed fixture/resource setup, outside the native frame loop.
use crate::Editor;
use serde_json::{Value, json};
use std::path::PathBuf;
/// Editor launch settings, from `ara-editor`'s command line.
pub struct Options {
    /// Render a scene capture without creating a native window, then exit
    /// (`studio::headless`).
    pub headless: bool,
    /// Open the window without activating it or taking focus (automated captures and tests
    /// beside a person's work): `--background`, or `ARA_EDITOR_BACKGROUND=1`.
    pub background: bool,
    /// Project directory containing ara.project.json.
    pub project: PathBuf,
    /// Populate an empty scene using one typed sample-content batch.
    pub sample: bool,
    /// Where a headless or screenshot capture is written.
    pub output: PathBuf,
    /// The headless capture's size, or the studio window's, in pixels.
    pub size: [u32; 2],
    /// Stop after this many presented frames (`--frames N`). The window stays hidden unless
    /// `screenshot` asks for a picture of it; `None` keeps the editor open.
    pub frames: Option<u64>,
    /// After the frames, save the editor window with its Scene in it (the capture service's
    /// `window` target) as `<output>/editor.png`, then exit (`--screenshot`).
    pub screenshot: bool,
    /// Open an authoring panel directly; --sample creates its starter asset when absent.
    pub panel: Option<String>,
    /// Scene to open instead of the project's main scene (`levels/hangar.scene`).
    pub scene: Option<String>,
    /// Entities to select once the scene is open (comma-separated addresses), e.g. for
    /// a capture of their outline.
    pub select: Option<String>,
    /// Start Play (in the Game view) once the window is up.
    pub play: bool,
}

impl Options {
    /// A bounded run: it ends by itself after its frames, or once its screenshot is taken.
    pub fn bounded(&self) -> bool {
        self.frames.is_some() || self.screenshot
    }

    /// A bounded run that wants no picture of the window: it never shows it, and keeps the
    /// protocol server up for as long as it runs (`--frames 1000000` is a server for an
    /// export step). Nothing renders into a hidden window, so counted frames are the loop's.
    pub fn hidden(&self) -> bool {
        self.frames.is_some() && !self.screenshot
    }

    /// Nobody is at the window: a background start (`--background`, `ARA_EDITOR_BACKGROUND=1`)
    /// or any bounded or headless run. The studio's import gate opens by itself for these
    /// once the imports settle, so an agent's captures are never held for a person to press
    /// Open project.
    pub fn unattended(&self) -> bool {
        self.background || self.bounded() || self.headless
    }

    /// A headless or bounded run never touches the owner's profile or the project's saved
    /// window layout (read, never written) and starts from fresh preferences: preferences (`ARA_CONFIG_DIR`) and crash reports (`ARA_CRASH_DIR`) go
    /// to `<output>/.ara-profile`, and the players and import workers it starts inherit
    /// both, hidden (`ARA_PLAY_OFFSCREEN`).
    ///
    /// # Safety
    /// Mutates the process environment: call it before any thread exists.
    pub unsafe fn isolate_profile(&self) {
        if !(self.headless || self.bounded()) {
            return;
        }
        let profile = self.output.join(".ara-profile");
        let profile = std::path::absolute(&profile).unwrap_or(profile);
        let _ = std::fs::remove_dir_all(profile.join("config"));
        for (name, dir) in [("ARA_CONFIG_DIR", "config"), ("ARA_CRASH_DIR", "crashes")] {
            // SAFETY: the caller guarantees the process is still single-threaded.
            unsafe { std::env::set_var(name, profile.join(dir)) };
        }
        // SAFETY: as above.
        unsafe { std::env::set_var("ARA_PLAY_OFFSCREEN", "1") };
        // SAFETY: as above.
        unsafe { std::env::set_var(crate::state::LAYOUT_READ_ONLY, "1") };
    }
}

impl Default for Options {
    fn default() -> Self {
        Self {
            headless: false,
            background: false,
            project: ".".into(),
            sample: false,
            output: PathBuf::from(".ara/captures"),
            size: [1440, 900],
            frames: None,
            screenshot: false,
            panel: None,
            scene: None,
            select: None,
            play: false,
        }
    }
}

pub(crate) fn prepare(editor: &mut Editor, options: &Options) -> ara_ui::Result<()> {
    if let Some(scene) = &options.scene {
        editor.action("editor.scene.open", json!({"scene": scene}))?;
    }
    if let Some(select) = &options.select {
        let entities: Vec<&str> = select.split(',').map(str::trim).collect();
        editor.action("editor.select", json!({"entities": entities}))?;
    }
    if options.play {
        editor
            .queued_commands
            .push(("play.start".into(), crate::catalog::Target::None));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_run_is_hidden_and_a_screenshot_run_is_shown_and_both_are_unattended() {
        let frames = Options {
            frames: Some(20),
            ..Default::default()
        };
        assert!(frames.bounded() && frames.hidden() && frames.unattended());
        let shot = Options {
            frames: Some(4),
            screenshot: true,
            ..Default::default()
        };
        assert!(shot.bounded() && !shot.hidden() && shot.unattended());
        let background = Options {
            background: true,
            ..Default::default()
        };
        assert!(!background.bounded() && !background.hidden() && background.unattended());
        let person = Options::default();
        assert!(!person.bounded() && !person.hidden() && !person.unattended());
    }
}
