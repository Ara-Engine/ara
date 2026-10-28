use super::*;
use ara_diag::{
    bus::Meta,
    store::{Filter, Store},
};
use ara_script::codes::{COMPILE_ERROR, COMPILE_RESULT};

#[test]
fn identical_compiler_rows_stay_distinct_by_world_player_attempt_and_replacement_source() {
    let mut sources = DiagnosticSources::default();
    let mut store = Store::default();
    let row = || {
        Diagnostic::error(COMPILE_ERROR, "CS1002: ; expected")
        .subject(Subject::module("Combat"))
        .subject(Subject::system("csharp.compile/compiler/41"))
        .data(json!({"module":"Combat","compile":{"instance":"compiler","attempt":41},"custom":{"keep":true},"play":{"world":"spoofed"}}))
    };
    for (world, player, source) in [
        ("world-a", 0, "process-a"),
        ("world-a", 1, "process-b"),
        ("world-b", 0, "process-c"),
    ] {
        store.push(
            sources
                .scope(world, player, source, row())
                .expect("trusted scope"),
            Meta::default(),
        );
    }
    assert_eq!(
        store.len(),
        3,
        "data-only scope would have deduplicated these rows"
    );
    let replacement = sources
        .scope("world-a", 0, "process-d", row())
        .expect("new incarnation");
    let replacement_binding = replacement.data.as_ref().expect("metadata")["play"]["binding"]
        .as_u64()
        .expect("binding");
    store.push(replacement, Meta::default());
    assert_eq!(store.len(), 4);
    let records = store.query(&Filter::default());
    let original = records
        .iter()
        .find(|r| r.diagnostic.data.as_ref().expect("metadata")["play"]["source"] == "process-a")
        .expect("original source");
    let original_binding = original.diagnostic.data.as_ref().expect("metadata")["play"]["binding"]
        .as_u64()
        .expect("binding");
    assert!(replacement_binding > original_binding);
    for record in records {
        let data = record.diagnostic.data.as_ref().expect("metadata");
        assert_eq!(data["custom"]["keep"], true);
        assert_eq!(data["reportedPlay"]["world"], "spoofed");
        assert_ne!(data["play"]["world"], "spoofed");
        assert_eq!(record.diagnostic.message, "CS1002: ; expected");
    }
    let success = Diagnostic::info(COMPILE_RESULT, "Combat compilation completed")
        .subject(Subject::module("Combat"))
        .subject(Subject::system("csharp.compile/compiler/42"))
        .data(json!({"compile":{"attempt":42,"phase":"completed","result":"success"}}));
    store.push(
        sources
            .scope("world-a", 0, "process-d", success)
            .expect("same source"),
        Meta::default(),
    );
    assert_eq!(
        store.len(),
        5,
        "success never deletes Console history or another source's diagnostics"
    );
}
