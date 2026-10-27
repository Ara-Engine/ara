use super::{
    Settings,
    live_tests::{Project, agent, apply, ticks},
};
use ara_doc::{AssetId, Document, Edit, RecordId, Workspace};
use ara_host::project_policy::{PREFERENCES, PREFERENCES_ID, policy};
use ara_ops::{Author, TxnMeta};
use ara_proto::Endpoint;
use ara_schema::FieldPath;
use serde_json::{Value, json};

#[test]
fn unrelated_canonical_changes_survive_human_settings_undo_redo_and_keep_its_labels() {
    let project = Project::new();
    let mut editor = project.open();
    editor
        .action(
            "editor.settings.set",
            json!({"key":"play.clients","value":4}),
        )
        .unwrap();
    let label = editor.state.settings_undo.clone();
    let session = agent(&mut editor);
    apply(
        &mut editor,
        &session,
        &[
            ("settings.settings[build.profile]", json!("agent shipping")),
            ("settings.rendering.quality", json!("medium")),
            (
                "settings.plugin.agentOnly",
                json!({"retain":["unrecognized","policy"]}),
            ),
        ],
    );
    ticks(&mut editor);
    let revision = editor.host.store().revision();
    assert_eq!(
        editor.state.settings["play.clients"], 4,
        "an unchanged Host snapshot must not replay over a local setting"
    );
    assert_eq!(editor.state.settings_undo, label);
    for (redo, clients) in [(false, 1), (true, 4)] {
        editor
            .action("editor.settings.undo", json!({"redo":redo}))
            .unwrap();
        ticks(&mut editor);
        assert_eq!(editor.state.settings["play.clients"], clients);
        assert_eq!(editor.state.settings["build.profile"], "agent shipping");
        assert_eq!(editor.state.settings["rendering.quality"], "medium");
        assert_eq!(
            editor.host.store().revision(),
            revision,
            "existing settings undo must not manufacture Host content ops"
        );
        let workspace = editor.host.store().workspace();
        assert_eq!(
            policy(workspace, "settings").unwrap()["plugin.option"],
            json!({"retained":true})
        );
        assert_eq!(
            policy(workspace, "plugin").unwrap()["opaque"],
            json!([1, 2, 3])
        );
        assert_eq!(
            policy(workspace, "plugin").unwrap()["agentOnly"],
            json!({"retain":["unrecognized","policy"]})
        );
    }
    assert_eq!(editor.state.settings_undo, label);
    assert!(editor.state.settings_redo.is_none());
}

#[test]
fn an_agent_change_to_the_same_setting_blocks_old_undo_and_redo_without_reverting_it() {
    let project = Project::new();
    let mut editor = project.open();
    editor
        .action(
            "editor.settings.set",
            json!({"key":"build.profile","value":"human build"}),
        )
        .unwrap();
    let session = agent(&mut editor);
    apply(
        &mut editor,
        &session,
        &[("settings.settings[build.profile]", json!("agent build"))],
    );
    ticks(&mut editor);
    let revision = editor.host.store().revision();
    let error = editor
        .action("editor.settings.undo", json!({}))
        .unwrap_err();
    assert!(
        error.0.data.as_ref().unwrap()["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "ARA1502")
    );
    assert_eq!(editor.state.settings["build.profile"], "agent build");
    assert_eq!(editor.host.store().revision(), revision);
    // A new explicit edit captures the current agent value as its own inverse.
    editor
        .action(
            "editor.settings.set",
            json!({"key":"build.profile","value":"human follow-up"}),
        )
        .unwrap();
    ticks(&mut editor);
    assert_eq!(editor.state.settings["build.profile"], "human follow-up");
    editor.action("editor.settings.undo", json!({})).unwrap();
    assert_eq!(editor.state.settings["build.profile"], "agent build");
    apply(
        &mut editor,
        &session,
        &[("settings.settings[build.profile]", json!("new agent build"))],
    );
    ticks(&mut editor);
    let redo = editor
        .action("editor.settings.undo", json!({"redo":true}))
        .unwrap_err();
    assert!(
        redo.0.data.as_ref().unwrap()["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "ARA1502")
    );
    assert_eq!(editor.state.settings["build.profile"], "new agent build");
    assert_eq!(editor.host.store().revision(), revision + 1);
}

#[test]
fn recorded_same_value_policy_ops_override_local_values_only_from_the_effective_source() {
    let project = Project::new();
    let mut editor = project.open();
    editor
        .action(
            "editor.settings.set",
            json!({"key":"build.profile","value":"human private value"}),
        )
        .unwrap();
    let session = agent(&mut editor);
    // This is a real recorded mutation, but the canonical section masks it.
    Endpoint::call(&mut editor.host, "cmd.apply", json!({
        "session":session,"doc":"pj_project","why":"Edit masked legacy policy",
        "commands":[{"cmd":"property.set","args":{"entity":"p_settings","path":"settings.settings[build.profile]","value":"legacy"}}]
    })).unwrap();
    ticks(&mut editor);
    assert_eq!(
        editor.state.settings["build.profile"],
        "human private value"
    );
    let revision = editor.host.store().revision();
    apply(
        &mut editor,
        &session,
        &[("settings.settings[build.profile]", json!("initial"))],
    );
    let repeated = editor
        .host
        .store()
        .since(revision)
        .iter()
        .flat_map(|txn| &txn.ops)
        .any(|op| {
            matches!(op,
                ara_doc::Op::Set { value:Some(value),prev:Some(previous),.. }
                    if value==previous && value=="initial"
            )
        });
    ticks(&mut editor);
    if repeated {
        assert_eq!(
            editor.state.settings["build.profile"], "initial",
            "a recorded equal-value Set is an explicit canonical override"
        );
    } else {
        assert_eq!(
            editor.state.settings["build.profile"], "human private value",
            "an elided no-op RPC must not invent authoring intent"
        );
    }
    editor
        .action(
            "editor.settings.set",
            json!({"key":"build.profile","value":"new human private value"}),
        )
        .unwrap();
    Endpoint::call(&mut editor.host, "cmd.apply", json!({
        "session":session,"doc":"sc_livepolicy","why":"Unrelated background scene edit",
        "commands":[{"cmd":"property.set","args":{"entity":"e_policy01","path":"name","value":"Background rename"}}]
    })).unwrap();
    ticks(&mut editor);
    assert_eq!(
        editor.state.settings["build.profile"],
        "new human private value"
    );
    // Two real mutations can return to the same observed value between ticks.
    let revision = editor.host.store().revision();
    apply(
        &mut editor,
        &session,
        &[
            (
                "settings.settings[build.profile]",
                json!("intermediate agent value"),
            ),
            ("settings.settings[build.profile]", json!("initial")),
        ],
    );
    assert!(
        editor
            .host
            .store()
            .since(revision)
            .iter()
            .flat_map(|txn| &txn.ops)
            .any(|op| matches!(op, ara_doc::Op::Set { value,prev,.. } if value!=prev)),
        "the round trip must contain actual mutations"
    );
    ticks(&mut editor);
    assert_eq!(editor.state.settings["build.profile"], "initial");
    let revision = editor.host.store().revision();
    ticks(&mut editor);
    assert_eq!(editor.host.store().revision(), revision);
}

fn workspace(preferences: Option<Value>) -> Workspace {
    let mut workspace = Workspace::default();
    for (path, id, value) in [
        (
            "ara.project.json",
            "pj_project",
            Some(
                json!({"name":"Live settings","main":"levels/main.scene","settings":{"build.profile":"legacy","play.tickHz":"30"},"rendering":{"quality":"epic"}}),
            ),
        ),
        (PREFERENCES, PREFERENCES_ID, preferences),
    ] {
        if let Some(value) = value {
            let mut document = Document::new("project", AssetId::parse(id).unwrap());
            document.insert_unchecked(
                RecordId::parse("p_settings").unwrap(),
                serde_json::Map::from_iter([("settings".into(), value)]),
            );
            workspace.insert_project_document(path, document);
        }
    }
    workspace
}

#[test]
fn live_projection_obeys_empty_null_invalid_and_legacy_sections_without_writing_files_or_history() {
    let project = Project::new();
    let mut settings = Settings::open(&project.0, None).unwrap();
    settings
        .set("appearance.theme", Some(json!("light")), Author::Human)
        .unwrap();
    let files: Vec<_> = ["ara.project.json", PREFERENCES]
        .map(|path| (path, std::fs::read(project.0.join(path)).unwrap()))
        .into_iter()
        .collect();
    let revision = settings.store.revision();
    let cases = [
        (
            Some(json!({"settings":{},"rendering":null})),
            "",
            "60",
            "auto",
        ),
        (None, "legacy", "30", "epic"),
        (
            Some(
                json!({"settings":{"build.profile":false,"play.tickHz":"999"},"rendering":{"quality":"ultra"}}),
            ),
            "",
            "60",
            "auto",
        ),
        (
            Some(
                json!({"settings":{"build.profile":"canonical","plugin.unrecognized":[1,2,3]},"rendering":{"quality":"medium"}}),
            ),
            "canonical",
            "60",
            "medium",
        ),
    ];
    for (preferences, profile, tick_hz, quality) in cases {
        let workspace = workspace(preferences);
        assert!(settings.reconcile_values(&workspace, &[]));
        assert!(
            !settings.reconcile_values(&workspace, &[]),
            "an idle read must be unchanged"
        );
        assert_eq!(settings.value("build.profile"), json!(profile));
        assert_eq!(settings.value("play.tickHz"), json!(tick_hz));
        assert_eq!(settings.value("rendering.quality"), json!(quality));
        assert_eq!(settings.value("appearance.theme"), "light");
        assert_eq!(settings.value("project.name"), "Live settings");
        assert_eq!(settings.store.revision(), revision);
        for (path, before) in &files {
            assert_eq!(
                &std::fs::read(project.0.join(path)).unwrap(),
                before,
                "projection wrote {path}"
            );
        }
    }
}

#[test]
fn a_legacy_whole_map_history_entry_cannot_overwrite_a_new_host_projection() {
    let project = Project::new();
    let mut settings = Settings::open(&project.0, None).unwrap();
    let mut values = settings.project.clone();
    values.insert("play.clients".into(), json!(4));
    let mut meta = TxnMeta::by(Author::Human);
    meta.label = Some("Legacy clients edit".into());
    settings
        .store
        .transact(
            meta,
            [Edit::Set {
                doc: settings.doc.clone(),
                id: RecordId::parse(super::PROJECT).unwrap(),
                path: FieldPath::field("EditorSettings").child("values"),
                value: Some(Value::Object(values)),
            }],
        )
        .unwrap();
    settings.reload(Some(super::Scope::Project));
    assert!(settings.reconcile_values(&workspace(Some(json!({"settings":{"build.profile":"agent build","play.tickHz":"120"},"rendering":{"quality":"high"}}))), &[]));
    let revision = settings.store.revision();
    let before = settings.snapshot();
    let errors = settings.undo(&Author::Human, false).unwrap_err();
    assert!(
        errors
            .iter()
            .any(|diagnostic| diagnostic.code == ara_diag::codes::UNDO_CONFLICT)
    );
    assert_eq!(settings.snapshot(), before);
    assert_eq!(settings.value("play.clients"), 4);
    assert_eq!(settings.value("build.profile"), "agent build");
    assert_eq!(settings.store.revision(), revision);
}
