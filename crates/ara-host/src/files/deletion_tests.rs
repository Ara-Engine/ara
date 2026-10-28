//! Delayed watch observations must not unload another path/incarnation of an ID.
#![allow(clippy::unwrap_used)]
use super::*;
use crate::{Host, HostConfig, Project};
use ara_proto::ConnId;
use serde_json::json;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

struct Fixture {
    host: Host,
    root: PathBuf,
    session: String,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::var_os("ARA_TEST_ROOT")
            .or_else(|| std::env::var_os("CARGO_TARGET_DIR"))
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("host-deletion-order")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("props")).unwrap();
        fs::write(root.join("ara.project.json"), ara_json::to_canonical_string(&json!({
            "layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Deletion ordering","main":"main.scene"
        }))).unwrap();
        for (path, kind, id) in [
            ("main.scene", "scene", "sc_deletion"),
            ("props/A.prefab", "prefab", "pf_prop"),
        ] {
            fs::write(
                root.join(ara_doc::AUTHORING_ROOT).join(path),
                ara_json::to_canonical_string(&json!({
                    "$schema":format!("ara://schema/{kind}/1"),"id":id,"records":{}
                })),
            )
            .unwrap();
        }
        let mut host = Host::open(
            Project::open(&root).unwrap(),
            HostConfig {
                debounce: Duration::from_secs(3600),
                watch_every: Duration::from_secs(3600),
                scan_every: Duration::from_secs(3600),
                ..HostConfig::default()
            },
        )
        .unwrap();
        let session = host
            .call(
                ConnId::LOCAL,
                "session.begin",
                json!({"client":"deletion-order","author":"human"}),
            )
            .unwrap()["session"]
            .as_str()
            .unwrap()
            .to_owned();
        host.flush();
        Self {
            host,
            root,
            session,
        }
    }
    fn storage(path: &str) -> String {
        format!("{}/{path}", ara_doc::AUTHORING_ROOT)
    }
    fn recreate(&mut self, destination: &str) {
        self.host
            .call(
                ConnId::LOCAL,
                "cmd.apply",
                json!({"session":self.session,"doc":"ara.project.json",
            "why":"Recreate the same document identity through typed lifecycle commands",
            "commands":[{"cmd":"doc.delete","args":{"document":"@props/A.prefab#pf_prop"}},
                {"cmd":"doc.create","args":{"path":destination,"kind":"prefab","id":"pf_prop"}}]}),
            )
            .unwrap();
    }
    fn assert_path(&self, path: &str) {
        assert!(self.host.store().workspace().doc("pf_prop").is_some());
        assert_eq!(
            self.host.store().workspace().path_of("pf_prop"),
            Some(Self::storage(path).as_str())
        );
    }
}
#[test]
fn delayed_deleted_old_path_before_saved_keeps_relocated_document_model() {
    let mut f = Fixture::new();
    let before = f.host.store().revision();
    f.recreate("props/Renamed.prefab");
    f.assert_path("props/Renamed.prefab");
    f.host.flush();
    f.assert_path("props/Renamed.prefab");
    f.host
        .call(ConnId::LOCAL, "history.undo", json!({"session":f.session}))
        .unwrap();
    f.assert_path("props/A.prefab");
    f.host.flush();
    f.assert_path("props/A.prefab");
    assert_eq!(f.host.store().revision(), before + 2);
    let old = Fixture::storage("props/A.prefab");
    let prior_stamp = crate::file_io::stamp(&f.root.join(&old));
    let delayed = f
        .host
        .file_test_state()
        .files
        .io
        .deletion_observation(&old, prior_stamp.clone());
    let held = f.host.file_test_state().files.io.hold();
    f.host
        .call(ConnId::LOCAL, "history.redo", json!({"session":f.session}))
        .unwrap();
    f.assert_path("props/Renamed.prefab");
    let revision = f.host.store().revision();
    assert_eq!(revision, before + 3);
    f.host.file_test_state().file_reply(Reply::Deleted {
        path: old.clone(),
        observed: delayed,
    });
    // Even a current watch token for the old path cannot address its ID's new path.
    let current_old = f
        .host
        .file_test_state()
        .files
        .io
        .deletion_observation(&old, prior_stamp);
    f.host.file_test_state().file_reply(Reply::Deleted {
        path: old,
        observed: current_old,
    });
    f.assert_path("props/Renamed.prefab");
    let _receipt = f.host.queue_flush();
    assert!(f.host.unsaved());
    drop(held);
    f.host.flush();
    f.assert_path("props/Renamed.prefab");
    assert_eq!(f.host.store().revision(), revision);
}

#[test]
fn stale_same_path_deletion_cannot_unload_recreated_identity_but_fresh_external_delete_can() {
    let mut f = Fixture::new();
    let path = Fixture::storage("props/A.prefab");
    let delayed = f
        .host
        .file_test_state()
        .files
        .io
        .deletion_observation(&path, crate::file_io::stamp(&f.root.join(&path)));
    let held = f.host.file_test_state().files.io.hold();
    f.recreate("props/A.prefab");
    let revision = f.host.store().revision();
    f.host.file_test_state().file_reply(Reply::Deleted {
        path: path.clone(),
        observed: delayed,
    });
    f.assert_path("props/A.prefab");
    drop(held);
    f.host.flush();
    f.assert_path("props/A.prefab");
    fs::remove_file(f.root.join(&path)).unwrap();
    f.host.file_test_state().files.io.sync();
    let deadline = Instant::now() + Duration::from_secs(5);
    while f.host.store().workspace().doc("pf_prop").is_some() {
        f.host.tick();
        assert!(
            Instant::now() < deadline,
            "a current genuine deletion must still unload the model"
        );
        std::thread::yield_now();
    }
    assert_eq!(
        f.host.store().revision(),
        revision,
        "watch reconciliation never invents an authoring txn"
    );
}
