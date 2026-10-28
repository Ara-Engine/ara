//! Real asset operations must not mistake their restored native bytes for human edits.
use super::{AssetService, ProcessWorker, begin, call, complete, scratch, write_project};
use ara_host::{Host, HostConfig, Project};
use serde_json::{Value, json};
use std::{fs, sync::Arc, time::Duration};

#[test]
fn native_asset_rename_move_and_delete_keep_redo_after_canonical_default_echoes() {
    for (command, args, destination) in [
        (
            "asset.rename",
            json!({"source":"scenes/prop.scene","name":"renamed.scene"}),
            "scenes/renamed.scene",
        ),
        (
            "asset.move",
            json!({"source":"scenes/prop.scene","destination":"props/moved.scene"}),
            "props/moved.scene",
        ),
        ("asset.delete", json!({"source":"scenes/prop.scene"}), ""),
    ] {
        let scratch = scratch();
        write_project(scratch.path());
        let authored = scratch.path().join(ara_doc::AUTHORING_ROOT);
        let source = authored.join("scenes/prop.scene");
        // Snapshots retain these explicit defaults; native document relocation uses
        // the canonical sparse form. Undo must reconcile those as the same state.
        fs::write(
            &source,
            ara_json::to_canonical_string(
                &json!({"$schema":"ara://schema/scene/1","id":"sc_prop","records":{
                    "e_prop":{"name":"Prop","Transform":{"position":[0,0,0],"scale":[1,1,1]}}
                }}),
            ),
        )
        .expect("native scene with explicit defaults");
        let mut host = Host::open(
            Project::open(scratch.path()).expect("project"),
            HostConfig {
                debounce: Duration::from_secs(3600),
                watch_every: Duration::ZERO,
                scan_every: Duration::ZERO,
                seed: Some(942),
                ..HostConfig::default()
            },
        )
        .expect("host");
        host.register(Box::new(AssetService::new(
            ProcessWorker::new(scratch.path().join("must-not-run-importer")),
            Arc::new(|_, _| Ok(())),
        )))
        .expect("asset service");
        let session = begin(&mut host, None);
        let started = call(
            &mut host,
            "cmd.apply",
            json!({"session":session,"why":"Organize native scene","commands":[{"cmd":command,"args":args}]}),
        );
        complete(
            &mut host,
            &session,
            started["job"].as_str().expect("asset preparation"),
        );
        host.flush();
        let undone = call(&mut host, "history.undo", json!({"session":session}));
        if let Some(job) = undone["job"].as_str() {
            complete(&mut host, &session, job);
        }
        host.flush();
        let revision = host.store().revision();
        for _ in 0..3 {
            host.tick();
        }
        assert_eq!(
            host.store().revision(),
            revision,
            "{command}: our restored defaults became an external edit"
        );
        assert!(source.is_file(), "{command}: Undo restores the source");
        let history = call(&mut host, "history.list", json!({"session":session}));
        assert!(
            !history["items"]
                .as_array()
                .expect("history")
                .iter()
                .any(|item| item["label"] == "external edit"),
            "{command}: {history}"
        );
        let redone = call(&mut host, "history.redo", json!({"session":session}));
        if let Some(job) = redone["job"].as_str() {
            complete(&mut host, &session, job);
        }
        host.flush();
        for _ in 0..3 {
            host.tick();
        }
        assert!(!source.exists(), "{command}: Redo removes the old path");
        if !destination.is_empty() {
            assert!(
                authored.join(destination).is_file(),
                "{command}: Redo restores the new path"
            );
        }
        let undone = call(&mut host, "history.undo", json!({"session":session}));
        if let Some(job) = undone["job"].as_str() {
            complete(&mut host, &session, job);
        }
        host.flush();
        assert!(
            source.is_file(),
            "{command}: another Undo restores the source"
        );
        let parsed: Value =
            serde_json::from_str(&fs::read_to_string(&source).expect("restored scene"))
                .expect("scene JSON");
        assert_eq!(parsed["records"]["e_prop"]["name"], "Prop");
    }
}
