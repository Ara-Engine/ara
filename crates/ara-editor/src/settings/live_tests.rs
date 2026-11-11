#![allow(clippy::unwrap_used)] // Tests assert each fixture operation at its call site.

use crate::Editor;
use ara_host::project_policy::{PREFERENCES, PREFERENCES_ID};
use ara_proto::Endpoint;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
pub(super) struct Project(pub(super) PathBuf);
impl Project {
    pub(super) fn new() -> Self {
        let parent = std::path::absolute(crate::test_target())
            .unwrap()
            .join("editor-live-project-policy");
        std::fs::create_dir_all(&parent).unwrap();
        let root = parent.join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
        std::fs::create_dir(root.join("settings")).unwrap();
        for (path, value) in [
            (
                "ara.project.json",
                json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Live settings","main":"levels/main.scene", "settings":{"build.profile":"legacy","play.tickHz":"30"},"rendering":{"quality":"epic"}}),
            ),
            (
                PREFERENCES,
                json!({"settings":{"build.profile":"initial","play.tickHz":"120","plugin.option":{"retained":true}},"rendering":{"quality":"high"},"plugin":{"opaque":[1,2,3]}}),
            ),
            (
                "levels/main.scene",
                json!({"$schema":"ara://schema/scene/1","id":"sc_livepolicy","records":{"e_policy01":{"name":"Policy fixture","Transform":{},"MeshRenderer":{}}}}),
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
        Self(root)
    }
    pub(super) fn open(&self) -> Editor {
        Editor::open(&self.0).unwrap()
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        assert!(
            self.0
                .starts_with(std::path::absolute(crate::test_target()).unwrap())
        );
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(super) fn agent(editor: &mut Editor) -> String {
    Endpoint::call(
        &mut editor.host,
        "session.begin",
        json!({"client":"live-policy-test","author":"agent:policy"}),
    )
    .unwrap()["session"]
        .as_str()
        .unwrap()
        .to_owned()
}
pub(super) fn apply(editor: &mut Editor, session: &str, changes: &[(&str, Value)]) -> Value {
    let commands: Vec<_> = changes.iter().map(|(path,value)|json!({"cmd":"property.set","args":{"entity":"p_settings","path":path,"value":value}})).collect();
    Endpoint::call(
        &mut editor.host,
        "cmd.apply",
        json!({"session":session,"doc":PREFERENCES_ID,"why":"Update canonical project policy live","commands":commands}),
    )
    .unwrap()
}
pub(super) fn ticks(editor: &mut Editor) {
    for _ in 0..3 {
        editor.tick();
    }
}

