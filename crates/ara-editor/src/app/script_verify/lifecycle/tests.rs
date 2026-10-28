//! Deliberately malformed external evidence must never pass the native proof.
#![allow(clippy::unwrap_used)]
use super::*;

fn target(producer: &str) -> Target {
    let diagnostic = ara_diag::Diagnostic::error(ara_diag::Code(903),MARKER).data(
        json!({"module":"Gameplay","play":{"world":"world-a","player":0,"source":"opaque-z","binding":4},
        "compile":{"schema":1,"phase":"diagnostic","channel":"compiler","producer":producer,
            "profile":"gameplay","instance":"instance-z","binding":2,"attempt":3}}));
    Target::parse(&json!({"seq":8,"first":7,"diagnostic":diagnostic})).unwrap()
}
fn complete(target: &Target) -> Value {
    let order = json!({"playBinding":4,"compileBinding":2,"attempt":4});
    json!({"key":target.key,"bound":{"playBinding":4,"compileBinding":2,
        "instance":"instance-z","source":"opaque-z"},"order":order,"covered":order,
        "lastVerifiedCompletion":{"order":order,"result":"success","validation":"current-module-inputs"},
        "phase":"success","pending":false,"incomplete":false,"unknown":false,
        "omittedCurrentRows":false,"errors":0,"activeErrorSeqs":[],
        "attempt":{"started":true,"tainted":false,"unknownCodeFamily":false,"verified":true,
            "result":"success","truncated":false,"total":1,"emitted":1,"observed":1,"observedErrors":0}})
}
fn projection(scopes: Vec<Value>) -> Value {
    json!({"schema":1,"scopes":scopes,"readError":null,"unknownEarlierScopes":false,"invalidMetadata":0,
        "omittedScopeEvents":0,"omittedRows":0,"omittedProtectedRows":0,"protectedRuntimeFaultSeqs":[]})
}

#[test]
fn another_producer_player_or_module_cannot_retire_the_failed_scope() {
    let failed = target("project");
    let warm = target("warm");
    assert!(verify::scope(&failed, &projection(vec![complete(&warm)])).is_err());
    for (field, value) in [
        ("player", json!(1)),
        ("module", json!("Other")),
        ("world", json!("world-b")),
    ] {
        let mut other = complete(&failed);
        other["key"][field] = value;
        assert!(verify::scope(&failed, &projection(vec![other])).is_err());
    }
    let mut lifecycle = Lifecycle {
        captured: true,
        before: projection(vec![]),
        targets: vec![failed, warm],
        ..Default::default()
    };
    let only_warm = projection(vec![complete(&lifecycle.targets[1])]);
    assert!(!lifecycle.assess(&only_warm));
    let both = projection(lifecycle.targets.iter().map(complete).collect());
    assert!(lifecycle.assess(&both));
    assert!(
        !lifecycle.complete(),
        "JSON evidence alone cannot substitute native Problems and Console controls"
    );
}

#[test]
fn old_coverage_or_unobserved_partial_completion_is_not_a_clean_fix() {
    let failed = target("warm");
    assert!(verify::scope(&failed, &projection(vec![complete(&failed)])).is_ok());
    let mut stale = complete(&failed);
    stale["covered"]["attempt"] = json!(3);
    assert!(verify::scope(&failed, &projection(vec![stale])).is_err());
    for (field, value) in [
        ("started", json!(false)),
        ("tainted", json!(true)),
        ("unknownCodeFamily", json!(true)),
        ("verified", Value::Null),
        ("truncated", json!(true)),
        ("observed", json!(0)),
        ("observedErrors", json!(1)),
    ] {
        let mut partial = complete(&failed);
        partial["attempt"][field] = value;
        assert!(
            verify::scope(&failed, &projection(vec![partial])).is_err(),
            "{field}"
        );
    }
}

#[test]
fn opaque_identity_is_equality_only_and_requires_a_new_numeric_binding() {
    let failed = target("warm");
    let mut changed = complete(&failed);
    changed["bound"]["instance"] = json!("instance-a");
    changed["bound"]["source"] = json!("opaque-a");
    assert!(verify::scope(&failed, &projection(vec![changed.clone()])).is_err());
    for path in [
        "/bound/playBinding",
        "/order/playBinding",
        "/covered/playBinding",
        "/lastVerifiedCompletion/order/playBinding",
    ] {
        *changed.pointer_mut(path).unwrap() = json!(5);
    }
    assert!(verify::scope(&failed, &projection(vec![changed])).is_ok());
    let mut stale_bound = complete(&failed);
    stale_bound["bound"]["compileBinding"] = json!(3);
    assert!(verify::scope(&failed, &projection(vec![stale_bound])).is_err());
}

#[test]
fn missing_metadata_coverage_or_inventory_is_explicitly_incomplete() {
    let failed = target("project");
    let mut untagged = failed.entry.clone();
    untagged["diagnostic"]["data"]["compile"] = Value::Null;
    assert!(Target::parse(&untagged).is_err());
    let mut missing = complete(&failed);
    missing["activeErrorSeqs"] = Value::Null;
    assert!(verify::scope(&failed, &projection(vec![missing])).is_err());
    let baseline = projection(vec![]);
    for (field, value) in [
        ("readError", json!("read failed")),
        ("omittedScopeEvents", json!(1)),
        ("omittedRows", json!(1)),
        ("omittedProtectedRows", json!(1)),
        ("protectedRuntimeFaultSeqs", Value::Null),
    ] {
        let mut invalid = baseline.clone();
        invalid[field] = value;
        assert!(verify::inventory(&invalid, &baseline).is_err(), "{field}");
    }
}

#[test]
fn history_repeat_preserves_identity_but_replaced_or_missing_errors_do_not() {
    let old = target("warm").entry;
    let mut repeat = old.clone();
    repeat["seq"] = json!(99);
    assert_eq!(retained(&[repeat.clone()], &old).unwrap()["seq"], 99);
    repeat["diagnostic"]["message"] = json!("different error");
    assert!(retained(&[repeat], &old).is_err());
    assert!(retained(&[], &old).is_err());
}
