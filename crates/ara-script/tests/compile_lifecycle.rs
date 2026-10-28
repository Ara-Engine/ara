//! Actual compiler/loader JSON preserves lifecycle ownership without tagging runtime faults.
mod support;
use ara_diag::{Diagnostic, Subject};
use ara_script::codes;
use serde_json::{Value, json};
use support::Live;

fn metadata(diagnostic: &Diagnostic) -> &Value {
    diagnostic.data.as_deref().unwrap_or(&Value::Null)
}

const SOURCE: &str = r#"
/// <summary>Counter state in ECS.</summary>
[Component] public partial struct Probe { /// <summary>Ticks.</summary>
 public int Value; }
public static partial class Counting {
 [Query] static void Run(ref Probe p) { p.Value += 1; }
}
/// <summary>A runtime fault independent of compiler results.</summary>
public sealed partial class RuntimeFault : Behaviour {
 /// <inheritdoc/>
 protected override void OnTick(in Tick tick) => throw new InvalidOperationException("runtime fault stays separate");
}
"#;

#[test]
fn repair_completion_crosses_the_live_loader_with_module_attempt_binding_and_mapped_source() {
    let _clr = support::clr_lock();
    let mut live = Live::start("compile-lifecycle", &[("Combat", SOURCE, &[])]);
    let counter = live.spawn(&[("Probe", json!({"value":0}))]);
    live.spawn(&[("RuntimeFault", json!({}))]);
    live.until("runtime callback fault", |live| {
        live.diagnostics
            .iter()
            .any(|d| d.code == codes::BEHAVIOUR_FAULT)
    });
    let before = live.int(counter, "Probe", "value");
    live.edit("Combat", "p.Value += 1;", "p.Value += Missing;");
    live.until("scoped compiler error", |live| {
        live.diagnostics.iter().any(|d| {
            d.code == codes::COMPILE_ERROR
                && metadata(d)["module"] == "Combat"
                && metadata(d)["compile"]["phase"] == "diagnostic"
        })
    });
    let error = live
        .diagnostics
        .iter()
        .find(|d| d.code == codes::COMPILE_ERROR)
        .expect("compiler error");
    assert!(
        error
            .location
            .file
            .as_deref()
            .is_some_and(|file| file.replace('\\', "/").ends_with("Combat/Code.cs"))
    );
    assert!(error.location.line.is_some());
    assert!(error.location.column.is_some());
    assert!(error.subjects.contains(&Subject::module("Combat")));
    let attempt = metadata(error)["compile"]["attempt"]
        .as_u64()
        .expect("attempt");
    let instance = metadata(error)["compile"]["instance"].clone();
    let binding = metadata(error)["compile"]["binding"].clone();
    live.tick();
    assert!(
        live.int(counter, "Probe", "value") > before,
        "last-good executable keeps ticking"
    );
    live.edit("Combat", "p.Value += Missing;", "p.Value += 3;");
    live.until("authoritative repair completion", |live| {
        live.diagnostics.iter().any(|d| {
            let data = metadata(d);
            d.code == codes::COMPILE_RESULT
                && data["module"] == "Combat"
                && data["compile"]["phase"] == "completed"
                && data["compile"]["result"] == "success"
                && data["compile"]["validation"] == "current-module-inputs"
                && data["compile"]["attempt"]
                    .as_u64()
                    .is_some_and(|value| value > attempt)
        })
    });
    let completed = live
        .diagnostics
        .iter()
        .rev()
        .find(|d| d.code == codes::COMPILE_RESULT && metadata(d)["compile"]["phase"] == "completed")
        .expect("completion");
    assert_eq!(metadata(completed)["compile"]["instance"], instance);
    assert_eq!(metadata(completed)["compile"]["binding"], binding);
    assert!(
        live.diagnostics
            .iter()
            .any(|d| d.code == codes::COMPILE_RESULT && metadata(d)["compile"]["phase"] == "bound")
    );
    let fault = live
        .diagnostics
        .iter()
        .find(|d| d.code == codes::BEHAVIOUR_FAULT)
        .expect("separate runtime fault");
    assert!(
        metadata(fault)["compile"].is_null(),
        "runtime failures are not compiler contributions"
    );
}
