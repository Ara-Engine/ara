//! Controlled callback replies exercise the production authority completion path.
use super::*;
use ara_host::{Host, HostConfig};
use ara_proto::ConnId;
use std::{path::PathBuf, sync::atomic::AtomicU64};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let root = crate::test_target()
            .join("extension-navigation")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        assert!(!root.exists());
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
        std::fs::write(
            root.join("ara.project.json"),
            ara_json::to_canonical_string(
                &json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Callback ownership","main":"levels/first.scene"}),
            ),
        )
        .unwrap();
        for (path, id, entity, name) in [
            ("levels/first.scene", "sc_owner01", "e_owner001", "Original"),
            ("levels/second.scene", "sc_owner02", "e_other001", "Other"),
        ] {
            std::fs::write(
                root.join(ara_doc::AUTHORING_ROOT).join(path),
                ara_json::to_canonical_string(&json!({"$schema":"ara://schema/scene/1","id":id,"records":{entity:{"name":name,"Transform":{}}}})),
            ).unwrap();
        }
        Self(root)
    }

    fn open(&self) -> Harness {
        let mut project = ara_play_host::open_project(&self.0).unwrap();
        crate::authoring::register(&mut project.registry, &mut project.db);
        crate::commands::register(&mut project.registry, &mut project.db);
        let mut host = Host::open(project, HostConfig::default()).unwrap();
        let interaction = crate::interaction::Interaction::default();
        let (requests, incoming) = mpsc::channel();
        let (send, completed) = mpsc::channel();
        let (changes, updates) = mpsc::channel();
        let (controls, control_updates) = mpsc::channel();
        let navigation = crate::state::Navigation::new(
            &self.0,
            crate::state::EditorState {
                doc: "sc_owner01".into(),
                selection: vec!["e_owner001".into()],
                focus: "inspector".into(),
                focus_field: Some("Transform.position".into()),
                ..Default::default()
            },
        )
        .unwrap();
        host.register(Box::new(crate::actions::EditorService {
            navigation,
            settings: crate::settings::Settings::open(&self.0, None).unwrap(),
            changes,
            controls,
            resolver: Default::default(),
            bounds: crate::world::bounds::ModelBounds::new(&self.0).unwrap(),
            bounds_key: None,
            pending_frames: Default::default(),
            frame_aspect: 16. / 9.,
            extensions: Extensions {
                interaction: interaction.clone(),
                catalog: Catalog {
                    commands: vec![Command {
                        id: "tools.owned".into(),
                        title: "Owned callback".into(),
                        summary: "Test callback ownership".into(),
                        enabled: true,
                        disabled_reason: None,
                    }],
                    menus: vec![],
                    generation: Some("test-generation".into()),
                },
                pending: Default::default(),
                completed,
                requests,
                cancelled: Arc::new(AtomicBool::new(false)),
                worker: None,
            },
        }))
        .unwrap();
        let human = host
            .call(
                ConnId::LOCAL,
                "session.begin",
                json!({"client":"ownership-test","author":"human"}),
            )
            .unwrap()["session"]
            .as_str()
            .unwrap()
            .to_owned();
        Harness {
            host,
            interaction,
            send,
            incoming,
            _updates: updates,
            _controls: control_updates,
            human,
        }
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        assert!(
            self.0
                .starts_with(crate::test_target().join("extension-navigation"))
        );
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Harness {
    host: Host,
    interaction: crate::interaction::Interaction,
    send: mpsc::Sender<Completion>,
    incoming: mpsc::Receiver<Request>,
    _updates: mpsc::Receiver<crate::state::EditorState>,
    _controls: mpsc::Receiver<crate::actions::Control>,
    human: String,
}
impl Harness {
    fn invoke(&mut self, session: &str) -> String {
        let result = self
            .host
            .call(
                ConnId::LOCAL,
                "editor.command",
                json!({"session":session,"id":"tools.owned"}),
            )
            .unwrap();
        let job = result["job"].as_str().unwrap().to_owned();
        let Request::Invoke(request_job, request) = self.incoming.try_recv().unwrap() else {
            panic!("expected callback invocation")
        };
        assert_eq!(request_job, job);
        assert_eq!(request["scene"]["entities"][0]["id"], "e_owner001");
        job
    }

    fn complete(&mut self, session: &str, job: &str) -> Value {
        self.complete_with_diagnostics(session, job, Vec::new())
    }

    fn complete_with_diagnostics(
        &mut self,
        session: &str,
        job: &str,
        diagnostics: Vec<Diagnostic>,
    ) -> Value {
        self.send.send(Completion::Invoke(job.into(), Ok(Reply {
            catalog: Catalog {generation:Some("test-generation".into()),..Default::default()},
            accepted: true,
            diagnostics,
            authoring: vec![ara_host::params::CallSpec {cmd:"property.set".into(),args:Some(json!({"entity":"e_owner001","path":"name","value":"Callback authored"})),symbol:None}],
            selection: Some(vec!["e_owner001".into()]),
        }))).unwrap();
        self.host.tick();
        let status = self
            .host
            .call(
                ConnId::LOCAL,
                "jobs.status",
                json!({"session":session,"job":job}),
            )
            .unwrap();
        assert_eq!(status["status"], "done", "{status}");
        assert_eq!(
            self.host
                .store()
                .workspace()
                .doc("sc_owner01")
                .unwrap()
                .record("e_owner001")
                .unwrap()["name"],
            "Callback authored"
        );
        assert_eq!(
            self.host
                .store()
                .workspace()
                .doc("sc_owner02")
                .unwrap()
                .record("e_other001")
                .unwrap()["name"],
            "Other"
        );
        status["result"].clone()
    }

    fn state(&mut self) -> Value {
        self.host
            .call(ConnId::LOCAL, "editor.state", json!({"session":self.human}))
            .unwrap()["state"]
            .clone()
    }
}

#[test]
fn accepted_callback_publishes_informational_rows_and_warnings_without_failing_the_job() {
    let project = Project::new();
    let mut editor = project.open();
    let human = editor.human.clone();
    let job = editor.invoke(&human);
    editor.complete_with_diagnostics(
        &human,
        &job,
        vec![
            Diagnostic::info(
                ara_diag::codes::IO,
                "Accepted callback informational result",
            ),
            Diagnostic::warning(ara_diag::codes::IO, "Accepted callback warning"),
        ],
    );
    let logs = editor
        .host
        .call(
            ConnId::LOCAL,
            "logs",
            json!({"session":human,"severity":"info","group":false,"limit":100}),
        )
        .unwrap();
    for message in [
        "Accepted callback informational result",
        "Accepted callback warning",
    ] {
        assert!(
            logs["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["diagnostic"]["message"] == message),
            "{logs}"
        );
    }
}

#[test]
fn human_navigation_suppresses_hint_but_keeps_captured_authoring_and_undo() {
    let project = Project::new();
    let mut editor = project.open();
    let human = editor.human.clone();
    let revision = editor.host.store().revision();
    let job = editor.invoke(&human);
    editor
        .host
        .call(
            ConnId::LOCAL,
            "editor.select",
            json!({"session":human,"doc":"sc_owner02","entities":["e_other001"]}),
        )
        .unwrap();
    let before = editor.state();
    assert_eq!(editor.host.store().revision(), revision);
    let result = editor.complete(&human, &job);
    assert!(result["navigationSuppressed"].is_string(), "{result}");
    assert!(result.get("navigation").is_none());
    assert_eq!(editor.state(), before);
    editor
        .host
        .call(ConnId::LOCAL, "history.undo", json!({"session":human}))
        .unwrap();
    assert_eq!(
        editor
            .host
            .store()
            .workspace()
            .doc("sc_owner01")
            .unwrap()
            .record("e_owner001")
            .unwrap()["name"],
        "Original"
    );
    assert_eq!(editor.state(), before);
}

#[test]
fn same_focused_field_input_suppresses_hint_without_rejecting_valid_authoring() {
    let project = Project::new();
    let mut editor = project.open();
    let human = editor.human.clone();
    let before = editor.state();
    let revision = editor.host.store().revision();
    let job = editor.invoke(&human);
    editor.interaction.note();
    assert_eq!(editor.state(), before);
    assert_eq!(editor.host.store().revision(), revision);
    let result = editor.complete(&human, &job);
    assert!(
        result["navigationSuppressed"]
            .as_str()
            .unwrap()
            .contains("input")
    );
    assert_eq!(editor.state(), before);
}

#[test]
fn unchanged_human_context_accepts_the_navigation_hint() {
    let project = Project::new();
    let mut editor = project.open();
    let human = editor.human.clone();
    let job = editor.invoke(&human);
    let result = editor.complete(&human, &job);
    assert!(result["navigation"].is_object(), "{result}");
    assert!(result.get("navigationSuppressed").is_none());
}

#[test]
fn agent_without_presence_keeps_original_doc_and_never_steals_later_human_view() {
    let project = Project::new();
    let mut editor = project.open();
    let agent = editor
        .host
        .call(
            ConnId::LOCAL,
            "session.begin",
            json!({"client":"agent-ownership-test","author":"agent:ownership-test"}),
        )
        .unwrap()["session"]
        .as_str()
        .unwrap()
        .to_owned();
    let job = editor.invoke(&agent);
    editor
        .host
        .call(
            ConnId::LOCAL,
            "editor.select",
            json!({"session":editor.human,"doc":"sc_owner02","entities":["e_other001"]}),
        )
        .unwrap();
    editor.interaction.note();
    let before = editor.state();
    let result = editor.complete(&agent, &job);
    assert!(result["navigation"].is_object(), "{result}");
    let after = editor.state();
    assert_eq!(after["doc"], before["doc"]);
    assert_eq!(after["selection"], before["selection"]);
    assert_eq!(after["camera"], before["camera"]);
    assert_eq!(after["presence"][&agent]["doc"], "sc_owner01");
    assert_eq!(
        after["presence"][&agent]["selection"],
        json!(["e_owner001"])
    );
}
