//! CPU UI/Host transport coverage. Injected owner observations do not verify GPU captures.
#![allow(clippy::unwrap_used)]
mod fixture;
use super::*;
use crate::probe_capture::{Event, Request, SceneBinding};
use ara_runtime_render::probes::{ProbeJob, ProbeJobStatus};
use ara_ui::{input::InputEvent, source::stable_id};
use fixture::*;

#[test]
fn numeric_probe_resolution_is_a_real_select_with_numeric_ops_and_one_undo() {
    let mut f = Fixture::new();
    let entity = f.create();
    f.shell.frame().unwrap();
    let id = "enum-ReflectionProbe-captureResolution";
    let index = f.shell.shown_node(id).unwrap();
    assert_eq!(f.shell.ui.nodes[index].element.tag, "dropdown");
    f.shell.ui.scroll_into_view(index).unwrap();
    f.shell.frame().unwrap();
    let revision = f.editor.host.store().revision();
    click(&mut f.shell, &mut f.editor, id);
    f.shell.key(&mut f.editor, 77, true, &mut Clip).unwrap();
    f.shell.key(&mut f.editor, 40, true, &mut Clip).unwrap();
    f.shell.frame().unwrap();
    let probe = f.probe(&entity);
    assert_eq!(probe.capture_resolution, 1024);
    let resolved = crate::world::resolve(f.editor.host.store(), &f.editor.state.doc).unwrap();
    assert!(resolved[&entity].record["ReflectionProbe"]["captureResolution"].is_number());
    assert_eq!(f.editor.host.store().revision(), revision + 1);
    f.editor.action("history.undo", json!({})).unwrap();
    assert_eq!(f.probe(&entity).capture_resolution, 256);
    assert!(ara_ui_test::audit_attached_ids(&f.shell.ui).is_empty());
}

#[test]
fn unsupported_capability_and_retained_since_rows_show_real_receipt_copy_without_authoring() {
    let mut f = Fixture::new();
    let entity = f.create();
    let doc = f.editor.state.doc.clone();
    f.shell.probe_ui.current_doc(&doc);
    assert!(
        f.shell
            .probe_ui
            .observe(snapshot(&doc, &entity, 9, false), None)
    );
    let mut empty = snapshot(&doc, &entity, 9, false);
    empty.probes.clear();
    assert!(!f.shell.probe_ui.observe(empty, Some(9)));
    assert_eq!(f.shell.probe_ui.snapshot.as_ref().unwrap().probes.len(), 1);
    let revision = f.editor.host.store().revision();
    let selection = f.editor.state.selection.clone();
    let camera = f.editor.state.camera;
    f.shell.rebuild(&mut f.editor).unwrap();
    f.shell.frame().unwrap();
    let recapture = f.shell.ui.find("lighting-probe-recapture").unwrap();
    assert!(f.shell.ui.nodes[recapture].state.disabled);
    let unavailable = f.shell.ui.find("lighting-probes-unavailable").unwrap();
    assert!(
        f.shell.ui.nodes[unavailable].element.attrs["text"]
            .contains("This CPU fixture has no capture renderer")
    );
    assert!(
        f.shell
            .recapture_probes(&mut f.editor, &commands::Target::Entity(entity.clone()))
            .is_err()
    );
    f.shell
        .copy_probe_state(&f.editor, &commands::Target::Entity(entity.clone()))
        .unwrap();
    let copy: Value = serde_json::from_str(&f.shell.pending_clipboard.take().unwrap()).unwrap();
    assert_eq!(copy["probe"]["lastReceipt"]["faces"], 6);
    assert_eq!(copy["probe"]["lastReceipt"]["captureHash"], "capture-proof");
    assert_eq!(copy["queryVersion"], 9);
    assert_eq!(copy["capabilities"]["reflectionProbes"], false);
    assert_eq!(f.editor.host.store().revision(), revision);
    assert_eq!(f.editor.state.selection, selection);
    assert_eq!(f.editor.state.camera, camera);
    f.shell.probe_ui.current_doc("sc_different");
    assert!(
        f.shell.probe_ui.snapshot.is_none(),
        "old Scene rows are never attributed to the replacement doc"
    );
}

#[test]
fn coalesced_requests_cancel_every_own_subscription_and_keep_other_session_work_active() {
    let mut f = Fixture::new();
    let entity = f.create();
    let binding = SceneBinding {
        doc: f.editor.state.doc.clone(),
        scene_revision: 7,
        source_revision: f.editor.host.store().revision(),
    };
    assert!(
        f.editor
            .probe_captures
            .send(Event::Snapshot {
                binding: binding.clone(),
                snapshot: snapshot(&binding.doc, &entity, 1, true)
            })
            .is_ok()
    );
    f.editor.tick();
    f.shell.refresh_probe_state(&mut f.editor).unwrap();
    for _ in 0..2 {
        f.shell
            .recapture_probes(&mut f.editor, &commands::Target::Entity(entity.clone()))
            .unwrap();
    }
    let other = f
        .editor
        .host
        .call(
            ara_proto::ConnId::LOCAL,
            "session.begin",
            json!({"client":"Probe UI other subscriber","author":"agent:probe-other"}),
        )
        .unwrap()["session"]
        .as_str()
        .unwrap()
        .to_owned();
    let other_job = f
        .editor
        .host
        .call(
            ara_proto::ConnId::LOCAL,
            "render.probe.recapture",
            json!({"session":other,"doc":binding.doc,"entities":[entity]}),
        )
        .unwrap()["job"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut tasks = Vec::new();
    while let Some(request) = f.editor.probe_captures.try_recv() {
        if let Request::Recapture { job, session, .. } = request {
            tasks.push((job, session));
        }
    }
    assert_eq!(tasks.len(), 3);
    for (job, session) in &tasks {
        assert!(
            f.editor
                .probe_captures
                .send(Event::Started {
                    job: job.clone(),
                    session: session.clone(),
                    binding: binding.clone(),
                    backend: ProbeJob(44)
                })
                .is_ok()
        );
    }
    f.editor.tick();
    f.shell
        .cancel_probe_capture(&mut f.editor, &commands::Target::Entity(entity.clone()))
        .unwrap();
    let own: Vec<_> = tasks.iter().filter(|(job, _)| job != &other_job).collect();
    for (job, _) in &own {
        assert_eq!(
            f.editor.action("jobs.status", json!({"job":job})).unwrap()["status"],
            "cancelled"
        );
        assert!(f.editor.probe_captures.cancel_requested(job));
        assert!(
            f.editor.probe_captures.cancel_target(job).is_none(),
            "other session still needs the coalesced generation"
        );
    }
    assert_eq!(
        f.editor
            .action("jobs.status", json!({"job":other_job}))
            .unwrap()["status"],
        "running"
    );
    assert!(
        f.editor.probe_captures.busy(),
        "cancelled Host jobs are not GPU retirement evidence"
    );
    let id = format!(
        "lighting-probe-{:016x}-retirement",
        stable_id(&format!("{}#{}", binding.doc, entity))
    );
    assert!(f.shell.ui.find(&id).is_some());
    // Explicit fake-owner completion only retires this CPU transport fixture.
    for (job, session) in tasks {
        let status:ProbeJobStatus=serde_json::from_value(json!({"job":44,"finished":true,"progress":1,"message":"CPU owner transport completion","cancellable":false,
            "receipts":[receipt(&binding.doc,&entity)],"failures":[],"diagnostics":[]})).unwrap();
        assert!(
            f.editor
                .probe_captures
                .send(Event::Status {
                    job,
                    session,
                    binding: binding.clone(),
                    status
                })
                .is_ok()
        );
    }
    while f.editor.probe_captures.try_recv().is_some() {}
    f.editor.tick();
    assert!(!f.editor.probe_captures.busy());
}

#[test]
fn terminal_history_is_bounded_without_hiding_any_active_or_unknown_subscription() {
    let mut state = State::default();
    for index in 0..164 {
        let id = format!("job-{index:03}");
        state.job_order.push_back(id.clone());
        state.jobs.insert(
            id,
            Job {
                doc: "sc_probe".into(),
                entities: vec!["e_probe".into()],
                error: None,
                latest: if index < 64 {
                    None
                } else {
                    Some(json!({"status":"done","cancellable":false}))
                },
            },
        );
    }
    state.trim_terminal();
    assert_eq!(state.jobs.values().filter(|job| job.running()).count(), 64);
    assert_eq!(state.jobs.values().filter(|job| !job.running()).count(), 32);
    assert!((0..64).all(|index| state.jobs.contains_key(&format!("job-{index:03}"))));
    assert_eq!(state.job_order.len(), 96);
}
