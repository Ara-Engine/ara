//! Read the real ECS at authority ticks; no wall-clock rotation estimates.
use crate::Editor;
use serde_json::{Value, json};

pub(super) fn job(
    editor: &mut Editor,
    pending: &mut Option<String>,
) -> ara_ui::Result<Option<Value>> {
    let Some(id) = pending.as_deref() else {
        return Ok(None);
    };
    let status = editor.action("jobs.status", json!({"job":id}))?;
    match status["status"].as_str() {
        Some("running" | "queued") => Ok(None),
        Some("done") => {
            if status.get("result").is_none() {
                return Err("J10 job completed without a result".into());
            }
            pending.take();
            Ok(Some(status["result"].clone()))
        }
        _ => Err(format!("J10 job did not succeed: {status}").into()),
    }
}

struct Sample {
    tick: u64,
    rotation_y: f64,
}
#[derive(Default)]
pub(super) struct Probe {
    pending: Option<String>,
    first: Option<Sample>,
}
impl Probe {
    pub fn poll(
        &mut self,
        editor: &mut Editor,
        world: &str,
        entity: &str,
        expected: f64,
    ) -> ara_ui::Result<Option<Value>> {
        let path = format!("{entity}.Transform.rotation");
        if self.pending.is_none() {
            let response = editor.action(
                "play.read",
                json!({"world":world,"paths":[path],"background":true}),
            )?;
            self.pending = Some(
                response["job"]
                    .as_str()
                    .ok_or("play.read returned no background job")?
                    .into(),
            );
            return Ok(None);
        }
        let Some(result) = job(editor, &mut self.pending)? else {
            return Ok(None);
        };
        let tick = result["tick"]
            .as_u64()
            .ok_or("play.read returned no authoritative tick")?;
        let rotation = result["values"][&path]
            .as_array()
            .filter(|v| v.len() == 3)
            .ok_or("play.read did not return the entity's real Transform.rotation vector")?;
        let rotation_y = rotation[1]
            .as_f64()
            .filter(|n| n.is_finite())
            .ok_or("runtime rotation is not finite")?;
        let sample = Sample { tick, rotation_y };
        let Some(first) = &self.first else {
            self.first = Some(sample);
            return Ok(None);
        };
        let ticks = sample
            .tick
            .checked_sub(first.tick)
            .ok_or("Play tick moved backwards during J10 measurement")?;
        if ticks < 30 {
            return Ok(None);
        }
        let delta = sample.rotation_y - first.rotation_y;
        let rate = delta / ticks as f64;
        let tolerance = (expected.abs() * 0.02).max(0.01);
        let measured = json!({"startTick":first.tick,"endTick":sample.tick,"ticks":ticks,
            "startRotationY":first.rotation_y,"endRotationY":sample.rotation_y,"deltaDegrees":delta,
            "degreesPerTick":rate,"expectedDegreesPerTick":expected,"tolerance":tolerance,
            "passed":(rate-expected).abs()<=tolerance,"ingress":"play.read background jobs; real runtime ECS, not a native gesture"});
        self.first = None;
        Ok(Some(measured))
    }
}
