//! Pure fail-closed evidence checks, not simulated native/backend acceptance.
use super::*;
fn report() -> Value {
    json!({"selected":{"world":"play:fixture","player":1,"source":"incarnation-A","binding":1},
        "current":{"target":{"world":"play:fixture","player":1,"source":"incarnation-A","binding":1},"frames":3,"closedFrames":3,"minDurationNs":1,"cpuSpanCount":3,"hasPlayerFrameScope":true,"frameDigest":"a".repeat(64)}})
}
#[test]
fn valid_scoped_completed_capture_is_accepted() {
    let value = report();
    assert!(proof::validate_capture(&value, &value["selected"]).is_ok());
}
#[test]
fn another_incarnation_never_authorizes_capture() {
    let mut value = report();
    value["current"]["target"]["source"] = json!("incarnation-B");
    assert!(proof::validate_capture(&value, &value["selected"]).is_err());
}
#[test]
fn editor_or_missing_data_never_stands_in_for_player() {
    let mut value = report();
    value["current"]["target"] = Value::Null;
    assert!(proof::validate_capture(&value, &value["selected"]).is_err());
}
#[test]
fn open_zero_duration_frames_are_rejected() {
    let mut value = report();
    value["current"]["closedFrames"] = json!(2);
    assert!(proof::validate_capture(&value, &value["selected"]).is_err());
    value["current"]["closedFrames"] = json!(3);
    value["current"]["minDurationNs"] = json!(0);
    assert!(proof::validate_capture(&value, &value["selected"]).is_err());
}
#[test]
fn response_beyond_real_player_capacity_is_rejected() {
    let mut value = report();
    value["current"]["frames"] = json!(121);
    value["current"]["closedFrames"] = json!(121);
    assert!(proof::validate_capture(&value, &value["selected"]).is_err());
}
#[test]
fn missing_real_player_scopes_are_rejected() {
    let mut value = report();
    value["current"]["hasPlayerFrameScope"] = json!(false);
    assert!(proof::validate_capture(&value, &value["selected"]).is_err());
}
#[test]
fn missing_trusted_binding_is_rejected() {
    let mut value = report();
    value["selected"]["binding"] = Value::Null;
    value["current"]["target"] = value["selected"].clone();
    assert!(proof::validate_capture(&value, &value["selected"]).is_err());
}
