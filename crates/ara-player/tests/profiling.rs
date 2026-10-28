//! Native control mailbox proof against a real headless player; never creates windows.
#![allow(clippy::unwrap_used)]
mod support;
use ara_play_host::{
    PlayProfileAction, PlayProfileOutcome, PlayProfileRequest, PlayProfileStatus,
    PlayProfileTarget, PlayProfilingHandle,
};
use serde_json::json;
use support::{Agent, repo};

fn perform(
    agent: &mut Agent,
    handle: &mut PlayProfilingHandle,
    target: &PlayProfileTarget,
    action: PlayProfileAction,
) -> PlayProfileStatus {
    let ticket = handle
        .request(PlayProfileRequest {
            session: agent.session.clone(),
            target: target.clone(),
            action,
            why: Some("native profiling proof".into()),
        })
        .unwrap();
    let mut result = None;
    agent.until("private player profiling result", |_| {
        result = handle.poll(ticket);
        result.as_ref().is_some_and(|status| status.finished)
    });
    let result = result.unwrap();
    assert_eq!(&result.target, target);
    result
}

#[test]
fn selected_player_records_completed_frames_without_changing_simulation_and_refuses_exited_targets()
{
    let dir = std::env::var_os("ARA_TEST_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| repo().join("target/player-profiling"))
        .join(format!("private-profile-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(ara_doc::AUTHORING_ROOT)).unwrap();
    std::fs::write(
        dir.join("ara.project.json"),
        ara_json::to_canonical_string(
            &json!({"name":"Profile proof","main":"main.scene","layoutVersion":4}),
        ),
    )
    .unwrap();
    std::fs::write(dir.join(ara_doc::AUTHORING_ROOT).join("main.scene"),ara_json::to_canonical_string(&json!({"$schema":"ara://schema/scene/1","id":"sc_profile","records":{"e_probe":{"name":"Probe","Transform":{}}}}))).unwrap();
    let mut handle = None;
    let mut agent = Agent::open_with(
        &dir,
        5,
        |_| {},
        |service| handle = Some(service.profiling()),
    );
    let mut handle = handle.unwrap();
    agent.pool_ready();
    let baseline = agent.play("start", json!({"startPaused":true,"seed":5}))["world"].clone();
    let baseline_hash = agent.play("step", json!({"world":baseline,"ticks":120}))["hash"].clone();
    assert!(
        baseline_hash.is_string(),
        "real simulation hash: {baseline_hash}"
    );
    agent.play("stop", json!({"world":baseline}));
    agent.pool_ready();
    let world = agent.play("start", json!({"startPaused":true,"seed":5}))["world"]
        .as_str()
        .unwrap()
        .to_owned();
    agent.until("the chosen player's profiler", |_| {
        handle.refresh_targets().unwrap();
        handle
            .targets()
            .iter()
            .any(|row| row.target.world == world && row.available)
    });
    let row = handle
        .targets()
        .iter()
        .find(|row| row.target.world == world && row.available)
        .unwrap();
    assert!(row.process_id > 0);
    assert!(!row.recording);
    assert!(row.name.contains(&world));
    let target = row.target.clone();
    assert!(target.binding > 0 && !target.source.is_empty());
    assert_eq!(
        perform(&mut agent, &mut handle, &target, PlayProfileAction::Stop).outcome,
        Some(PlayProfileOutcome::Unchanged)
    );
    assert_eq!(
        perform(&mut agent, &mut handle, &target, PlayProfileAction::Start).outcome,
        Some(PlayProfileOutcome::Completed)
    );
    assert_eq!(
        perform(&mut agent, &mut handle, &target, PlayProfileAction::Start).outcome,
        Some(PlayProfileOutcome::Unchanged)
    );
    let stepped = agent.play("step", json!({"world":world,"ticks":120}));
    assert_eq!(
        stepped["hash"], baseline_hash,
        "profiling metadata must not enter simulation state"
    );
    let captured = perform(
        &mut agent,
        &mut handle,
        &target,
        PlayProfileAction::Snapshot,
    );
    assert_eq!(captured.outcome, Some(PlayProfileOutcome::Completed));
    assert!(captured.recording);
    let profile = captured.profile.as_ref().unwrap();
    assert!(!profile.frames.is_empty() && profile.frames.len() <= 120);
    assert!(
        profile
            .frames
            .iter()
            .any(|frame| !frame.spans.is_empty() && frame.counters.entities > 0)
    );
    assert!(
        profile
            .labels
            .iter()
            .any(|label| label == "player.control" || label == "player.frame")
    );
    assert!(
        profile.frames.iter().all(|frame| frame.gpu.is_empty()),
        "headless unsampled GPU passes stay absent"
    );
    let frozen = perform(&mut agent, &mut handle, &target, PlayProfileAction::Freeze);
    assert_eq!(frozen.outcome, Some(PlayProfileOutcome::Completed));
    assert!(!frozen.recording);
    assert!(frozen.profile.is_some());
    assert_eq!(
        perform(&mut agent, &mut handle, &target, PlayProfileAction::Stop).outcome,
        Some(PlayProfileOutcome::Unchanged)
    );
    assert_eq!(
        agent.play(
            "read",
            json!({"world":world,"paths":["e_probe.Transform.position"]})
        )["tick"],
        stepped["tick"]
    );
    agent.play("stop", json!({"world":world}));
    agent.until("exited profiler target is retained as unavailable", |_| {
        handle.refresh_targets().unwrap();
        handle
            .targets()
            .iter()
            .any(|row| row.target == target && !row.available && row.reason.is_some())
    });
    let refused = perform(
        &mut agent,
        &mut handle,
        &target,
        PlayProfileAction::Snapshot,
    );
    assert_eq!(refused.outcome, Some(PlayProfileOutcome::Refused));
    assert!(refused.profile.is_none());
    assert!(!refused.diagnostics.is_empty());
    assert!(
        handle
            .poll(ara_play_host::PlayProfileTicket(u64::MAX))
            .is_none()
    );
}
