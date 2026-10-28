#![allow(clippy::unwrap_used)] // Tests assert each fixture operation at its call site.

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

fn canonical_clients(editor: &crate::Editor) -> f64 {
    policy(editor.host.store().workspace(), "settings")
        .and_then(|settings| settings.get("play.clients"))
        .map(|value| value.as_f64().expect("Canonical clients is numeric"))
        .unwrap_or(1.)
}

fn wait_for_client_echo(editor: &mut crate::Editor, expected: f64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while canonical_clients(editor) != expected {
        editor.tick();
        assert!(
            std::time::Instant::now() < deadline,
            "Settings persistence did not reach the canonical Host policy: expected {expected}, got {}",
            canonical_clients(editor)
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

fn without_client(value: Option<&Value>, suffix: &[&str]) -> (Option<Value>, Option<Value>) {
    let Some(mut value) = value.cloned() else {
        return (None, None);
    };
    let Some((last, parents)) = suffix.split_last() else {
        return (Some(value), None);
    };
    let mut parent = &mut value;
    for key in parents {
        parent = parent
            .get_mut(*key)
            .expect("The persisted client setting has its parent object");
    }
    let client = parent
        .as_object_mut()
        .expect("The persisted setting parent is an object")
        .remove(*last);
    (client, Some(value))
}

fn assert_only_client_echoes(editor: &crate::Editor, since: u64, expected: f64) {
    let transactions = editor.host.store().since(since);
    assert!(
        !transactions.is_empty(),
        "The changed persisted client setting must produce a canonical echo"
    );
    for transaction in transactions {
        assert_eq!(
            transaction.author,
            Author::Human,
            "Unexpected settings echo: {transaction:?}"
        );
        assert_eq!(
            transaction.why.as_deref(),
            Some("`settings/project.json` changed on disk"),
            "Unexpected settings echo: {transaction:?}"
        );
        for op in &transaction.ops {
            let ara_doc::Op::Set {
                doc,
                id,
                path,
                value,
                prev,
            } = op
            else {
                panic!("Unexpected settings echo operation: {op:?}")
            };
            assert_eq!(
                doc.as_str(),
                PREFERENCES_ID,
                "Unexpected settings echo: {op:?}"
            );
            assert_eq!(
                id.as_str(),
                "p_settings",
                "Unexpected settings echo: {op:?}"
            );
            let canonical_path = ["settings", "settings", "play.clients"];
            let segments: Vec<_> = path
                .segments()
                .iter()
                .map(|segment| {
                    let (ara_schema::Segment::Field(key) | ara_schema::Segment::Key(key)) = segment;
                    key.as_str()
                })
                .collect();
            assert!(
                canonical_path.starts_with(&segments),
                "Echo changed another policy key: {op:?}"
            );
            let suffix = &canonical_path[segments.len()..];
            let (_, previous_other) = without_client(prev.as_ref(), suffix);
            let (client, current_other) = without_client(value.as_ref(), suffix);
            assert_eq!(
                previous_other, current_other,
                "Echo changed policy outside play.clients: {op:?}"
            );
            assert_eq!(
                client
                    .as_ref()
                    .map(|value| value.as_f64().expect("Persisted clients is numeric"))
                    .unwrap_or(1.),
                expected,
                "Echo stored an unexpected client count: {op:?}"
            );
        }
    }
    assert_eq!(
        canonical_clients(editor),
        expected,
        "Canonical clients after echoes {transactions:?}"
    );
}

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
    wait_for_client_echo(&mut editor, 4.);
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
    assert_eq!(
        editor.state.settings["play.clients"], 4,
        "an unchanged Host snapshot must not replay over a local setting"
    );
    assert_eq!(editor.state.settings_undo, label);
    for (redo, clients) in [(false, 1), (true, 4)] {
        let revision = editor.host.store().revision();
        editor
            .action("editor.settings.undo", json!({"redo":redo}))
            .unwrap();
        ticks(&mut editor);
        wait_for_client_echo(&mut editor, f64::from(clients));
        assert_eq!(
            editor.state.settings["play.clients"].as_f64(),
            Some(f64::from(clients))
        );
        assert_eq!(editor.state.settings["build.profile"], "agent shipping");
        assert_eq!(editor.state.settings["rendering.quality"], "medium");
        assert_only_client_echoes(&editor, revision, f64::from(clients));
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
    let canonical_before =
        policy(editor.host.store().workspace(), "settings").unwrap()["build.profile"].clone();
    apply(
        &mut editor,
        &session,
        &[
            ("settings.settings[build.profile]", json!("initial")),
            ("settings.settings[build.profile]", json!("initial")),
        ],
    );
    let target: FieldPath = "settings.settings[build.profile]".parse().unwrap();
    let recorded: Vec<_> = editor
        .host
        .store()
        .since(revision)
        .iter()
        .flat_map(|txn| &txn.ops)
        .filter(|op| {
            matches!(op, ara_doc::Op::Set { doc,id,path,.. }
            if doc.as_str()==PREFERENCES_ID && id.as_str()=="p_settings" && path==&target)
        })
        .collect();
    let recorded_intent = !recorded.is_empty();
    let evidence = format!("canonical before {canonical_before}; recorded Ops {recorded:?}");
    if recorded.len() == 2 {
        assert!(
            recorded.iter().any(|op| matches!(op,
                ara_doc::Op::Set { value:Some(value),prev:Some(previous),.. }
                    if value==previous && value=="initial"
            )),
            "consecutive identical Sets must include an equal-value recorded Op: {evidence}"
        );
    }
    ticks(&mut editor);
    if recorded_intent {
        assert_eq!(
            editor.state.settings["build.profile"], "initial",
            "a recorded Set is an explicit canonical override: {evidence}"
        );
    } else {
        assert_eq!(
            editor.state.settings["build.profile"], "human private value",
            "an elided no-op RPC must not invent authoring intent: {evidence}"
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

#[test]
fn settings_undo_redo_titles_follow_original_transaction_identity_without_stripping_its_title() {
    let project = Project::new();
    let mut settings = Settings::open(&project.0, None).unwrap();
    let title = "undo authored workflow";
    let mut meta = TxnMeta::by(Author::Human);
    meta.label = Some(title.into());
    meta.why = Some("Keep an original action title containing undo".into());
    settings
        .store
        .transact(
            meta,
            [Edit::Set {
                doc: settings.doc.clone(),
                id: RecordId::parse(super::USER).unwrap(),
                path: FieldPath::field("EditorSettings")
                    .child("values")
                    .key("appearance.theme"),
                value: Some(json!("light")),
            }],
        )
        .unwrap();
    settings.reload(Some(super::Scope::User));
    let original = settings.store.history()[0].clone();
    for _ in 0..3 {
        assert_eq!(
            settings.next_label(&Author::Human, false).as_deref(),
            Some(title)
        );
        assert_eq!(settings.undo(&Author::Human, false).unwrap(), title);
        assert_eq!(settings.value("appearance.theme"), "dark");
        assert_eq!(
            settings.next_label(&Author::Human, true).as_deref(),
            Some(title)
        );
        assert_eq!(settings.undo(&Author::Human, true).unwrap(), title);
        assert_eq!(settings.value("appearance.theme"), "light");
    }
    assert_eq!(
        settings.next_label(&Author::Human, false).as_deref(),
        Some(title)
    );
    assert_eq!(
        settings.store.history()[0],
        original,
        "Presenting the original action leaves its attributed transaction intact"
    );
    assert_eq!(
        settings.store.history()[1].label.as_deref(),
        Some("undo undo authored workflow")
    );
    assert_eq!(settings.store.history().len(), 7);
}
