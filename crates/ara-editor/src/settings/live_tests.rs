#![allow(clippy::unwrap_used)] // Tests assert each fixture operation at its call site.

use crate::{Editor, shell::Shell};
use ara_host::project_policy::{PREFERENCES, PREFERENCES_ID};
use ara_proto::Endpoint;
use ara_render_fidelity::presets::{QualityChoice, QualityPreset};
use ara_ui::{input::InputEvent, source::stable_id, widgets::Widget};
use ara_ui_text::{Clipboard, EditEvent};
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
        std::fs::create_dir_all(root.join("levels")).unwrap();
        std::fs::create_dir(root.join("settings")).unwrap();
        for (path, value) in [
            (
                "ara.project.json",
                json!({"name":"Live settings","main":"levels/main.scene", "settings":{"build.profile":"legacy","play.tickHz":"30"},"rendering":{"quality":"epic"}}),
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
            std::fs::write(root.join(path), ara_json::to_canonical_string(&value)).unwrap();
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

#[derive(Default)]
struct Clip(String);
impl Clipboard for Clip {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, value: &str) -> ara_ui_text::Result<()> {
        self.0 = value.into();
        Ok(())
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

#[test]
fn host_project_policy_ops_and_host_undo_redo_update_live_settings_without_navigation_or_draft_loss()
 {
    let project = Project::new();
    let mut editor = project.open();
    editor
        .action("editor.theme", json!({"light":true}))
        .unwrap();
    editor
        .action("editor.select", json!({"entities":["e_policy01"]}))
        .unwrap();
    editor
        .action("editor.focus", json!({"panel":"inspector"}))
        .unwrap();
    let mut shell = Shell::new(&mut editor, [1280., 900.]).unwrap();
    shell.frame().unwrap();
    let mut clip = Clip::default();
    shell.click("entity-name", &mut clip).unwrap();
    for edit in [
        EditEvent::SelectAll,
        EditEvent::Insert("Uncommitted policy draft".into()),
    ] {
        shell.event(InputEvent::Edit(edit), &mut clip).unwrap();
    }
    shell.dispatch(&mut editor).unwrap();
    assert_eq!(
        shell.drafts().get("@name").map(String::as_str),
        Some("Uncommitted policy draft")
    );
    assert_eq!(shell.input.focused(), Some(stable_id("entity-name")));
    assert_eq!(
        editor.state.project_quality(),
        QualityChoice::Preset(QualityPreset::High)
    );
    assert_eq!(editor.state.settings["build.profile"], "initial");
    assert_eq!(editor.state.settings["play.tickHz"], "120");
    let camera = editor.state.camera;
    let selection = editor.state.selection.clone();
    let focus = editor.state.focus.clone();
    let history = editor.action("editor.history", json!({})).unwrap();
    let human_undo = editor.state.settings_undo.clone();
    let session = agent(&mut editor);
    let scene_revision = editor.scene_revision;
    let before = editor.host.store().revision();
    apply(
        &mut editor,
        &session,
        &[
            ("settings.rendering.quality", json!("medium")),
            ("settings.settings[build.profile]", json!("agent shipping")),
            ("settings.plugin.opaque", json!([4, 5, 6])),
        ],
    );
    ticks(&mut editor);
    assert_eq!(editor.state.settings["build.profile"], "agent shipping");
    assert_eq!(
        editor.state.project_quality(),
        QualityChoice::Preset(QualityPreset::Medium)
    );
    assert_eq!(
        editor.state.render_choice(),
        QualityChoice::Preset(QualityPreset::Medium)
    );
    assert!(
        editor.scene_revision > scene_revision,
        "High to Medium must invalidate the rendered Scene even though both use quality=true"
    );
    assert_eq!(
        editor.host.store().revision(),
        before + 1,
        "projection must not manufacture a content transaction"
    );
    let host_txn = editor.host.store().history().last().unwrap();
    assert_eq!(host_txn.author, ara_ops::Author::Agent("policy".into()));
    assert_eq!(
        host_txn.why.as_deref(),
        Some("Update canonical project policy live")
    );
    shell.rebuild_if_stale(&mut editor).unwrap();
    shell.frame().unwrap();
    assert_eq!(shell.input.focused(), Some(stable_id("entity-name")));
    assert_eq!(
        shell.drafts().get("@name").map(String::as_str),
        Some("Uncommitted policy draft")
    );
    let Widget::Input(field) = &shell.ui.widgets[&stable_id("entity-name")] else {
        panic!("Name input missing")
    };
    assert_eq!(field.value(), "Uncommitted policy draft");
    assert_eq!(editor.state.camera, camera);
    assert_eq!(editor.state.selection, selection);
    assert_eq!(editor.state.focus, focus);
    assert!(
        editor.state.light,
        "project policy must not replace user preferences"
    );
    assert_eq!(editor.state.settings_undo, human_undo);
    assert_eq!(
        editor.action("editor.history", json!({})).unwrap(),
        history,
        "read-model updates must not add navigation history"
    );
    let revision = editor.host.store().revision();
    Endpoint::call(&mut editor.host, "history.undo", json!({"session":session})).unwrap();
    ticks(&mut editor);
    assert_eq!(editor.state.settings["build.profile"], "initial");
    assert_eq!(
        editor.state.project_quality(),
        QualityChoice::Preset(QualityPreset::High)
    );
    Endpoint::call(&mut editor.host, "history.redo", json!({"session":session})).unwrap();
    ticks(&mut editor);
    assert_eq!(editor.state.settings["build.profile"], "agent shipping");
    assert_eq!(
        editor.state.project_quality(),
        QualityChoice::Preset(QualityPreset::Medium)
    );
    assert_eq!(editor.host.store().revision(), revision + 2);
    let policy =
        ara_host::project_policy::policy(editor.host.store().workspace(), "plugin").unwrap();
    assert_eq!(policy["opaque"], json!([4, 5, 6]));
    assert_eq!(
        ara_host::project_policy::policy(editor.host.store().workspace(), "settings").unwrap()["plugin.option"],
        json!({"retained":true})
    );
}
