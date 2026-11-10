//! Local patches must match the unchanged full-resolution path, including runtime divergence.
use super::*;
use ara_components::Transform;
use serde_json::json;
mod fixture;
use fixture::*;

#[test]
fn plain_and_nested_prefab_sets_match_full_resolution_and_preserve_runtime_mutations() {
    let mut local = fixture(false);
    let mut full = fixture(false);
    for edit in [&mut local, &mut full] {
        let root = edit.entity("e_1").expect("root");
        edit.world
            .get_mut::<Transform>(root)
            .expect("transform")
            .position = [9., 8., 7.];
        let unrelated = edit.entity("e_30/e_b/e_c").expect("other instance");
        edit.registry
            .write(
                &mut edit.world,
                &edit.ids,
                unrelated,
                "Stats",
                &json!({"hp":777}),
            )
            .expect("gameplay mutation");
    }
    let ops = vec![
        set(&local, "sc_test", "e_1", "Stats.hp", Some(json!(37))),
        set(
            &local,
            "sc_test",
            "e_3",
            "overrides[e_b/e_c/Stats.hp]",
            Some(json!(22)),
        ),
        set(
            &local,
            "sc_test",
            "e_3",
            "name",
            Some(json!("Renamed instance")),
        ),
    ];
    let handles = local.mapping.clone();
    local.apply_ops(&ops).expect("local batch");
    full_reference(&mut full, &ops).expect("full batch");
    same(&local, &full);
    assert_eq!(local.mapping, handles);
    assert_eq!(
        local
            .world
            .get::<Transform>(local.entity("e_1").expect("root"))
            .expect("transform")
            .position,
        [9., 8., 7.]
    );
    assert_eq!(
        local
            .component("e_3/e_b/e_c", "Stats")
            .expect("component")
            .expect("stats")["target"],
        json!("e_3/e_b")
    );
    assert_eq!(
        local
            .component("e_30/e_b/e_c", "Stats")
            .expect("component")
            .expect("stats")["hp"],
        json!(777)
    );
    let inverse = ops.iter().rev().map(Op::inverse).collect::<Vec<_>>();
    local.apply_ops(&inverse).expect("inverse local");
    full_reference(&mut full, &inverse).expect("inverse full");
    same(&local, &full);
}

#[test]
fn typed_preflight_failure_and_stale_preimages_roll_back_the_complete_batch() {
    for external in [false, true] {
        let mut local = fixture(external);
        let mut full = fixture(external);
        let queued = local.world.resource::<ExternalWrites>().0.clone();
        let before = comparable(&local);
        let cached = local.resolved.clone();
        let ops = vec![
            set(&local, "sc_test", "e_1", "Stats.hp", Some(json!(37))),
            set(&local, "sc_test", "e_2", "Guarded.value", Some(json!(13))),
        ];
        let error = local.apply_ops(&ops).expect_err("typed rejection");
        assert_eq!(
            error,
            full_reference(&mut full, &ops).expect_err("same rejection")
        );
        assert!(error.0.contains("thirteen"));
        same(&local, &full);
        assert_eq!(local.resolved, cached);
        assert_eq!(comparable(&local), before);
        let first = set(&local, "sc_test", "e_1", "Stats.hp", Some(json!(37)));
        let mut stale = set(&local, "sc_test", "e_2", "Stats.hp", Some(json!(2)));
        if let Op::Set { prev, .. } = &mut stale {
            *prev = Some(json!(999));
        }
        assert_eq!(
            local
                .apply_ops(&[first.clone(), stale.clone()])
                .expect_err("stale batch"),
            full_reference(&mut full, &[first, stale]).expect_err("same stale batch")
        );
        same(&local, &full);
        assert_eq!(comparable(&local), before);
        assert_eq!(local.world.resource::<ExternalWrites>().0, queued);
    }
}

#[test]
fn external_updates_and_removals_keep_complete_defaults_and_queue_order() {
    let mut local = fixture(true);
    let mut full = fixture(true);
    local.world.resource_mut::<ExternalWrites>().0.clear();
    full.world.resource_mut::<ExternalWrites>().0.clear();
    let ops = vec![
        set(&local, "sc_test", "e_1", "Stats", None),
        set(&local, "sc_test", "e_2", "Stats.hp", Some(json!(8))),
        set(
            &local,
            "sc_test",
            "e_3",
            "overrides[e_b/e_c/Stats.title]",
            Some(json!("changed")),
        ),
    ];
    local.apply_ops(&ops).expect("external local");
    full_reference(&mut full, &ops).expect("external full");
    same(&local, &full);
    let writes = &local.world.resource::<ExternalWrites>().0;
    assert_eq!(writes.len(), 3);
    assert_eq!(writes[0].value, None);
    assert_eq!(
        writes[1].value,
        Some(json!({"hp":8,"title":"","target":null}))
    );
    assert_eq!(
        writes[2].value.as_ref().expect("complete")["hp"],
        json!(100)
    );
}

#[test]
fn runtime_despawns_and_reused_slots_preserve_mapping_cleanup_and_full_fallback() {
    for changed_dead in [false, true] {
        let mut local = fixture(false);
        let mut full = fixture(false);
        for edit in [&mut local, &mut full] {
            let gone = edit.entity("e_10").expect("entity");
            let recycled = edit
                .world
                .despawn_no_free(gone)
                .expect("despawn authored row");
            let runtime = edit
                .world
                .spawn_at(
                    recycled,
                    (Identity("e_runtime".into()), Transform::default()),
                )
                .expect("reuse the exact freed index with its new generation")
                .id();
            assert_eq!(runtime.index(), gone.index(), "exercise a recycled index");
            assert_ne!(runtime, gone, "reused slot cannot revive an old handle");
        }
        let id = if changed_dead { "e_10" } else { "e_1" };
        let op = set(&local, "sc_test", id, "Stats.hp", Some(json!(11)));
        local
            .apply_ops(std::slice::from_ref(&op))
            .expect("local or fallback");
        full_reference(&mut full, &[op]).expect("reference");
        same(&local, &full);
        assert_eq!(local.mapping.contains_key("e_10"), changed_dead);
    }
}

#[test]
fn structural_and_prefab_document_edits_use_the_same_full_projection() {
    let mut local = fixture(false);
    let mut full = fixture(false);
    let invalid = set(
        &local,
        "sc_test",
        "e_3",
        "overrides[missing/Stats.hp]",
        Some(json!(29)),
    );
    let before = comparable(&local);
    assert_eq!(
        local
            .apply_ops(std::slice::from_ref(&invalid))
            .expect_err("invalid override"),
        full_reference(&mut full, &[invalid]).expect_err("same override error")
    );
    same(&local, &full);
    assert_eq!(comparable(&local), before);
    let op = set(&local, "pf_inner", "e_a", "Stats.hp", Some(json!(29)));
    local
        .apply_ops(std::slice::from_ref(&op))
        .expect("prefab doc");
    full_reference(&mut full, &[op]).expect("reference");
    same(&local, &full);
    let op = local
        .workspace
        .doc("sc_test")
        .expect("scene")
        .clone()
        .apply(
            Edit::Move {
                doc: AssetId::parse("sc_test").expect("doc"),
                id: RecordId::parse("e_4").expect("record"),
                parent: Some("e_1".into()),
                order: Some(ara_doc::OrderKey::parse("a2").expect("order")),
            },
            &local.registry.schema,
        )
        .expect("move");
    let mixed = vec![
        set(
            &local,
            "sc_test",
            "e_1",
            "Stats.title",
            Some(json!("mixed")),
        ),
        op,
        set(&local, "sc_test", "e_3", "removed", Some(json!(["e_b"]))),
    ];
    local.apply_ops(&mixed).expect("mixed structural");
    full_reference(&mut full, &mixed).expect("reference");
    same(&local, &full);
}

#[test]
fn portable_restore_and_recorded_local_ops_keep_source_baselines_and_state_hashes() {
    let mut local = fixture(false);
    let mut full = fixture(false);
    let op = set(&local, "sc_test", "e_1", "Stats.hp", Some(json!(37)));
    local.apply_ops(std::slice::from_ref(&op)).expect("local");
    full_reference(&mut full, &[op]).expect("full");
    let future = set(
        &local,
        "sc_test",
        "e_3",
        "overrides[e_b/e_c/Stats.hp]",
        Some(json!(9)),
    );
    let mut a = crate::Session::local(local, crate::TickConfig::default(), 77, Default::default())
        .expect("session");
    let mut b = crate::Session::local(full, crate::TickConfig::default(), 77, Default::default())
        .expect("session");
    a.pending_ops.insert(1, vec![future.clone()]);
    b.pending_ops.insert(1, vec![future]);
    let build = crate::StateHash::of_bytes(b"local-projection");
    let portable = a
        .snapshot(build)
        .expect("snapshot")
        .to_bytes()
        .expect("portable");
    assert_eq!(
        portable,
        b.snapshot(build)
            .expect("snapshot")
            .to_bytes()
            .expect("portable")
    );
    let saved = crate::Snapshot::from_bytes(&portable).expect("decode");
    let mut a = crate::PlayControl::new(a, build);
    let mut b = crate::PlayControl::new(b, build);
    a.start();
    b.start();
    a.record().expect("record");
    a.step(3, &[]).expect("steps");
    let expected = a.session().state_hash().expect("hash");
    let replay = a.finish_recording().expect("replay");
    let replay = crate::Replay::from_bytes(&replay.to_bytes().expect("encode")).expect("decode");
    assert_eq!(b.replay(&replay).expect("recorded local patch"), expected);
    a.restore(&saved).expect("portable restore");
    a.step(3, &[]).expect("queued local op after restore");
    assert_eq!(a.session().state_hash().expect("restored hash"), expected);
    assert_eq!(
        a.session().projection().resolved,
        b.session().projection().resolved
    );
}

#[test]
fn one_large_instance_name_and_override_edits_match_full_resolution() {
    let mut local = large_instance(512);
    let mut full = large_instance(512);
    let handles = local.mapping.clone();
    for (path, value) in [
        ("name", json!("Large renamed")),
        ("overrides[e_b/e_000001ff/Stats.hp]", json!(37)),
    ] {
        let op = set(&local, "sc_test", "e_3", path, Some(value));
        local
            .apply_ops(std::slice::from_ref(&op))
            .expect("large instance local");
        full_reference(&mut full, &[op]).expect("large instance full");
        same(&local, &full);
        assert_eq!(local.mapping, handles);
    }
    assert_eq!(
        local
            .component("e_3/e_b/e_000001ff", "Stats")
            .expect("read")
            .expect("stats")["hp"],
        json!(37)
    );
}
