//! Explicit recovery activates verified current inputs without advancing simulation.
mod support;

use ara_script::codes;
use serde_json::json;
use std::time::{Duration, Instant};
use support::Live;

const GAME: &str = r#"
namespace Recovery;
/// <summary>ECS-owned recovery probe.</summary>
[Component] public partial struct Probe {
    /// <summary>Completed updates.</summary>
    public int Count;
    /// <summary>Whether the update should fault.</summary>
    public int Fault;
}
public static partial class Updating {
    [Query] static void Run(ref Probe p) {
        if (p.Fault != 0) throw new System.InvalidOperationException("recovery probe");
        p.Count += 1;
    }
}
"#;

fn recover(live: &mut Live, ticket: u64) -> String {
    assert!(live.host.request_recovery(ticket));
    let frame = live.frame;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let boundary = live.host.tick_boundary(&mut live.world);
        assert!(
            boundary
                .reports
                .iter()
                .all(|report| report.kind != ara_script::abi::RELOAD_L4)
        );
        live.reports.extend(boundary.reports);
        live.diagnostics.extend(live.host.take_diagnostics());
        if let Some(outcome) = live.diagnostics.iter().find_map(|d| {
            let data = d.data.as_ref()?;
            (d.code == codes::RECOVERY_RESULT
                && data["recovery"]["ticket"].as_u64() == Some(ticket))
            .then(|| {
                data["recovery"]["outcome"]
                    .as_str()
                    .expect("typed outcome")
                    .to_owned()
            })
        }) {
            assert_eq!(live.frame, frame, "recovery must not step simulation");
            return outcome;
        }
        assert!(
            Instant::now() < deadline,
            "recovery timed out: {:?}",
            live.diagnostics
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn recovery_rearms_only_verified_code_and_preserves_ecs_while_paused() {
    let _clr = support::clr_lock();
    let mut live = Live::start("explicit-recovery", &[("Recovery", GAME, &[])]);
    let entity = live.spawn(&[("Probe", json!({"count":41,"fault":1}))]);
    live.run();
    assert!(live.diagnostics.iter().any(|d| {
        d.data
            .as_ref()
            .is_some_and(|data| data.get("exception").is_some())
    }));
    live.host
        .insert_component(
            &mut live.world,
            entity,
            "Probe",
            &json!({"count":41,"fault":0}),
        )
        .expect("clear ECS fault trigger");
    live.run();
    assert_eq!(
        live.int(entity, "Probe", "count"),
        41,
        "fault stays disabled until recovery"
    );
    assert_eq!(recover(&mut live, 101), "reloaded");
    assert_eq!(live.int(entity, "Probe", "count"), 41);
    live.run();
    assert_eq!(live.int(entity, "Probe", "count"), 42);

    live.host
        .insert_component(
            &mut live.world,
            entity,
            "Probe",
            &json!({"count":42,"fault":1}),
        )
        .expect("fault again");
    live.run();
    live.edit("Recovery", "p.Count += 1;", "p.Count += ;");
    assert_eq!(recover(&mut live, 102), "failed");
    live.host
        .insert_component(
            &mut live.world,
            entity,
            "Probe",
            &json!({"count":42,"fault":0}),
        )
        .expect("clear trigger after failed compile");
    live.run();
    assert_eq!(
        live.int(entity, "Probe", "count"),
        42,
        "failed compile must retain disabled state"
    );
    live.edit("Recovery", "p.Count += ;", "p.Count += 2;");
    assert_eq!(recover(&mut live, 103), "reloaded");
    live.run();
    assert_eq!(live.int(entity, "Probe", "count"), 44);
    live.finish();
}
