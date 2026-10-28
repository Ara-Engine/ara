use super::*;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::path::absolute(crate::test_target())
            .unwrap()
            .join("exception-stack-ui")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        let content = root.join(ara_doc::AUTHORING_ROOT);
        std::fs::create_dir_all(content.join("levels")).unwrap();
        std::fs::create_dir_all(content.join("Scripts Space")).unwrap();
        std::fs::create_dir_all(root.join(".ara/test-profile")).unwrap();
        std::fs::write(root.join("ara.project.json"), ara_json::to_canonical_string(&json!({
            "layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Exception details","main":"levels/main.scene"
        }))).unwrap();
        std::fs::write(
            content.join("levels/main.scene"),
            ara_json::to_canonical_string(&json!({
                "$schema":"ara://schema/scene/1","id":"sc_exceptionstack","records":{}
            })),
        )
        .unwrap();
        for file in ["玩家 Controller.cs", "Inner Cause.cs"] {
            std::fs::write(
                content.join("Scripts Space").join(file),
                "// Isolated source-location fixture.\n",
            )
            .unwrap();
        }
        Self(root)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(super) struct Fixture {
    pub editor: Editor,
    pub shell: Shell,
    _project: Project,
}
impl Fixture {
    pub fn new(panel: &str) -> Self {
        let project = Project::new();
        let mut editor = Editor::open_with(
            &project.0,
            Some(project.0.join(".ara/test-profile/editor-settings.json")),
        )
        .unwrap();
        editor.action("editor.settings.set", json!({"key":"scripts.ide","value":project.0.join("Configured Code/Code.exe").to_str().unwrap()})).unwrap();
        editor
            .action("editor.layout.move", json!({"panel":panel,"slot":"view"}))
            .unwrap();
        let mut shell = Shell::new(&mut editor, [1120., 780.]).unwrap();
        shell.offscreen = true;
        shell.open_ide = false;
        Self {
            editor,
            shell,
            _project: project,
        }
    }

    pub fn source(&self, inner: bool) -> String {
        self.editor
            .host
            .root()
            .join(ara_doc::AUTHORING_ROOT)
            .join("Scripts Space")
            .join(if inner {
                "Inner Cause.cs"
            } else {
                "玩家 Controller.cs"
            })
            .to_str()
            .unwrap()
            .to_owned()
    }

    pub fn log(&mut self, exception: Value) -> (u64, ara_diag::Diagnostic) {
        let diagnostic = ara_diag::Diagnostic::error(ara_diag::codes::INVALID_PARAMS, "Managed exception details UI proof")
            .data(json!({"exception":exception,"play":{"world":"world-proof","player":2,"source":"incarnation-proof","binding":3},
                "module":"Gameplay","compile":{"schema":1,"producer":"warm","instance":"compile-proof","binding":4,"attempt":9,"profile":"gameplay","phase":"diagnostic","channel":"compiler"}}));
        self.editor
            .action(
                "diag.report",
                json!({"engine":true,"diagnostics":[diagnostic]}),
            )
            .unwrap();
        let entries = self
            .editor
            .action("logs", json!({"group":false,"limit":500,"severity":"info"}))
            .unwrap();
        let row = entries["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["diagnostic"]["message"] == diagnostic.message)
            .unwrap();
        let seq = row["seq"].as_u64().unwrap();
        self.shell.rebuild(&mut self.editor).unwrap();
        self.shell.frame().unwrap();
        (seq, diagnostic)
    }

    pub fn click(&mut self, id: &str) {
        let index = self
            .shell
            .shown_node(id)
            .unwrap_or_else(|| panic!("missing {id}"));
        let node = &self.shell.ui.nodes[index];
        let rect = node.rect.intersection(node.clip);
        assert!(
            rect.w > 0. && rect.h > 0.,
            "{id}: {:?}/{:?}",
            node.rect,
            node.clip
        );
        let point = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
        assert_eq!(
            self.shell
                .ui
                .hit_test(point[0], point[1])
                .map(|i| self.shell.ui.nodes[i].element.id.as_str()),
            Some(id)
        );
        self.shell
            .event(InputEvent::PointerDown(point), &mut Clip)
            .unwrap();
        self.shell
            .event(InputEvent::PointerUp(point), &mut Clip)
            .unwrap();
        self.shell.dispatch(&mut self.editor).unwrap();
        self.shell.frame().unwrap();
    }

    pub fn wheel(&mut self, container: &str, dy: f32) {
        let index = self.shell.shown_node(container).unwrap();
        let node = &self.shell.ui.nodes[index];
        let rect = node.rect.intersection(node.clip);
        assert!(
            rect.w > 0. && rect.h > 0.,
            "{container} must remain visible"
        );
        let position = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
        self.shell
            .event(
                InputEvent::Wheel {
                    position,
                    delta: [0., dy],
                },
                &mut Clip,
            )
            .unwrap();
        self.shell.dispatch(&mut self.editor).unwrap();
        self.shell.frame().unwrap();
    }

    pub fn assert_request(&self, file: &str, line: u32, column: u32) {
        let request = self.shell.script_ide_request();
        let canonical = std::fs::canonicalize(file).unwrap();
        let file = canonical.to_str().unwrap();
        assert_eq!(request["targetFile"], file);
        assert_eq!(request["line"], line);
        assert_eq!(request["column"], column);
        assert_eq!(
            request["args"],
            json!([
                "--reuse-window",
                "--goto",
                format!("{file}:{line}:{column}")
            ])
        );
        assert_eq!(
            request["program"],
            self.editor.state.settings["scripts.ide"]
        );
        assert_eq!(request["attempted"], false);
        assert_eq!(request["launchSuppressed"], true);
    }
}

struct Clip;
impl ara_ui_text::Clipboard for Clip {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(String::new())
    }
    fn write(&mut self, _: &str) -> ara_ui_text::Result<()> {
        Ok(())
    }
}
