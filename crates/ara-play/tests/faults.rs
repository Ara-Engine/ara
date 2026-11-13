//! A step reports the script faults raised while it ran.
#![allow(clippy::unwrap_used)]
use ara_play::{Event, Stepped, faults};
use serde_json::json;

#[test]
fn only_error_diagnostics_are_faults() {
    let fault =
        json!({"code": "ARA0911", "severity": "error", "message": "behaviour `Camera` threw"});
    let events = [
        Event::Diagnostic {
            diagnostic: json!({"code": "ARA7310", "severity": "warning", "message": "placeholder geometry"}),
        },
        Event::Restart,
        Event::Diagnostic {
            diagnostic: fault.clone(),
        },
    ];
    assert_eq!(faults(&events), vec![fault]);
}

#[test]
fn a_clean_step_serializes_as_before_and_old_results_still_parse() {
    let clean = Stepped {
        ticks: 4,
        tick: 4,
        reached: false,
        hash: "00".into(),
        samples: Vec::new(),
        faults: Vec::new(),
    };
    let value = serde_json::to_value(&clean).unwrap();
    assert!(value.get("faults").is_none());
    assert_eq!(serde_json::from_value::<Stepped>(value).unwrap(), clean);

    let faulted = Stepped {
        faults: vec![json!({"code": "ARA0911", "severity": "error"})],
        ..clean
    };
    let value = serde_json::to_value(&faulted).unwrap();
    assert_eq!(value["faults"][0]["code"], "ARA0911");
    assert_eq!(serde_json::from_value::<Stepped>(value).unwrap(), faulted);
}
