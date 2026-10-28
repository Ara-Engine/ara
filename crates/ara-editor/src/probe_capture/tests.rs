#![allow(clippy::unwrap_used)]
mod fixture;
use super::*;
use ara_runtime_render::probes::{ProbeJob, ProbeState};
use fixture::*;
use serde_json::json;

#[test]
fn invalidating_one_subscriber_preserves_a_later_same_scene_request() {
    let mut f = Fixture::new();
    let old = f.queue("e_probe");
    f.started(&old, 71);
    let fresh = f.queue("e_probe");
    f.bridge.request_cancel(&old.job);
    assert!(f.bridge.cancel_requested(&old.job));
    assert!(!f.bridge.cancel_requested(&fresh.job));
    assert!(matches!(f.bridge.try_recv(), Some(Request::Cancel { job, .. }) if job == old.job));
    // A subscriber not assigned a backend yet can still coalesce. Wait until its
    // association is known before authorizing cancellation of the older backend.
    assert!(f.bridge.cancel_target(&old.job).is_none());
    f.started(&fresh, 72);
    assert_eq!(f.bridge.cancel_target(&old.job).map(|job| job.0), Some(71));
    f.status(&old, 71, true, false, 1.);
    f.status(&fresh, 72, true, false, 1.);
    assert_eq!(f.job(&fresh.job)["status"], "done");
    assert!(!f.bridge.busy());
}

#[test]
fn host_job_reports_real_progress_and_receipts_without_authoring_ops() {
    let mut f = Fixture::new();
    let revision = f.host.store().revision();
    let first = f.query(None, None);
    assert_eq!(first["capabilities"]["reflectionProbes"], false);
    assert!(
        first["capabilities"]["reasons"]["reflectionProbes"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    assert!(matches!(f.bridge.try_recv(), Some(Request::Refresh)));
    let task = f.queue("e_probe");
    assert_eq!(task.source_revision, revision);
    assert_eq!(task.session, f.session);
    f.started(&task, 11);
    f.status(&task, 11, false, true, 0.5);
    let running = f.job(&task.job);
    assert_eq!(running["status"], "running");
    assert_eq!(running["progress"], 0.5);
    assert_eq!(running["session"], f.session);
    assert!(f.bridge.busy());
    f.status(&task, 11, true, false, 1.);
    let done = f.job(&task.job);
    assert_eq!(done["status"], "done");
    assert_eq!(done["result"]["ops"], json!([]));
    assert!(done["result"]["txn"].is_null());
    assert_eq!(done["result"]["receipts"][0]["faces"], 6);
    assert_eq!(
        done["result"]["receipts"][0]["sourceRevision"], 7,
        "renderer content counter is distinct from Host revision"
    );
    assert_eq!(f.host.store().revision(), revision);
    assert!(!f.bridge.busy());
}

#[test]
fn cancellation_is_owned_and_shared_backend_retirement_outlives_cancelled_host_receipts() {
    let mut f = Fixture::new();
    let first = f.queue("e_probe");
    let other = f.begin("agent:other");
    let second = f.queue_for(&other, "e_probe");
    f.started(&first, 23);
    f.started(&second, 23);
    assert!(
        f.host
            .call(
                ara_proto::ConnId::LOCAL,
                "render.probe.cancel",
                json!({"session":other,"job":first.job})
            )
            .is_err()
    );
    assert_eq!(f.cancel(&first)["cancelRequested"], true);
    assert!(matches!(f.bridge.try_recv(), Some(Request::Cancel { job, .. }) if job == first.job));
    assert!(
        f.bridge.cancel_target(&first.job).is_none(),
        "other session still needs the shared generation"
    );
    let cancelled = f.call("jobs.cancel", json!({"session":other,"job":second.job}));
    assert_eq!(cancelled["cancelled"], true);
    f.host.tick();
    assert!(matches!(f.bridge.try_recv(), Some(Request::Cancel { job, .. }) if job == second.job));
    assert_eq!(
        f.bridge.cancel_target(&first.job).map(|job| job.0),
        Some(23)
    );
    assert_eq!(f.job(&first.job)["status"], "cancelled");
    assert_eq!(f.bridge.resource_report()["activeHostJobs"], 2);
    let wrong = status(&first, 23, true, false, 1.);
    assert!(
        f.bridge
            .send(Event::Status {
                job: first.job.clone(),
                session: other.clone(),
                binding: first.binding(),
                status: wrong
            })
            .is_err()
    );
    assert!(f.bridge.busy());
    f.status(&first, 23, true, false, 1.);
    assert_eq!(f.bridge.resource_report()["activeHostJobs"], 1);
    f.status(&second, 23, true, false, 1.);
    assert!(!f.bridge.busy());
    assert_eq!(f.job(&first.job)["status"], "cancelled");
    assert_eq!(f.job(&second.job)["status"], "cancelled");
}

#[test]
fn owner_failure_preserves_structured_location_and_never_fabricates_success() {
    let mut f = Fixture::new();
    let task = f.queue("e_probe");
    f.started(&task, 61);
    let mut failed = status(&task, 61, true, false, 1.);
    failed.receipts.clear();
    failed
        .failures
        .push(ara_runtime_render::probes::ProbeFailure {
            doc: task.doc.clone(),
            entity: task.entity.clone(),
            state: ProbeState::Failed,
            diagnostics: vec![ara_diag::Diagnostic::error(
                crate::codes::RENDER,
                "Capture source was deleted before publication",
            )],
        });
    assert!(
        f.bridge
            .send(Event::Status {
                job: task.job.clone(),
                session: task.session.clone(),
                binding: task.binding(),
                status: failed
            })
            .is_ok()
    );
    f.host.tick();
    let record = f.job(&task.job);
    assert_eq!(record["status"], "failed");
    assert_eq!(record["diagnostics"][0]["location"]["doc"], task.doc);
    assert_eq!(record["diagnostics"][0]["location"]["record"], task.entity);
    assert_eq!(
        record["diagnostics"][0]["location"]["path"],
        "ReflectionProbe"
    );
    assert_eq!(
        record["diagnostics"][0]["data"]["probeCapture"]["failures"][0]["state"],
        "failed"
    );
    assert!(record.get("result").is_none());
    assert!(!f.bridge.busy());
}

#[test]
fn publishing_boundary_updates_ordinary_host_cancellability() {
    let mut f = Fixture::new();
    let task = f.queue("e_probe");
    f.started(&task, 31);
    f.status(&task, 31, false, false, 0.95);
    assert_eq!(f.job(&task.job)["cancellable"], false);
    assert_eq!(f.cancel(&task)["cancelRequested"], false);
    assert_eq!(
        f.call("jobs.cancel", json!({"job":task.job}))["cancelled"],
        false
    );
    assert!(f.bridge.try_recv().is_none());
    f.status(&task, 31, true, false, 1.);
    assert_eq!(f.job(&task.job)["status"], "done");
}

#[test]
fn source_mismatch_fails_located_receipt_but_keeps_owner_tracked_until_finished() {
    let mut f = Fixture::new();
    let task = f.queue("e_probe");
    let mut wrong = task.binding();
    wrong.source_revision += 1;
    assert!(
        f.bridge
            .send(Event::Started {
                job: task.job.clone(),
                session: task.session.clone(),
                binding: wrong.clone(),
                backend: ProbeJob(41)
            })
            .is_ok()
    );
    f.host.tick();
    let failed = f.job(&task.job);
    assert_eq!(failed["status"], "failed");
    assert_eq!(failed["diagnostics"][0]["location"]["doc"], "sc_probejobs");
    assert!(matches!(f.bridge.try_recv(), Some(Request::Cancel { job, .. }) if job == task.job));
    assert!(f.bridge.busy());
    assert!(
        f.bridge
            .send(Event::Status {
                job: task.job.clone(),
                session: task.session.clone(),
                binding: wrong,
                status: status(&task, 41, true, false, 1.)
            })
            .is_ok()
    );
    f.host.tick();
    assert!(!f.bridge.busy());
    assert_eq!(f.job(&task.job)["status"], "failed");
    let task = f.queue("e_probe");
    f.started(&task, 42);
    let mut completed = status(&task, 42, true, false, 1.);
    completed.receipts[0].source_revision += 1;
    assert!(
        f.bridge
            .send(Event::Status {
                job: task.job.clone(),
                session: task.session.clone(),
                binding: task.binding(),
                status: completed
            })
            .is_ok()
    );
    f.host.tick();
    let failed = f.job(&task.job);
    assert_eq!(failed["status"], "failed");
    assert!(
        failed["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["message"]
                .as_str()
                .is_some_and(|text| text.contains("source-matching")))
    );
    assert_eq!(f.host.store().revision(), task.source_revision);
}

#[test]
fn full_snapshot_cache_is_scene_scoped_and_since_empty_does_not_erase_last_good_rows() {
    let mut f = Fixture::new();
    for _ in 0..3 {
        assert_eq!(f.query(None, None)["version"], 0);
    }
    assert!(matches!(f.bridge.try_recv(), Some(Request::Refresh)));
    assert!(f.bridge.try_recv().is_none(), "refresh requests coalesce");
    f.snapshot(8);
    let first = f.query(None, None);
    assert_eq!(first["capabilities"]["reflectionProbes"], true);
    assert_eq!(first["probes"][0]["entity"], "e_probe");
    assert!(
        f.query(None, Some(8))["probes"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(f.query(None, None)["probes"].as_array().unwrap().len(), 1);
    let other = f.query(Some("sc_otherjobs"), None);
    assert_eq!(other["capabilities"]["reflectionProbes"], false);
    assert!(other["probes"].as_array().unwrap().is_empty());
    assert_eq!(f.query(None, None)["probes"].as_array().unwrap().len(), 1);
    assert!(
        f.bridge
            .send(Event::Unavailable {
                reason: "Scene owner closed; captures are retiring".into()
            })
            .is_ok()
    );
    f.host.tick();
    let unavailable = f.query(None, None);
    assert_eq!(unavailable["capabilities"]["reflectionProbes"], false);
    assert!(
        unavailable["capabilities"]["reasons"]["reflectionProbes"]
            .as_str()
            .unwrap()
            .contains("closed")
    );
}

#[test]
fn cached_rows_are_unavailable_immediately_after_typed_edit_or_delete() {
    let mut f = Fixture::new();
    f.call(
        "cmd.apply",
        json!({"doc":"sc_probejobs","why":"Create snapshot source",
        "commands":[{"cmd":"entity.create","args":{"id":"e_probe","name":"Probe source"}}]}),
    );
    f.snapshot(8);
    assert_eq!(f.query(None, None)["probes"][0]["entity"], "e_probe");
    for (command, version) in [
        (
            json!({"cmd":"property.set","args":{"entity":"e_probe","path":"name","value":"Changed source"}}),
            9,
        ),
        (
            json!({"cmd":"entity.delete","args":{"entities":["e_probe"]}}),
            10,
        ),
    ] {
        let cached_revision = f.host.store().revision();
        f.call(
            "cmd.apply",
            json!({"doc":"sc_probejobs","why":"Invalidate cached renderer source",
            "commands":[command]}),
        );
        let edited_revision = f.host.store().revision();
        assert!(edited_revision > cached_revision);
        // No owner poll, renderer update, sleep or service-event injection occurs
        // between the authoritative edit and this real protocol query.
        let unavailable = f.query(None, None);
        assert_eq!(unavailable["capabilities"]["reflectionProbes"], false);
        assert!(unavailable["probes"].as_array().unwrap().is_empty());
        assert!(
            unavailable["capabilities"]["reasons"]["reflectionProbes"]
                .as_str()
                .unwrap()
                .contains("changed")
        );
        assert_eq!(f.host.store().revision(), edited_revision);
        if version == 9 {
            f.snapshot(version);
            assert_eq!(f.query(None, None)["version"], version);
            assert_eq!(f.query(None, None)["probes"][0]["entity"], "e_probe");
        }
    }
}

#[test]
fn owner_backpressure_and_scene_close_cannot_report_false_retirement() {
    let mut f = Fixture::new();
    let task = f.queue("e_probe");
    f.started(&task, 51);
    f.bridge.cancel_scene("sc_otherjobs");
    assert!(!f.bridge.cancel_requested(&task.job));
    f.bridge.cancel_scene("sc_probejobs");
    assert!(f.bridge.cancel_requested(&task.job));
    assert!(matches!(f.bridge.try_recv(), Some(Request::Cancel { .. })));
    for version in 1..=super::CHANNEL_CAPACITY as u64 {
        assert!(
            f.bridge
                .send(Event::Snapshot {
                    binding: task.binding(),
                    snapshot: snapshot(&task, version)
                })
                .is_ok()
        );
    }
    let terminal = Event::Status {
        job: task.job.clone(),
        session: task.session.clone(),
        binding: task.binding(),
        status: status(&task, 51, true, false, 1.),
    };
    let retry = match f.bridge.send(terminal) {
        Err(event) => event,
        Ok(()) => panic!("full event channel must return the terminal observation"),
    };
    assert_eq!(f.bridge.resource_report()["activeHostJobs"], 1);
    f.host.tick();
    assert!(f.bridge.send(*retry).is_ok());
    f.host.tick();
    assert!(!f.bridge.busy());
    f.bridge.begin_drain();
    let queued = f.queue("e_probe");
    assert!(f.bridge.cancel_requested(&queued.job));
    assert!(
        f.bridge
            .send(Event::Rejected {
                job: queued.job.clone(),
                session: queued.session.clone(),
                diagnostics: vec![crate::invalid("Scene capture owner is draining")]
            })
            .is_ok()
    );
    f.host.tick();
    assert!(!f.bridge.busy());
    assert_eq!(f.job(&queued.job)["status"], "failed");
}
