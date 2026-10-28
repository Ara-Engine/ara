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
            .any(|diagnostic| diagnostic["code"] == json!(ara_diag::codes::MISSING_SCOPE))
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
fn another_authors_round_trip_still_reports_an_undo_conflict_without_advancing_history() {
    let project = Project::new();
    let mut editor = open(&project);
    editor
        .action(
            "editor.settings.set",
            json!({"key":"play.clients","value":4}),
        )
        .unwrap();
    let session = agent(&mut editor);
    for value in [8, 4] {
        Endpoint::call(
            &mut editor.host,
            "editor.settings.set",
            json!({
                "session":session,"key":"play.clients","value":value
            }),
        )
        .unwrap();
    }
    assert_settings_history_conflict(&mut editor, false);
}

#[test]
fn direct_command_round_trip_still_blocks_settings_undo_without_private_agent_history() {
    let project = Project::new();
    let mut editor = open(&project);
    editor
        .action(
            "editor.settings.set",
            json!({"key":"play.clients","value":4}),
        )
        .unwrap();
    let session = agent(&mut editor);
    super::live_tests::apply(
        &mut editor,
        &session,
        &[
            ("settings.settings[play.clients]", json!(8)),
            ("settings.settings[play.clients]", json!(4)),
        ],
    );
    assert_settings_history_conflict(&mut editor, false);
}

#[test]
fn direct_command_round_trip_still_blocks_settings_redo_at_the_undo_revision() {
    let project = Project::new();
    let mut editor = open(&project);
    editor
        .action(
            "editor.settings.set",
            json!({"key":"build.profile","value":"human"}),
        )
        .unwrap();
    editor.action("editor.settings.undo", json!({})).unwrap();
    let session = agent(&mut editor);
    super::live_tests::apply(
        &mut editor,
        &session,
        &[
            (
                "settings.settings[build.profile]",
                json!("agent intermediate"),
            ),
            ("settings.settings[build.profile]", json!("initial")),
        ],
    );
    assert_settings_history_conflict(&mut editor, true);
}

fn assert_settings_history_conflict(editor: &mut crate::Editor, is_redo: bool) {
    editor.action("editor.settings", json!({})).unwrap();
    let revision = editor.host.store().revision();
    let history = editor.host.store().history().len();
    let canonical = policy(editor.host.store().workspace(), "settings").cloned();
    let settings = editor.state.settings.clone();
    let undo = editor.state.settings_undo.clone();
    let redo = editor.state.settings_redo.clone();
    let error = editor
        .action("editor.settings.undo", json!({"redo":is_redo}))
        .unwrap_err();
    assert!(
        error.0.data.as_ref().unwrap()["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == json!(ara_diag::codes::UNDO_CONFLICT)),
        "Overlapping later intent remains an Undo conflict even when its final value matches: {error}"
    );
    editor.action("editor.settings", json!({})).unwrap();
    assert_eq!(editor.host.store().revision(), revision);
    assert_eq!(editor.host.store().history().len(), history);
    assert_eq!(
        policy(editor.host.store().workspace(), "settings"),
        canonical.as_ref()
    );
    assert_eq!(editor.state.settings, settings);
    assert_eq!(editor.state.settings_undo, undo);
    assert_eq!(editor.state.settings_redo, redo);
}

#[test]
fn project_settings_undo_redo_link_each_inverse_to_its_own_host_revision() {
    let project = Project::new();
    let mut editor = open(&project);
    editor
        .action(
            "editor.settings.set",
            json!({"key":"build.profile","value":"human"}),
        )
        .unwrap();
    let title = editor.state.settings_undo.clone();
    for redo in [false, true, false, true] {
        let base = editor.host.store().revision();
        editor
            .action("editor.settings.undo", json!({"redo":redo}))
            .unwrap();
        let transaction = editor.host.store().history().last().unwrap();
        assert_eq!(
            transaction.base, base,
            "Each private inverse uses its linked Host transaction, not the original Set"
        );
        assert_eq!(transaction.result, base + 1);
        assert_eq!(
            editor.state.settings["build.profile"],
            if redo { "human" } else { "initial" }
        );
        assert_eq!(
            if redo {
                &editor.state.settings_undo
            } else {
                &editor.state.settings_redo
            },
            &title
        );
    }
}

#[test]
fn sequential_project_settings_undo_redo_skips_its_cancelled_local_pairs() {
    let project = Project::new();
    let mut editor = open(&project);
    for value in [2, 4] {
        editor
            .action(
                "editor.settings.set",
                json!({"key":"play.clients","value":value}),
            )
            .unwrap();
    }
    for (redo, expected) in [(false, 2), (false, 1), (true, 2), (true, 4)] {
        let base = editor.host.store().revision();
        editor
            .action("editor.settings.undo", json!({"redo":redo}))
            .unwrap();
        assert_eq!(editor.state.settings["play.clients"], expected);
        let transaction = editor.host.store().history().last().unwrap();
        assert_eq!(transaction.base, base);
        assert_eq!(transaction.result, base + 1);
    }
}

#[test]
fn direct_same_value_intent_still_blocks_older_undo_after_a_later_local_edit_is_undone() {
    let project = Project::new();
    let mut editor = open(&project);
    editor
        .action(
            "editor.settings.set",
            json!({"key":"play.clients","value":2}),
        )
        .unwrap();
    let session = agent(&mut editor);
    let before = editor.host.store().revision();
    super::live_tests::apply(
        &mut editor,
        &session,
        &[("settings.settings[play.clients]", json!(2))],
    );
    let intent = editor.host.store().since(before);
    assert_eq!(
        intent.len(),
        1,
        "The fixture requires a recorded equal-value command"
    );
    assert!(intent[0].ops.iter().any(|op| matches!(op,
        ara_doc::Op::Set { value:Some(value),prev:Some(previous),.. } if value==previous && value==&json!(2))));
    editor
        .action(
            "editor.settings.set",
            json!({"key":"play.clients","value":4}),
        )
        .unwrap();
    editor.action("editor.settings.undo", json!({})).unwrap();
    assert_eq!(editor.state.settings["play.clients"], 2);
    assert_settings_history_conflict(&mut editor, false);
}

#[test]
fn host_native_cancelled_command_pairs_do_not_block_older_settings_undo() {
    let project = Project::new();
    let mut editor = open(&project);
    editor
        .action(
            "editor.settings.set",
            json!({"key":"play.clients","value":2}),
        )
        .unwrap();
    let session = agent(&mut editor);
    super::live_tests::apply(
        &mut editor,
        &session,
        &[("settings.settings[play.clients]", json!(4))],
    );
    Endpoint::call(&mut editor.host, "history.undo", json!({"session":session})).unwrap();
    editor.action("editor.settings.undo", json!({})).unwrap();
    assert_eq!(editor.state.settings["play.clients"], 1);
    assert!(
        policy(editor.host.store().workspace(), "settings")
            .unwrap()
            .get("play.clients")
            .is_none()
    );
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
