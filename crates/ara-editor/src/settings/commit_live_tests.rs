#![allow(clippy::unwrap_used)] // Each assertion operates on its isolated fixture.
use super::live_tests::{Project, agent};
use ara_host::project_policy::{PREFERENCES_ID, policy};
use ara_ops::Author;
use ara_proto::Endpoint;
use serde_json::json;

fn open(project: &Project) -> crate::Editor {
    crate::Editor::open_config(
        &project.0,
        None,
        ara_host::HostConfig {
            watch_every: std::time::Duration::ZERO,
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn rejected_project_edit_does_not_advance_private_settings_history() {
    let project = Project::new();
    let mut editor = open(&project);
    let begin = |editor: &mut crate::Editor, scopes| {
        Endpoint::call(&mut editor.host, "session.begin", json!({"client":"settings-permission-proof","author":"agent:restricted-settings","scopes":scopes})).unwrap()["session"].as_str().unwrap().to_owned()
    };
    let read = begin(&mut editor, vec!["read"]);
    let revision = editor.host.store().revision();
    let before = editor.state.settings.clone();
    let undo = editor.state.settings_undo.clone();
    let error = Endpoint::call(
        &mut editor.host,
        "editor.settings.set",
        json!({"session":read,"key":"build.profile","value":"rejected"}),
    )
    .unwrap_err();
    assert!(
        error.data.as_ref().unwrap()["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == ara_diag::codes::MISSING_SCOPE)
    );
    editor.action("editor.settings", json!({})).unwrap();
    assert_eq!(editor.state.settings, before);
    assert_eq!(editor.state.settings_undo, undo);
    assert_eq!(editor.host.store().revision(), revision);
    // The same author, now permitted to edit policy, still has no settings edit
    // to undo: permission rejection must not leave a hidden private transaction.
    let edit = begin(&mut editor, vec!["read", "edit:assets"]);
    assert!(
        Endpoint::call(
            &mut editor.host,
            "editor.settings.undo",
            json!({"session":edit})
        )
        .is_err()
    );
    assert_eq!(editor.host.store().revision(), revision);
    // A read-only content session retains its existing private preference access.
    Endpoint::call(
        &mut editor.host,
        "editor.settings.set",
        json!({"session":read,"key":"appearance.theme","value":"light"}),
    )
    .unwrap();
    Endpoint::call(
        &mut editor.host,
        "editor.settings.undo",
        json!({"session":read}),
    )
    .unwrap();
    assert_eq!(editor.host.store().revision(), revision);
}

#[test]
fn project_settings_commit_and_local_undo_keep_each_authors_policy_and_labels() {
    let project = Project::new();
    let mut editor = open(&project);
    let baseline = editor.host.store().revision();
    editor
        .action(
            "editor.settings.set",
            json!({"key":"play.tickHz","value":"30"}),
        )
        .unwrap();
    let title = editor.state.settings_undo.clone();
    let session = agent(&mut editor);
    Endpoint::call(
        &mut editor.host,
        "editor.settings.set",
        json!({"session":session,"key":"build.profile","value":"agent shipping"}),
    )
    .unwrap();
    editor.action("editor.settings.undo", json!({})).unwrap();
    assert_eq!(editor.state.settings_redo, title);
    let settings = policy(editor.host.store().workspace(), "settings").unwrap();
    assert_eq!(settings["play.tickHz"], "120");
    assert_eq!(settings["build.profile"], "agent shipping");
    assert_eq!(settings["plugin.option"], json!({"retained":true}));
    Endpoint::call(
        &mut editor.host,
        "editor.settings.undo",
        json!({"session":session}),
    )
    .unwrap();
    let settings = policy(editor.host.store().workspace(), "settings").unwrap();
    assert_eq!(settings["play.tickHz"], "120");
    assert_eq!(settings["build.profile"], "initial");
    let transactions = editor.host.store().since(baseline);
    assert_eq!(transactions.len(), 4);
    assert_eq!(
        transactions
            .iter()
            .map(|transaction| transaction.author.clone())
            .collect::<Vec<_>>(),
        vec![
            Author::Human,
            Author::Agent("policy".into()),
            Author::Human,
            Author::Agent("policy".into())
        ]
    );
    assert!(transactions.iter().all(|transaction| {
        transaction
            .ops
            .iter()
            .any(|op| op.doc().as_str() == PREFERENCES_ID)
    }));
    assert_eq!(
        transactions[0].label.as_deref(),
        Some("Set Fixed tick rate")
    );
    assert_eq!(
        transactions[2].label.as_deref(),
        Some("Undo Set Fixed tick rate")
    );
    let revision = editor.host.store().revision();
    editor.host.flush();
    for _ in 0..3 {
        editor.tick();
    }
    assert_eq!(
        editor.host.store().revision(),
        revision,
        "Owned persistence must not manufacture external-edit echoes"
    );
}
