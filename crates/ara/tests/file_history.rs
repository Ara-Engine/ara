//! Real CLI processes must redo batches whose undo removed a newly created folder.
use ara_diag::Diagnostic;
use ara_doc::{AssetId, Edit, file_state::FileState};
use ara_host::{Cx, Host, HostConfig, Method, Project, Service, file_state::FileHistory};
use ara_proto::{ConnId, Scope};
use ara_schema::SchemaDb;
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

struct FileBatch(FileState);
impl Service for FileBatch {
    fn name(&self) -> &'static str {
        "fixture"
    }
    fn methods(&self, db: &mut SchemaDb) -> Vec<Method> {
        vec![
            Method::new::<ara_host::params::SessionParams>(
                db,
                "fixture.batch",
                "Reversible file fixture batch.",
            )
            .scope(Scope::EditAssets),
        ]
    }
    fn call(&mut self, cx: &mut Cx<'_>, _: &str, _: Value) -> Result<Value, Vec<Diagnostic>> {
        let state = self.0.clone();
        cx.observe_files([("drop/note.txt".into(), Some(state.clone()))]);
        let op = |path: &str, base, state| Edit::File {
            doc: AssetId::parse("sc_files").expect("fixture anchor"),
            path: path.into(),
            base,
            state,
        };
        // Five logical commands, one transaction: folder, move, rename, duplicate, delete.
        cx.transact(
            Some("Exercise one reversible file batch".into()),
            |store, meta| {
                store.transact(
                    meta,
                    [
                        op("organized", None, Some(FileState::directory())),
                        op("drop/note.txt", Some(state.clone()), None),
                        op("organized/note.txt", None, Some(state.clone())),
                        op("organized/note.txt", Some(state.clone()), None),
                        op("organized/renamed.txt", None, Some(state.clone())),
                        op("organized/copy.txt", None, Some(state.clone())),
                        op("organized/copy.txt", Some(state), None),
                    ],
                )?;
                Ok(json!({"ok":true}))
            },
        )?
    }
}

#[test]
#[ignore = "invoked in its own process by the parent regression"]
fn file_batch_fixture_child() {
    let root = std::env::var_os("ARA_FILE_FIXTURE_ROOT").expect("child root");
    let root = Path::new(&root);
    let state = FileHistory::new(root)
        .snapshot("drop/note.txt")
        .expect("snapshot")
        .expect("source");
    let mut host =
        Host::open(Project::open(root).expect("project"), HostConfig::default()).expect("host");
    host.register(Box::new(FileBatch(state)))
        .expect("typed fixture service");
    let session = host
        .call(
            ConnId::LOCAL,
            "session.begin",
            json!({"client":"file-roundtrip","author":"agent:file-roundtrip"}),
        )
        .expect("session")["session"]
        .as_str()
        .expect("id")
        .to_owned();
    host.call(ConnId::LOCAL, "fixture.batch", json!({"session":session}))
        .expect("one transaction");
    assert_eq!(host.store().history().len(), 1);
    host.flush();
}

#[test]
fn created_folder_batch_round_trips_undo_redo_undo_in_separate_cli_processes() {
    let root = std::env::temp_dir().join(format!("ara-file-parent-redo-{}", std::process::id()));
    fs::create_dir_all(root.join("drop")).expect("drop directory");
    fs::create_dir_all(root.join("content")).expect("authored root");
    for (path, value) in [
        (
            "ara.project.json",
            json!({"name":"File history","main":"main.scene","layoutVersion":4}),
        ),
        (
            "content/main.scene",
            json!({"$schema":"ara://schema/scene/1","id":"sc_files","records":{}}),
        ),
    ] {
        fs::write(root.join(path), ara_json::to_canonical_string(&value)).expect("fixture");
    }
    fs::write(root.join("drop/note.txt"), b"hello file history").expect("source");
    let batch = Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "--ignored",
            "--exact",
            "file_batch_fixture_child",
            "--nocapture",
        ])
        .env("ARA_FILE_FIXTURE_ROOT", &root)
        .env("ARA_CONFIG_DIR", root.join(".ara/cli-profile"))
        .output()
        .expect("batch process");
    assert!(
        batch.status.success(),
        "{}{}",
        String::from_utf8_lossy(&batch.stdout),
        String::from_utf8_lossy(&batch.stderr)
    );
    assert!(!root.join("drop/note.txt").exists());
    assert_eq!(
        fs::read(root.join("organized/renamed.txt")).expect("moved and renamed"),
        b"hello file history"
    );
    assert!(!root.join("organized/copy.txt").exists());
    for (operation, restored) in [("undo", true), ("redo", false), ("undo", true)] {
        let output = Command::new(env!("CARGO_BIN_EXE_ara"))
            .current_dir(&root)
            .env("ARA_CONFIG_DIR", root.join(".ara/cli-profile"))
            .args(["--author", "agent:file-roundtrip", operation, "--json"])
            .output()
            .expect("separate CLI process");
        assert!(
            output.status.success(),
            "{operation}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if restored {
            assert_eq!(
                fs::read(root.join("drop/note.txt")).expect("restored source"),
                b"hello file history"
            );
            assert!(
                !root.join("organized").exists(),
                "undo removes the created folder"
            );
        } else {
            assert!(!root.join("drop/note.txt").exists());
            assert_eq!(
                fs::read(root.join("organized/renamed.txt")).expect("redone destination"),
                b"hello file history"
            );
            assert!(!root.join("organized/copy.txt").exists());
        }
    }
}
