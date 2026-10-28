//! Reject pre-readiness GPU windows and retain explicit timeout failures.
use serde_json::Value;

pub(crate) fn failure(player: &Value) -> Option<String> {
    if player["completed"] == true
        && let Err(error) = validate_passes(player)
    {
        return Some(error);
    }
    if player["pass_presence"]["status"] == "invalid" {
        return Some(format!(
            "{}: {}; rt present: {}",
            ara_bench::passes::INVALID_PASS_SET,
            player["pass_presence"]["failure"],
            player["pass_presence"]["rt_present"]
        ));
    }
    let ready = &player["readiness"];
    (ready["required"] == true && matches!(ready["state"].as_str(), Some("timed_out" | "failed")))
        .then(|| {
            let description = if ready["state"] == "timed_out" {
                format!("renderer readiness timed out ({} ms)", ready["timeout_ms"])
            } else {
                "presentation failed after readiness".into()
            };
            format!(
                "{description}; last status {}; presentation error {}",
                ready.get("current_renderer").unwrap_or(&ready["renderer"]),
                ready["presentation_error"],
            )
        })
}

pub(crate) fn validate(player: &Value) -> Result<(), String> {
    if let Some(reason) = failure(player) {
        return Err(reason);
    }
    validate_passes(player)?;
    let ready = &player["readiness"];
    if player["completed"] != true
        || ready["required"] != true
        || ready["state"] != "ready"
        || ready["renderer_ready"] != true
        || ready["renderer"]["raster"] != true
        || ready["renderer"]["gi_enabled"] != true
    {
        return Err("no complete, GI-enabled raster readiness window".into());
    }
    let first = ready["ready_frame"]
        .as_u64()
        .ok_or("missing readiness frame")?;
    let warmup = ready["warmup_frames"]
        .as_u64()
        .ok_or("missing readiness warmup")?;
    let start = ready["gpu_start_frame"]
        .as_u64()
        .ok_or("missing steady GPU start frame")?;
    let minimum = first
        .checked_add(warmup)
        .and_then(|v| v.checked_add(1))
        .ok_or("invalid readiness boundary")?;
    if start < minimum {
        return Err("GPU window starts before readiness plus warmup".into());
    }
    let elapsed = ready["time_to_ready_ms"]
        .as_f64()
        .ok_or("missing time to readiness")?;
    let timeout = ready["timeout_ms"]
        .as_f64()
        .ok_or("missing readiness timeout")?;
    if !elapsed.is_finite() || elapsed < 0. || !timeout.is_finite() || timeout <= elapsed {
        return Err("readiness did not arrive before its deadline".into());
    }
    Ok(())
}

/// Even a plausible aggregate is invalid unless every required pass covered every frame.
pub(crate) fn validate_passes(player: &Value) -> Result<(), String> {
    let evidence = &player["pass_presence"];
    let frames = player["frames"].as_u64().unwrap_or(0);
    ara_bench::passes::validate(evidence, frames).map_err(|error| error.to_string())?;
    let required = evidence["required_passes"].as_array().ok_or_else(|| {
        format!(
            "{}: missing required pass list",
            ara_bench::passes::INVALID_PASS_SET
        )
    })?;
    for name in required {
        let name = name.as_str().ok_or_else(|| {
            format!(
                "{}: malformed required pass",
                ara_bench::passes::INVALID_PASS_SET
            )
        })?;
        if player["gpu_passes"][name]["samples"].as_u64() != Some(frames) {
            return Err(format!(
                "{}: {name} did not execute in every timed frame",
                ara_bench::passes::INVALID_PASS_SET
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    pub(super) fn ready() -> Value {
        json!({"completed":true,"frames":1,"pass_presence":{"format":"ara.timing.passes/1","expected":"raster","status":"valid","timed_frames":1,"rt_present_frames":0,"required_passes":[]},"readiness":{"required":true,"state":"ready",
            "renderer_ready":true,"renderer":{"raster":true,"gi_enabled":true},
            "ready_frame":430,"warmup_frames":60,"gpu_start_frame":491,
            "time_to_ready_ms":2000.,"timeout_ms":120000.}})
    }
    #[test]
    fn gpu_window_requires_readiness_then_every_warmup_frame() {
        let mut player = ready();
        assert!(validate(&player).is_ok());
        player["readiness"]["gpu_start_frame"] = json!(490);
        assert!(validate(&player).is_err());
        player["readiness"]["gpu_start_frame"] = json!(491);
        player["readiness"]["renderer"]["gi_enabled"] = json!(false);
        assert!(validate(&player).is_err());
    }
    #[test]
    fn timeout_is_an_explicit_failure_even_with_plausible_gpu_numbers() {
        let mut player = ready();
        for state in ["timed_out", "failed"] {
            player["readiness"]["state"] = json!(state);
            assert!(failure(&player).is_some());
            assert!(validate(&player).is_err());
        }
        assert!(validate(&json!({})).is_err());
    }
    #[test]
    fn plausible_ssr_only_window_cannot_satisfy_expected_rt() {
        let mut player = ready();
        player["frames"] = json!(240);
        player["gpu_passes"] = json!({"SSR":{"samples":240,"p50":1.5}});
        player["pass_presence"] = json!({"format":"ara.timing.passes/1","expected":"hybrid","status":"invalid","timed_frames":240,"rt_present_frames":239});
        assert!(validate(&player).is_err());
    }
    #[test]
    fn aggregate_pass_coverage_must_agree_with_the_per_frame_proof() {
        use ara_bench::passes::{Expected, Frame, Guard};
        let names = [
            "RT.Reflect",
            "RT.Filter.X",
            "RT.Filter.Y",
            "RT.Rough",
            "RT.Rough.X",
            "RT.Rough.Y",
        ];
        let mut guard = Guard::new(Expected::Hybrid, 1, std::time::Duration::from_secs(1));
        guard
            .wait(std::time::Duration::ZERO, Some(Frame::from_names(names)))
            .unwrap();
        guard.record(Frame::from_names(names)).unwrap();
        let mut player = ready();
        player["pass_presence"] = guard.report();
        for name in names {
            player["gpu_passes"][name] = json!({"samples":1});
        }
        assert!(validate(&player).is_ok());
        player["gpu_passes"]["RT.Reflect"]["samples"] = json!(0);
        assert!(
            validate(&player)
                .unwrap_err()
                .starts_with(&format!("{}:", ara_bench::passes::INVALID_PASS_SET))
        );
    }
}
