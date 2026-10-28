use super::*;
use ara_host::{Host, HostConfig, Project};
use ara_runtime_render::probes::{
    ProbeFailure, ProbeJobStatus, ProbeReceipt, ProbeRow, ProbeSnapshot, RenderCapabilities,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) struct Task {
    pub job: String,
    pub session: String,
    pub doc: String,
    pub entity: String,
    pub source_revision: u64,
}
impl Task {
    pub fn binding(&self) -> SceneBinding {
        SceneBinding {
            doc: self.doc.clone(),
            scene_revision: 7,
            source_revision: self.source_revision,
        }
    }
}

pub(super) struct Fixture {
    pub host: Host,
    pub bridge: Bridge,
    pub session: String,
    root: PathBuf,
}
impl Fixture {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::path::absolute(crate::test_target())
            .unwrap()
            .join("probe-job-service")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
        std::fs::write(root.join("ara.project.json"), ara_json::to_canonical_string(&json!({
            "layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Probe job bridge","main":"levels/main.scene"
        }))).unwrap();
        for (path, id) in [
            ("main.scene", "sc_probejobs"),
            ("other.scene", "sc_otherjobs"),
        ] {
            std::fs::write(
                root.join(ara_doc::AUTHORING_ROOT).join("levels").join(path),
                ara_json::to_canonical_string(
                    &json!({"$schema":"ara://schema/scene/1","id":id,"records":{}}),
                ),
            )
            .unwrap();
        }
        let mut host = Host::open(
            Project::open(&root).unwrap(),
            HostConfig {
                seed: Some(7391),
                debounce: std::time::Duration::from_secs(3600),
                ..HostConfig::default()
            },
        )
        .unwrap();
        let (service, bridge) = channel();
        host.register(Box::new(service)).unwrap();
        let session = host
            .call(
                ara_proto::ConnId::LOCAL,
                "session.begin",
                json!({"client":"probe bridge CPU owner test","author":"agent:probe-test"}),
            )
            .unwrap()["session"]
            .as_str()
            .unwrap()
            .to_owned();
        Self {
            host,
            bridge,
            session,
            root,
        }
    }
    pub fn call(&mut self, method: &str, mut params: serde_json::Value) -> serde_json::Value {
        if params.get("session").is_none() {
            params["session"] = json!(self.session);
        }
        self.host
            .call(ara_proto::ConnId::LOCAL, method, params)
            .unwrap_or_else(|error| panic!("{method}: {error}"))
    }
    pub fn begin(&mut self, author: &str) -> String {
        self.host
            .call(
                ara_proto::ConnId::LOCAL,
                "session.begin",
                json!({"client":"second probe owner","author":author}),
            )
            .unwrap()["session"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    pub fn queue(&mut self, entity: &str) -> Task {
        let session = self.session.clone();
        self.queue_for(&session, entity)
    }
    pub fn queue_for(&mut self, session: &str, entity: &str) -> Task {
        let receipt = self.call("render.probe.recapture", json!({"session":session,"doc":"sc_probejobs","entities":[entity],"why":"Derived CPU bridge verification"}));
        assert_eq!(receipt["ops"], json!([]));
        assert!(receipt["txn"].is_null());
        match self.bridge.try_recv().expect("queued owner request") {
            Request::Recapture {
                job,
                session,
                doc,
                entities,
                source_revision,
                why,
            } => {
                assert_eq!(job, receipt["job"]);
                assert_eq!(why.as_deref(), Some("Derived CPU bridge verification"));
                Task {
                    job,
                    session,
                    doc,
                    entity: entities[0].clone(),
                    source_revision,
                }
            }
            request => panic!("expected Recapture, got {request:?}"),
        }
    }
    pub fn started(&mut self, task: &Task, backend: u64) {
        assert!(
            self.bridge
                .send(Event::Started {
                    job: task.job.clone(),
                    session: task.session.clone(),
                    binding: task.binding(),
                    backend: ProbeJob(backend)
                })
                .is_ok()
        );
        self.host.tick();
    }
    pub fn status(
        &mut self,
        task: &Task,
        backend: u64,
        finished: bool,
        cancellable: bool,
        progress: f32,
    ) {
        assert!(
            self.bridge
                .send(Event::Status {
                    job: task.job.clone(),
                    session: task.session.clone(),
                    binding: task.binding(),
                    status: status(task, backend, finished, cancellable, progress)
                })
                .is_ok()
        );
        self.host.tick();
    }
    pub fn job(&mut self, job: &str) -> serde_json::Value {
        self.call("jobs.status", json!({"job":job}))
    }
    pub fn cancel(&mut self, task: &Task) -> serde_json::Value {
        self.call(
            "render.probe.cancel",
            json!({"session":task.session,"job":task.job}),
        )
    }
    pub fn query(&mut self, doc: Option<&str>, since: Option<u64>) -> serde_json::Value {
        self.call("render.probes.query", json!({"doc":doc,"since":since}))
    }
    pub fn snapshot(&mut self, version: u64) {
        let task = Task {
            job: String::new(),
            session: self.session.clone(),
            doc: "sc_probejobs".into(),
            entity: "e_probe".into(),
            source_revision: self.host.store().revision(),
        };
        assert!(
            self.bridge
                .send(Event::Snapshot {
                    binding: task.binding(),
                    snapshot: snapshot(&task, version)
                })
                .is_ok()
        );
        self.host.tick();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Host owns open files until its own drop. Leave the isolated test output
        // under target for the parent gate to inspect; never touch a real profile.
        assert!(
            self.root.starts_with(
                std::path::absolute(crate::test_target())
                    .unwrap()
                    .join("probe-job-service")
            )
        );
    }
}

pub(super) fn receipt(task: &Task) -> ProbeReceipt {
    ProbeReceipt {
        doc: task.doc.clone(),
        entity: task.entity.clone(),
        generation: 3,
        source_revision: task.binding().scene_revision,
        source_hash: "source-digest".into(),
        capture_hash: "capture-digest".into(),
        resolution: 256,
        faces: 6,
        cache_path: ".ara/cache/probes/capture-digest".into(),
        published: true,
    }
}
pub(super) fn status(
    task: &Task,
    backend: u64,
    finished: bool,
    cancellable: bool,
    progress: f32,
) -> ProbeJobStatus {
    ProbeJobStatus {
        job: ProbeJob(backend),
        finished,
        progress: Some(progress),
        message: if finished {
            "Published six filtered faces"
        } else {
            "Capturing faces: 3 of 6"
        }
        .into(),
        cancellable,
        receipts: if finished {
            vec![receipt(task)]
        } else {
            Vec::new()
        },
        failures: Vec::<ProbeFailure>::new(),
        diagnostics: Vec::new(),
    }
}
pub(super) fn snapshot(task: &Task, version: u64) -> ProbeSnapshot {
    ProbeSnapshot {
        version,
        scene_revision: 7,
        capabilities: RenderCapabilities {
            reflection_probes: true,
            gi_diffuse: false,
            overdraw: false,
            reasons: [
                ("giDiffuse".into(), "Not available on this owner".into()),
                ("overdraw".into(), "Not available on this owner".into()),
            ]
            .into(),
        },
        probes: vec![ProbeRow {
            doc: task.doc.clone(),
            entity: task.entity.clone(),
            enabled: true,
            generation: 3,
            state: ProbeState::Done,
            stage: None,
            progress: Some(1.),
            completed_work: Some(6),
            total_work: Some(6),
            cancellable: false,
            scene_revision: 7,
            source_hash: "source-digest".into(),
            stale: false,
            retained_previous: false,
            last_receipt: Some(receipt(task)),
            diagnostics: Vec::new(),
        }],
    }
}
