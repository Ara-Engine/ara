//! One-shot automation uses the same snapshots and players as interactive Play.
use crate::*;
use ara_schema::Schema;
use serde::Deserialize;

/// Run the main scene or a named scene without creating windows.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HeadlessRunParams {
    /// Calling session.
    pub session: String,
    /// Scene path or ID; defaults to the project's main scene.
    pub scene: Option<String>,
    /// Exact number of fixed ticks (default 60); mutually exclusive with duration.
    pub ticks: Option<u32>,
    /// Simulated seconds, rounded up to a fixed tick; mutually exclusive with ticks.
    pub duration: Option<f64>,
    /// Deterministic seed (default 0).
    pub seed: Option<u64>,
    /// Fixed tick frequency (default 60).
    pub tick_hz: Option<u32>,
}

/// Render a project scene into a PNG on an offscreen GPU device.
#[derive(Debug, Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SceneCaptureParams {
    /// Calling session.
    pub session: String,
    /// Scene path or ID; defaults to the project's main scene.
    pub scene: Option<String>,
    /// Camera entity ID or exact name; omitted uses the scene's active camera.
    pub camera: Option<String>,
    /// Output PNG path; relative paths resolve inside the project.
    pub path: Option<String>,
    /// Image width in pixels (default 1280, maximum 8192).
    pub width: Option<u32>,
    /// Image height in pixels (default 720, maximum 8192).
    pub height: Option<u32>,
    /// Rendering quality: auto, low, medium, high or epic.
    pub preset: Option<ara_play::QualitySetting>,
    /// Fixed ticks before capture (default 0); presentation settles deterministically.
    pub ticks: Option<u32>,
    /// Deterministic seed (default 0).
    pub seed: Option<u64>,
    /// Numbered entity marks (default 40; zero disables them).
    pub marks: Option<u32>,
}

fn tick_count(ticks: Option<u32>, duration: Option<f64>, hz: u32) -> Result<u32, Vec<Diagnostic>> {
    if duration.is_some_and(|seconds| !seconds.is_finite() || seconds < 0.) {
        return Err(fail(
            codes::REQUEST,
            "duration must be finite and nonnegative",
        ));
    }
    if ticks.is_some() && duration.is_some() {
        return Err(fail(codes::REQUEST, "give ticks or duration, not both"));
    }
    let count = duration.map_or_else(
        || f64::from(ticks.unwrap_or(60)),
        |s| (s * f64::from(hz)).ceil(),
    );
    if !count.is_finite() || !(0.0..=10_000_000.0).contains(&count) {
        return Err(fail(
            codes::REQUEST,
            "run length must be finite and between 0 and 10000000 ticks",
        ));
    }
    Ok(count as u32)
}

impl PlayService {
    /// One-shot CLI calls may exit immediately, so reap their players before replying.
    fn finish_automation(&mut self, id: &str) {
        let world = self
            .worlds
            .remove(id)
            .expect("automation owns its started world");
        lifecycle::shutdown(world);
        self.close(id);
    }
    fn headless_start(
        &mut self,
        cx: &Cx<'_>,
        session: String,
        scene: Option<String>,
        seed: Option<u64>,
        hz: Option<u32>,
    ) -> Result<String, Vec<Diagnostic>> {
        let started = self.start(
            cx,
            PlayStartParams {
                session,
                scene,
                background: None,
                mode: None,
                process_mode: None,
                clients: None,
                window: Some(false),
                embed: None,
                start_paused: Some(true),
                seed,
                tick_hz: hz,
                hot_reload: Some(false),
                conditions: None,
            },
        )?;
        Ok(started["world"]
            .as_str()
            .expect("play.start returns a world")
            .to_owned())
    }

    fn automation_result(&mut self, id: &str, mut result: Value) -> Result<Value, Vec<Diagnostic>> {
        let world = self.world(id)?;
        let logs: Vec<_> = world
            .players
            .iter()
            .map(|(_, player)| std::fs::read_to_string(&player.log).unwrap_or_default())
            .filter(|s| !s.is_empty())
            .collect();
        let mut events = std::mem::take(&mut world.events);
        for (_, player) in &mut world.players {
            for event in std::mem::take(&mut player.events) {
                events.push(
                    serde_json::to_value(event).map_err(|e| fail(codes::PLAY, e.to_string()))?,
                );
            }
        }
        result["logs"] = json!(logs);
        result["events"] = json!(events);
        result["diagnostics"] = json!(
            events
                .iter()
                .filter(|e| e["kind"] == "diagnostic")
                .map(|e| &e["diagnostic"])
                .collect::<Vec<_>>()
        );
        result["scene"] = json!(world.scene);
        result["text"] = json!(format!(
            "Ran {} to tick {} (hash {}).\n",
            world.scene, result["tick"], result["hash"]
        ));
        Ok(result)
    }

    pub(super) fn run_scene(
        &mut self,
        cx: &Cx<'_>,
        p: HeadlessRunParams,
    ) -> Result<Value, Vec<Diagnostic>> {
        let ticks = tick_count(
            p.ticks,
            p.duration,
            p.tick_hz.unwrap_or(self.config.tick_hz),
        )?;
        let id = self.headless_start(cx, p.session, p.scene, p.seed, p.tick_hz)?;
        let result = self
            .call(&id, method::STEP, json!({"ticks":ticks}), false)
            .and_then(|result| self.automation_result(&id, result));
        self.finish_automation(&id);
        result
    }

    pub(super) fn capture_scene(
        &mut self,
        cx: &Cx<'_>,
        p: SceneCaptureParams,
    ) -> Result<Value, Vec<Diagnostic>> {
        let size = [p.width.unwrap_or(1280), p.height.unwrap_or(720)];
        if size.iter().any(|v| !(1..=8192).contains(v)) {
            return Err(fail(
                codes::REQUEST,
                "capture dimensions must be between 1 and 8192 pixels",
            ));
        }
        let ticks = tick_count(p.ticks.or(Some(0)), None, self.config.tick_hz)?;
        let id = self.headless_start(cx, p.session, p.scene, p.seed, None)?;
        let path = self.root.join(
            p.path
                .unwrap_or_else(|| format!(".ara/captures/{}.png", id.replace(':', "-"))),
        );
        let result = (|| {
            self.call(&id, method::STEP, json!({"ticks":ticks}), false)?;
            let mut result = self.call(
                &id,
                method::CAPTURE,
                json!({
                    "path":path,"width":size[0],"height":size[1],"marks":p.marks.unwrap_or(40),
                    "camera":p.camera,"preset":p.preset,
                }),
                false,
            )?;
            result["path"] = json!(path);
            result["width"] = json!(size[0]);
            result["height"] = json!(size[1]);
            self.automation_result(&id, result)
        })();
        self.finish_automation(&id);
        let mut result = result?;
        result["text"] = json!(format!(
            "Captured {} at {}x{} to {}.\n",
            result["scene"],
            size[0],
            size[1],
            path.display()
        ));
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duration_is_simulation_time_and_invalid_lengths_are_rejected() {
        assert_eq!(tick_count(None, Some(0.025), 60).expect("duration"), 2);
        assert_eq!(tick_count(Some(12), None, 60).expect("ticks"), 12);
        assert!(tick_count(Some(12), Some(1.), 60).is_err());
        for seconds in [-1., -0.0001, f64::NAN, f64::INFINITY] {
            assert!(tick_count(None, Some(seconds), 60).is_err());
        }
    }
}
