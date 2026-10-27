//! Real read sessions, authority jobs and kernel diagnostics, with isolated disk fixtures.
use super::*;
use ara_host::{Host, HostConfig};
use ara_proto::ConnId;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Fixture {
    // The host releases its file lock before Scratch removes the owned directory.
    host: Host,
    scratch: Scratch,
    read: String,
    edit: String,
}
impl Fixture {
    fn new(count: usize, invalid: bool) -> Self {
        let root = crate::test_target()
            .join("reachability-validation")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(root.parent().unwrap()).unwrap();
        std::fs::create_dir(&root).unwrap();
        let content = root.join(ara_doc::AUTHORING_ROOT);
        std::fs::create_dir_all(content.join("levels")).unwrap();
        std::fs::write(root.join("ara.project.json"),ara_json::to_canonical_string(&json!({"name":"Validation job tests","layoutVersion":3,"main":"levels/main.scene"}))).unwrap();
        let records=(0..count).map(|index| {
            let record=if invalid {json!({"name":"Invalid validation source","Transform":{"position":"wrong-type"},"FollowCamera":{"target":"e_missing0000"}})}
                else {json!({"name":"Validation source","Transform":{}})};
            (format!("e_validation{index:04}"),record)
        }).collect::<serde_json::Map<_,_>>();
        std::fs::write(
            content.join("levels/main.scene"),
            ara_json::to_canonical_string(
                &json!({"$schema":"ara://schema/scene/1","id":"sc_validate01","records":records}),
            ),
        )
        .unwrap();
        let project = ara_play_host::open_project(&root).unwrap();
        let mut host = Host::open(
            project,
            HostConfig {
                seed: Some(71),
                watch_every: Duration::from_secs(3600),
                scan_every: Duration::from_secs(3600),
                ..HostConfig::default()
            },
        )
        .unwrap();
        host.register(Box::new(ValidationService::default()))
            .unwrap();
        let read = host
            .call(
                ConnId::LOCAL,
                "session.begin",
                json!({"client":"validation-read-test","author":"human","scopes":["read"]}),
            )
            .unwrap();
        assert_eq!(read["granted"], json!(["read"]));
        let edit=host.call(ConnId::LOCAL,"session.begin",json!({"client":"validation-edit-test","author":"human","scopes":["read","edit:scene"]})).unwrap();
        Self {
            host,
            scratch: Scratch(root),
            read: read["session"].as_str().unwrap().to_owned(),
            edit: edit["session"].as_str().unwrap().to_owned(),
        }
    }
    fn read(&mut self, method: &str, mut params: Value) -> Value {
        params["session"] = self.read.clone().into();
        self.host.call(ConnId::LOCAL, method, params).unwrap()
    }
    fn start(&mut self) -> String {
        self.read("editor.validate", json!({}))["job"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    fn status(&mut self, job: &str) -> Value {
        self.read("jobs.status", json!({"job":job}))
    }
    fn digest(&self) -> blake3::Hash {
        let mut hash = blake3::Hasher::new();
        for (path, doc) in self.host.store().workspace().docs() {
            hash.update(path.as_bytes());
            hash.update(&[0]);
            hash.update(ara_doc::write(doc, self.host.store().schema()).as_bytes());
            hash.update(&[0]);
        }
        hash.finalize()
    }
    fn phase(&mut self, job: &str, wanted: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            self.host.tick();
            assert_eq!(self.status(job)["status"], "running");
            let active = self.read("editor.validate", json!({}));
            assert_eq!(active["job"], job, "one job must serve repeated requests");
            if active["phase"] == wanted {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("Validation did not reach {wanted}");
    }
    fn complete(&mut self, job: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            self.host.tick();
            let status = self.status(job);
            if status["status"] != "running" {
                return status;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("Validation did not complete");
    }
    fn edit_name(&mut self) {
        self.host.call(ConnId::LOCAL,"cmd.apply",json!({"session":self.edit,"doc":"levels/main.scene","why":"Concurrent author edit in validation test",
            "commands":[{"cmd":"property.set","args":{"entity":"e_validation0000","path":"name","value":"Changed while validating"}}]})).unwrap();
    }
    fn validation_logs(&mut self) -> Value {
        self.read(
            "logs",
            json!({"text":"Validation at revision","limit":1000,"group":false}),
        )
    }
}

#[test]
fn read_session_checks_real_schema_and_references_with_bounded_diagnostic_publication() {
    let mut fixture = Fixture::new(300, true);
    let before = fixture.digest();
    let revision = fixture.host.store().revision();
    let history = fixture.host.store().history().len();
    let path = fixture
        .scratch
        .0
        .join(ara_doc::AUTHORING_ROOT)
        .join("levels/main.scene");
    let disk = std::fs::read(&path).unwrap();
    let job = fixture.start();
    fixture.phase(&job, "publishing");
    assert_eq!(fixture.validation_logs()["total"], 0);
    fixture.host.tick();
    let first = fixture.validation_logs()["entries"]
        .as_array()
        .unwrap()
        .len();
    assert!(
        (1..=64).contains(&first),
        "one publication tick is bounded: {first}"
    );
    let status = fixture.complete(&job);
    assert_eq!(status["status"], "done");
    assert_eq!(status["result"]["stale"], false);
    assert!(status["result"]["errors"].as_u64().unwrap() > 0);
    assert!(status["result"]["warnings"].as_u64().unwrap() > 0);
    let logs = fixture.validation_logs();
    let codes = logs["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["diagnostic"]["code"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(codes.contains(&ara_diag::codes::TYPE_MISMATCH.to_string().as_str()));
    assert!(codes.contains(&ara_diag::codes::DANGLING_ENTITY_REF.to_string().as_str()));
    assert_eq!(status["result"]["diagnosticsPublished"], logs["total"]);
    assert_eq!(fixture.digest(), before);
    assert_eq!(fixture.host.store().revision(), revision);
    assert_eq!(fixture.host.store().history().len(), history);
    assert_eq!(std::fs::read(path).unwrap(), disk);
}

#[test]
fn cancel_during_incremental_preparation_does_not_validate_or_mutate_authoring() {
    let mut fixture = Fixture::new(1024, false);
    let before = fixture.digest();
    let revision = fixture.host.store().revision();
    let job = fixture.start();
    fixture.phase(&job, "preparing");
    fixture.host.tick();
    assert!(fixture.status(&job)["progress"].as_f64().unwrap() < 0.25);
    fixture.read("jobs.cancel", json!({"job":job}));
    let status = fixture.complete(&job);
    assert_eq!(status["status"], "cancelled");
    assert!(status["result"].is_null());
    assert_eq!(fixture.validation_logs()["total"], 0);
    assert_eq!(fixture.digest(), before);
    assert_eq!(fixture.host.store().revision(), revision);
    assert!(fixture.host.store().history().is_empty());
}

#[test]
fn an_authoring_revision_change_aborts_partial_snapshot_instead_of_mixing_versions() {
    let mut fixture = Fixture::new(1024, false);
    let job = fixture.start();
    fixture.phase(&job, "preparing");
    fixture.host.tick();
    fixture.edit_name();
    let after_edit = fixture.digest();
    let status = fixture.complete(&job);
    assert_eq!(status["status"], "failed");
    assert!(
        status["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["message"].as_str().unwrap().contains("snapshot"))
    );
    assert_eq!(fixture.digest(), after_edit);
    assert_eq!(fixture.host.store().history().len(), 1);
    assert_eq!(fixture.validation_logs()["total"], 0);
}

#[test]
fn a_completed_snapshot_is_reported_stale_if_authoring_changes_during_worker_execution() {
    let mut fixture = Fixture::new(256, false);
    let revision = fixture.host.store().revision();
    let job = fixture.start();
    fixture.phase(&job, "checking");
    fixture.edit_name();
    let after_edit = fixture.digest();
    let status = fixture.complete(&job);
    assert_eq!(status["status"], "done");
    assert_eq!(status["result"]["revision"], revision);
    assert_eq!(
        status["result"]["currentRevision"],
        fixture.host.store().revision()
    );
    assert_eq!(status["result"]["stale"], true);
    assert_eq!(fixture.digest(), after_edit);
    assert_eq!(fixture.host.store().history().len(), 1);
}
