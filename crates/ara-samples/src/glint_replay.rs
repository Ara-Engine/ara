//! `glint-replay`: plays a recorded input list into Glint frame-exactly and checks the result.
//!
//! The list (`samples/glint/replay/*.json`) holds `action`/`value` injections at exact fixed
//! ticks. The engine steps with `ara_play step`, so a tick is the unit of time: no wall clock,
//! no frame rate, no device. The run's state hash after the last step, the finish time in
//! ticks and the number of falls are compared with the file's `expect`; `--record` rewrites
//! `expect` from the run. The same hash makes two runs on one machine byte-identical; it is
//! not promised across CPUs or builds (see the sample's README on what could break it).
use crate::play::Mcp;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
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

/// What a faithful replay must produce.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Expect {
    /// Run clock, in fixed ticks, when the goal was reached.
    finish_ticks: u64,
    /// Falls in that run.
    falls: u64,
    /// The world state hash after the last tick.
    hash: String,
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
}

/// Runs the replay, prints the finish time and either checks or records `expect`.
pub fn run(request: &Request<'_>) -> Result<()> {
    let text = std::fs::read_to_string(request.file)
        .with_context(|| format!("reading {}", request.file.display()))?;
    let mut replay: Replay = serde_json::from_str(&text).context("parsing the replay")?;
    if replay.format != "glint-replay/1" {
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
    let (finish_ticks, falls) = run.finish.context("the replay never reached the goal")?;
    let got = Expect {
        finish_ticks,
        falls,
        hash: run.hash.clone(),
    };
    let seconds = finish_ticks as f64 / 60.0;
    run.mcp.play("stop", json!({"world": run.world}))?;
    println!(
        "finished in {finish_ticks} ticks ({seconds:.2} s), {falls} fall(s), end state hash {}",
        got.hash
    );
    if request.record {
        replay.expect = Some(got);
        std::fs::write(
            request.file,
            ara_json::to_canonical_string(&serde_json::to_value(&replay)?),
        )?;
        println!("recorded expect into {}", request.file.display());
        return Ok(());
    }
    match replay.expect {
        Some(expect) if expect == got => {
            println!("replay matches: identical finish time and state hash")
        }
        Some(expect) => bail!("replay diverged: expected {expect:?}, got {got:?}"),
        None => bail!("the replay has no `expect`; run once with --record"),
    }
    Ok(())
}

/// One headless world being stepped.
struct Run<'a> {
    mcp: &'a mut Mcp,
    world: Value,
    tick: u64,
    hash: String,
    /// `(finish ticks, falls)` once the run state reads finished.
    finish: Option<(u64, u64)>,
    shots: Vec<Shot>,
    out: Option<PathBuf>,
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
                json!({"world": self.world, "paths": ["e_run.GlintRun.state", "e_run.GlintRun.ticks", "e_run.GlintRun.falls"]}),
            )?;
            let v = &read["values"];
            if v["e_run.GlintRun.state"].as_u64() == Some(2) {
                self.finish = Some((
                    v["e_run.GlintRun.ticks"].as_u64().unwrap_or(0),
                    v["e_run.GlintRun.falls"].as_u64().unwrap_or(0),
                ));
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
                self.mcp.play(
                    "capture",
                    json!({"world": self.world, "path": path.to_string_lossy(), "width": 1280, "height": 720, "marks": 0}),
                )?;
                println!("captured {} at tick {}", shot.name, self.tick);
            }
        }
        Ok(())
    }
}
