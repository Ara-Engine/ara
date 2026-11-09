//! Receipt validation is not GPU evidence; the native journey must obtain it live.
#![allow(clippy::unwrap_used)]
use super::*;
use ara_runtime_render::probes::ProbeState;

#[test]
fn requested_or_stale_debug_modes_cannot_stand_in_for_completed_renderer_output() {
    for value in [
        Value::Null,
        json!({"mode":"gi_diffuse"}),
        json!({"mode":"gi_diffuse","observationSerial":1,"completed":true,"current":false}),
        json!({"mode":"lit","observationSerial":1,"completed":true,"current":true}),
    ] {
        assert!(debug_view(&value, "gi_diffuse").unwrap().is_null());
    }
    assert!(
        debug_view(
            &json!({"mode":"gi_diffuse","observationSerial":0,"completed":true,"current":true}),
            "gi_diffuse"
        )
        .is_err()
    );
    assert!(
        debug_view(
            &json!({"mode":"overdraw","observationSerial":2,"completed":true,"current":true}),
            "overdraw"
        )
        .is_err()
    );
    let completed = json!({"mode":"overdraw","observationSerial":2,"completed":true,"current":true,"overdraw":{"max":3,"pixels":10,"bands":[2,4,4,0,0,0]}});
    assert_eq!(debug_view(&completed, "overdraw").unwrap(), completed);
    let mut incomplete = completed.clone();
    incomplete["overdraw"]["pixels"] = 11.into();
    assert!(debug_view(&incomplete, "overdraw").is_err());
    let mut wrong_max = completed;
    wrong_max["overdraw"]["max"] = 16.into();
    assert!(debug_view(&wrong_max, "overdraw").is_err());
}

#[test]
fn published_receipt_requires_six_source_matching_faces_and_a_private_cache_destination() {
    let path = ".ara/cache/probes/validation-only.capture";
    let receipt = ProbeReceipt {
        doc: SCENE.into(),
        entity: "e_testprobe".into(),
        generation: 7,
        source_revision: 4,
        source_hash: "a".repeat(64),
        capture_hash: "b".repeat(64),
        resolution: 128,
        faces: 6,
        cache_path: path.into(),
        published: true,
    };
    let row = ProbeRow {
        doc: SCENE.into(),
        entity: receipt.entity.clone(),
        enabled: true,
        generation: 7,
        state: ProbeState::Done,
        stage: None,
        progress: Some(1.),
        completed_work: Some(6),
        total_work: Some(6),
        cancellable: false,
        scene_revision: 4,
        source_hash: receipt.source_hash.clone(),
        stale: false,
        retained_previous: false,
        last_receipt: Some(receipt.clone()),
        diagnostics: Vec::new(),
    };
    assert_eq!(
        published_receipt(&row, &receipt).unwrap()["publishedImageHashed"],
        true
    );
    let mut incomplete = receipt.clone();
    incomplete.faces = 5;
    assert!(published_receipt(&row, &incomplete).is_err());
    let mut wrong_source = receipt.clone();
    wrong_source.source_hash = "c".repeat(64);
    assert!(published_receipt(&row, &wrong_source).is_err());
    let mut stale = row.clone();
    stale.stale = true;
    assert!(published_receipt(&stale, &receipt).is_err());
    let mut outside = receipt;
    outside.cache_path = ".ara/cache/../../../outside.capture".into();
    assert!(published_receipt(&row, &outside).is_err());
}
