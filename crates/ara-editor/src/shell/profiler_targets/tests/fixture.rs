use super::*;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, Sender},
    },
};
pub(super) enum Change {
    Rows(Vec<Row>),
    Reply(Status),
}
struct Controlled {
    rows: Vec<Row>,
    changes: Receiver<Change>,
    requests: Sender<(Ticket, Request)>,
    replies: BTreeMap<u64, Status>,
    next: u64,
    reads: Arc<AtomicUsize>,
}
impl Backend for Controlled {
    fn refresh_targets(&mut self) -> Result<(), Vec<ara_diag::Diagnostic>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        while let Ok(change) = self.changes.try_recv() {
            match change {
                Change::Rows(rows) => self.rows = rows,
                Change::Reply(status) => {
                    self.replies.insert(status.ticket.0, status);
                }
            }
        }
        Ok(())
    }
    fn targets(&self) -> &[Row] {
        &self.rows
    }
    fn request(&mut self, request: Request) -> Result<Ticket, Vec<ara_diag::Diagnostic>> {
        self.next += 1;
        let ticket = Ticket(self.next);
        self.requests.send((ticket, request)).unwrap();
        Ok(ticket)
    }
    fn poll(&mut self, ticket: Ticket) -> Option<Status> {
        self.replies.remove(&ticket.0)
    }
}
struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::path::absolute(crate::test_target())
            .unwrap()
            .join("profiler-targets")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(root.join(ara_doc::AUTHORING_ROOT).join("levels")).unwrap();
        std::fs::write(root.join("ara.project.json"),ara_json::to_canonical_string(&json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"name":"Profiler targets","main":"levels/main.scene"}))).unwrap();
        std::fs::write(
            root.join(ara_doc::AUTHORING_ROOT).join("levels/main.scene"),
            ara_json::to_canonical_string(
                &json!({"$schema":"ara://schema/scene/1","id":"sc_profilertest","records":{}}),
            ),
        )
        .unwrap();
        Self(root)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
pub(super) struct Fixture {
    pub editor: Editor,
    pub shell: Shell,
    pub requests: Option<Receiver<(Ticket, Request)>>,
    changes: Option<Sender<Change>>,
    pub reads: Option<Arc<AtomicUsize>>,
    _project: Project,
}
impl Fixture {
    pub fn new(controlled: bool) -> Self {
        let project = Project::new();
        let profile = project.0.join(".ara/test-profile");
        std::fs::create_dir_all(&profile).unwrap();
        let mut editor =
            Editor::open_with(&project.0, Some(profile.join("editor-settings.json"))).unwrap();
        editor
            .action(
                "editor.layout.move",
                json!({"panel":"profiler","slot":"view"}),
            )
            .unwrap();
        let mut requests = None;
        let mut changes = None;
        let mut reads = None;
        if controlled {
            let (send, receive) = mpsc::channel();
            let (requests_send, requests_receive) = mpsc::channel();
            let count = Arc::new(AtomicUsize::new(0));
            editor.play_profiling = Some(Box::new(Controlled {
                rows: vec![row(1, "first", 1), row(2, "second", 1)],
                changes: receive,
                requests: requests_send,
                replies: BTreeMap::new(),
                next: 0,
                reads: Arc::clone(&count),
            }));
            requests = Some(requests_receive);
            changes = Some(send);
            reads = Some(count);
        }
        let mut shell = Shell::new(&mut editor, [1100., 760.]).unwrap();
        shell.offscreen = true;
        Self {
            editor,
            shell,
            requests,
            changes,
            reads,
            _project: project,
        }
    }
    pub fn poll(&mut self) {
        self.shell.profiler_targets.next_poll = None;
        self.shell
            .poll_profiler_target_state(&mut self.editor)
            .unwrap();
    }
    pub fn reply(&mut self, status: Status) {
        self.changes
            .as_ref()
            .unwrap()
            .send(Change::Reply(status))
            .unwrap();
        self.poll();
        self.shell.rebuild(&mut self.editor).unwrap();
        self.shell.frame().unwrap();
    }
    pub fn rows(&mut self, rows: Vec<Row>) {
        self.changes
            .as_ref()
            .unwrap()
            .send(Change::Rows(rows))
            .unwrap();
        self.poll();
    }
}
pub(super) fn scope(player: u32, source: &str, binding: u64) -> Scope {
    Scope {
        world: "world-proof".into(),
        player,
        source: source.into(),
        binding,
    }
}
pub(super) fn row(player: u32, source: &str, binding: u64) -> Row {
    Row {
        target: scope(player, source, binding),
        name: format!("Client {player}"),
        process_id: Some(1200 + player),
        recording: false,
        available: true,
        reason: None,
    }
}
pub(super) fn profile() -> ara_profile::Profile {
    ara_profile::Profile {
        version: 1,
        budget_ns: 16_666_667,
        labels: vec!["Player.Update".into()],
        threads: vec![ara_profile::ThreadInfo {
            id: 1,
            name: "Player thread".into(),
        }],
        frames: [ara_profile::Frame {
            id: 7,
            duration_ns: 4_000_000,
            spans: vec![ara_profile::CpuSpan {
                label: 0,
                thread: 1,
                id: 1,
                parent: 0,
                start_ns: 0,
                duration_ns: 1_000_000,
                self_ns: 1_000_000,
                allocated_bytes: 0,
                self_allocated_bytes: 0,
            }],
            ..Default::default()
        }]
        .into(),
        ..Default::default()
    }
}
pub(super) struct Clip;
impl ara_ui_text::Clipboard for Clip {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(String::new())
    }
    fn write(&mut self, _: &str) -> ara_ui_text::Result<()> {
        Ok(())
    }
}
pub(super) fn click(shell: &mut Shell, editor: &mut Editor, id: &str) {
    let node = &shell.ui.nodes[shell.shown_node(id).unwrap()];
    let rect = node.rect.intersection(node.clip);
    assert!(rect.w > 0. && rect.h > 0.);
    let point = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
    assert_eq!(
        shell
            .ui
            .hit_test(point[0], point[1])
            .map(|index| shell.ui.nodes[index].element.id.as_str()),
        Some(id)
    );
    shell
        .event(InputEvent::PointerDown(point), &mut Clip)
        .unwrap();
    shell
        .event(InputEvent::PointerUp(point), &mut Clip)
        .unwrap();
    shell.dispatch(editor).unwrap();
    shell.frame().unwrap();
}
