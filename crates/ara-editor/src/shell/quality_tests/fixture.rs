use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

pub(super) struct Project(pub PathBuf);
impl Project {
    pub fn new() -> Self {
        let root = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target"))
            .join("editor-quality")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("textures")).unwrap();
        for (path, value) in [
            (
                "ara.project.json",
                json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Editor quality","main":"levels/main.scene"}),
            ),
            (
                "levels/main.scene",
                json!({"$schema":"ara://schema/scene/1","id":"sc_quality1","records":{
                    "e_quality1":{"name":"Quality fixture","Transform":{},"MeshRenderer":{},"Light":{"kind":"spot"},"Camera":{},"PostProcess":{}}
                }}),
            ),
        ] {
            std::fs::write(
                root.join(
                    if path == "ara.project.json" || path.starts_with("settings/") {
                        std::path::PathBuf::from(path)
                    } else {
                        std::path::PathBuf::from(ara_doc::AUTHORING_ROOT).join(path)
                    },
                ),
                ara_json::to_canonical_string(&value),
            )
            .unwrap();
        }
        image::RgbaImage::from_pixel(8, 8, image::Rgba([70, 120, 180, 255]))
            .save(
                root.join(ara_doc::AUTHORING_ROOT)
                    .join("textures/quality.png"),
            )
            .unwrap();
        Self(root)
    }

    pub fn open(&self, size: [f32; 2]) -> (Editor, Shell) {
        let mut editor =
            Editor::open_with(&self.0, Some(self.0.join(".ara/quality-user.json"))).unwrap();
        editor
            .action("editor.preferences", json!({"reduceMotion":true}))
            .unwrap();
        editor
            .action("editor.select", json!({"entities":["e_quality1"]}))
            .unwrap();
        for domain in ["material", "vfx", "animation"] {
            let extension = if domain == "animation" {
                "animation"
            } else {
                "graph"
            };
            editor
                .action(
                    "editor.resource.create",
                    json!({"path":format!("quality/{domain}.{extension}"),"domain":domain}),
                )
                .unwrap();
        }
        let job = editor
            .action(
                "editor.asset.import",
                json!({"path":format!("{}/textures/quality.png",ara_doc::AUTHORING_ROOT)}),
            )
            .unwrap();
        editor
            .action("jobs.cancel", json!({"job":job["job"]}))
            .unwrap();
        let mut shell = Shell::new(&mut editor, size).unwrap();
        shell.offscreen = true;
        shell.open_ide = false;
        (editor, shell)
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[derive(Default)]
pub(super) struct Clip(String);
impl ara_ui_text::Clipboard for Clip {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, value: &str) -> ara_ui_text::Result<()> {
        self.0 = value.into();
        Ok(())
    }
}

pub(super) fn show(editor: &mut Editor, shell: &mut Shell, panel: &str) {
    editor
        .action("editor.layout.move", json!({"panel":panel,"slot":"view"}))
        .unwrap();
    editor
        .action("editor.focus", json!({"panel":panel}))
        .unwrap();
    editor
        .action("editor.view", json!({"maximized":panel}))
        .unwrap();
    shell.rebuild(editor).unwrap();
    shell.settle_transitions();
    shell.frame().unwrap();
}
