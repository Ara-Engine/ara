//! Reject pre-readiness GPU windows and retain explicit timeout failures.
use serde_json::Value;

pub(crate) fn failure(player: &Value) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    pub(super) fn ready() -> Value {
        json!({"completed":true,"readiness":{"required":true,"state":"ready",
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
}
