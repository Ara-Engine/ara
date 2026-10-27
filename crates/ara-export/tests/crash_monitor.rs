#![allow(clippy::unwrap_used)]

#[test]
fn verification_keeps_the_monitor_startup_cause_even_with_a_successful_result() {
    let diagnostic = "ARA-EXP-CRASH-004: crash monitor unavailable: socket path too long: 163 bytes (maximum 103)";
    for output in [
        format!("{diagnostic}\nARA_RESULT {{\"verified\":true}}\n"),
        format!("ara-player crash monitor: {diagnostic}\n"),
    ] {
        let error = ara_export::verify::runtime_result(&output).unwrap_err();
        assert_eq!(error.to_string(), diagnostic);
    }
    assert!(ara_export::verify::runtime_result("ARA_RESULT {\"verified\":true}\n").is_ok());
}
