//! `glint-replay`: plays a recorded input list into Glint frame-exactly and checks the result.
//!
//! The list (`samples/glint/replay/*.json`) holds `action`/`value` injections at exact fixed
//! ticks. The engine steps with `ara_play step`, so a tick is the unit of time: no wall clock,
//! no frame rate, no device. The finish time in ticks, the falls and the orbs collected are
//! compared with the file's `expect` on every platform; the state hash after the last step
//! is compared with the hash recorded for this platform, if there is one. `--record` rewrites
//! the result and this platform's hash from the run. The same hash makes two runs on one
//! machine byte-identical; it is not promised across CPUs or builds (see the sample's README
//! on what could break it).
use crate::play::Mcp;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One injected action at a fixed tick, held for `hold` ticks.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Event {
    tick: u64,
    action: String,
    value: Value,
    hold: u64,
}

/// A still to capture once the run reaches `tick`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Shot {
    tick: u64,
    name: String,
}

/// How a run ended: what every platform must reproduce.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Outcome {
    /// Run clock, in fixed ticks, when the goal was reached.
    finish_ticks: u64,
    /// Falls in that run.
    falls: u64,
    /// Orbs collected, and how many the course has.
    orbs: u64,
    orb_total: u64,
}

/// What a faithful replay must produce.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Expect {
    #[serde(flatten)]
    outcome: Outcome,
    /// The world state hash after the last tick, per platform (`os-arch`). A hash is only
    /// compared on the platform it was recorded on: floating point in the physics solver
    /// and in `MathF` differs between platforms and compilers.
    #[serde(default)]
    hashes: BTreeMap<String, String>,
}

/// This machine as the replay names it.
fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// The replay file.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Replay {
    format: String,
    scene: String,
    seed: u64,
    /// Ticks one input chunk covers (the length the file was recorded with).
    chunk: u64,
    /// The last tick to run.
    end_tick: u64,
    events: Vec<Event>,
    #[serde(default)]
    captures: Vec<Shot>,
    #[serde(default)]
    expect: Option<Expect>,
}

/// What `run` does.
pub struct Request<'a> {
    /// Project root (`samples/glint`).
    pub root: &'a Path,
    /// The replay file.
    pub file: &'a Path,
    /// Rewrite `expect` from this run instead of checking it.
    pub record: bool,
    /// Where stills go (the file's `captures`), if anywhere.
    pub captures: Option<PathBuf>,
    /// Live frames to play at each still, so its timings are steady state.
    pub hold: Option<u32>,
    /// With `hold`: metres the camera travels along its heading over those frames (a clip).
    pub travel: Option<f32>,
    /// Capture size, pixels.
    pub size: [u32; 2],
    /// A WAV to render the end of the run's sound into, checked for a signal.
    pub audio: Option<PathBuf>,
}

/// Renders three seconds of the world's sound offline (no device) into `path` and returns the
/// master bus's peak and RMS in dBFS, or an error if the mixer produced silence.
fn sound(mcp: &mut Mcp, world: &Value, path: &Path) -> Result<(f64, f64)> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    mcp.play(
        "captureAudio",
        json!({"world": world, "path": path.to_string_lossy(), "seconds": 3.0}),
    )?;
    // The analysis is written beside the WAV as `<name>.audio.json`.
    let analysis = path.with_extension("audio.json");
    let report: Value = serde_json::from_str(
        &std::fs::read_to_string(&analysis)
            .with_context(|| format!("reading {}", analysis.display()))?,
    )?;
    let master = report["buses"]
        .as_array()
        .and_then(|buses| buses.iter().find(|bus| bus["name"] == "master"))
        .context("the audio analysis has no master bus")?;
    match (master["peakDbfs"].as_f64(), master["rmsDbfs"].as_f64()) {
        (Some(peak), Some(rms)) => Ok((peak, rms)),
        _ => bail!(
            "the mixer rendered silence: the master bus has no signal in {}",
            path.display()
        ),
    }
}

/// Runs the replay, prints the finish time and either checks or records `expect`.
pub fn run(request: &Request<'_>) -> Result<()> {
    let text = std::fs::read_to_string(request.file)
        .with_context(|| format!("reading {}", request.file.display()))?;
    let mut replay: Replay = serde_json::from_str(&text).context("parsing the replay")?;
    if replay.format != "glint-replay/2" {
        bail!("unknown replay format {}", replay.format);
    }
    std::fs::create_dir_all(request.root.join(".ara/samples"))?;
    let mut mcp = Mcp::start(request.root)?;
    let start = mcp.play(
        "start",
        json!({"scene": replay.scene, "window": false, "startPaused": true, "seed": replay.seed}),
    )?;
    let world = start["world"].clone();
    let mut run = Run {
        mcp: &mut mcp,
        world,
        tick: 0,
        hash: String::new(),
        finish: None,
        shots: replay.captures.clone(),
        out: request.captures.clone(),
        hold: request.hold,
        travel: request.travel,
        size: request.size,
    };
    let mut events = replay.events.clone();
    events.sort_by_key(|e| e.tick);
    let mut at = 0;
    while at < events.len() {
        let tick = events[at].tick;
        let group: Vec<&Event> = events[at..].iter().take_while(|e| e.tick == tick).collect();
        run.advance(tick)?;
        let inputs: Vec<Value> = group
            .iter()
            .map(|e| json!({"action": e.action, "value": e.value, "holdTicks": e.hold}))
            .collect();
        run.step(replay.chunk, Some(inputs))?;
        at += group.len();
    }
    run.advance(replay.end_tick)?;
    let outcome = run.finish.context("the replay never reached the goal")?;
    let (hash, here) = (run.hash.clone(), platform());
    if let Some(path) = &request.audio {
        let (peak, rms) = sound(run.mcp, &run.world, path)?;
        println!(
            "sound: 3 s rendered to {}, master bus peak {peak:.1} dBFS, rms {rms:.1} dBFS",
            path.display()
        );
    }
    let seconds = outcome.finish_ticks as f64 / 60.0;
    run.mcp.play("stop", json!({"world": run.world}))?;
    println!(
        "finished in {} ticks ({seconds:.2} s), {} fall(s), {} of {} orbs, end state hash {hash}",
        outcome.finish_ticks, outcome.falls, outcome.orbs, outcome.orb_total
    );
    if request.record {
        // Other platforms' hashes stay only while they describe the same run.
        let mut hashes = match replay.expect.take() {
            Some(old) if old.outcome == outcome => old.hashes,
            _ => BTreeMap::new(),
        };
        hashes.insert(here, hash);
        replay.expect = Some(Expect { outcome, hashes });
        std::fs::write(
            request.file,
            ara_json::to_canonical_string(&serde_json::to_value(&replay)?),
        )?;
        println!("recorded expect into {}", request.file.display());
        return Ok(());
    }
    let expect = replay
        .expect
        .context("the replay has no `expect`; run once with --record")?;
    if expect.outcome != outcome {
        bail!(
            "replay diverged: expected {:?}, got {outcome:?}. The same inputs no longer reach the goal the same way: record again with `samples/glint/replay/autopilot.py`, which plays closed-loop.",
            expect.outcome
        );
    }
    match expect.hashes.get(&here) {
        Some(recorded) if *recorded == hash => {
            println!("replay matches: identical finish time, falls, orbs and state hash")
        }
        Some(recorded) => bail!(
            "replay diverged: same finish time, falls and orbs, but the state hash recorded for {here} is {recorded}. Something that does not change the result changed the world (a scene edit, an engine change): run --record if that was intended."
        ),
        None => println!(
            "replay matches: identical finish time, falls and orbs (no state hash is recorded for {here}; recorded for {}; run --record here to pin this machine's hash)",
            expect.hashes.keys().cloned().collect::<Vec<_>>().join(", ")
        ),
    }
    Ok(())
}

/// One headless world being stepped.
struct Run<'a> {
    mcp: &'a mut Mcp,
    world: Value,
    tick: u64,
    hash: String,
    /// How the run ended, once the run state reads finished.
    finish: Option<Outcome>,
    shots: Vec<Shot>,
    out: Option<PathBuf>,
    hold: Option<u32>,
    travel: Option<f32>,
    size: [u32; 2],
}

impl Run<'_> {
    /// Steps plain to `target`, stopping at each capture tick on the way.
    fn advance(&mut self, target: u64) -> Result<()> {
        while self.tick < target {
            let next = self
                .shots
                .iter()
                .map(|s| s.tick)
                .filter(|t| *t > self.tick && *t <= target)
                .min()
                .unwrap_or(target);
            self.step(next - self.tick, None)?;
        }
        Ok(())
    }

    /// One `ara_play step`, then the finish check and any capture due at the new tick.
    fn step(&mut self, ticks: u64, inputs: Option<Vec<Value>>) -> Result<()> {
        let mut args = json!({"world": self.world, "ticks": ticks});
        if let Some(inputs) = inputs {
            args["inputs"] = json!(inputs);
        }
        let result = self.mcp.play("step", args)?;
        self.tick = result["tick"].as_u64().unwrap_or(self.tick + ticks);
        self.hash = result["hash"].as_str().unwrap_or_default().to_owned();
        if self.finish.is_none() {
            let read = self.mcp.play(
                "read",
                json!({"world": self.world, "paths": ["e_run.GlintRun.state", "e_run.GlintRun.ticks", "e_run.GlintRun.falls", "e_run.GlintRun.orbs", "e_run.GlintRun.orbTotal"]}),
            )?;
            let v = &read["values"];
            if v["e_run.GlintRun.state"].as_u64() == Some(2) {
                let field = |name: &str| v[format!("e_run.GlintRun.{name}")].as_u64().unwrap_or(0);
                self.finish = Some(Outcome {
                    finish_ticks: field("ticks"),
                    falls: field("falls"),
                    orbs: field("orbs"),
                    orb_total: field("orbTotal"),
                });
            }
        }
        let due: Vec<Shot> = self
            .shots
            .iter()
            .filter(|s| s.tick <= self.tick)
            .cloned()
            .collect();
        self.shots.retain(|s| s.tick > self.tick);
        for shot in due {
            if let Some(dir) = &self.out {
                std::fs::create_dir_all(dir)?;
                let path = dir.join(format!("{}.png", shot.name));
                let mut capture = json!({"world": self.world, "path": path.to_string_lossy(),
                    "width": self.size[0], "height": self.size[1], "marks": 0});
                match (self.hold, self.travel) {
                    // A clip: the camera moves along the follow rig's heading.
                    (Some(frames), Some(metres)) => {
                        let read = self.mcp.play(
                            "read",
                            json!({"world": self.world, "paths": ["e_camera.FollowCamera.yaw"]}),
                        )?;
                        let yaw = read["values"]["e_camera.FollowCamera.yaw"]
                            .as_f64()
                            .unwrap_or(0.0)
                            .to_radians();
                        capture["video"] = json!(frames);
                        capture["videoMove"] =
                            json!([yaw.sin() * metres as f64, 0.0, yaw.cos() * metres as f64]);
                    }
                    (Some(frames), None) => capture["stability"] = json!(frames),
                    (None, _) => {}
                }
                self.mcp.play("capture", capture)?;
                println!("captured {} at tick {}", shot.name, self.tick);
            }
        }
        Ok(())
    }
}
