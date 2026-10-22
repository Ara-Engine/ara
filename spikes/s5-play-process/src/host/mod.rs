//! The "editor host" process: loads the edit world from a JSON scene, then drives player
//! processes through each measurement suite and writes a JSON report.

mod boot;
mod faults;
mod live;
mod players;
mod stats;

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Result, bail};
use bevy_ecs::world::World;
use serde_json::json;

use crate::model::{entity_count, state_hash};
use crate::protocol::hash_hex;
use crate::scene::SceneDoc;
use players::Host;
use stats::ms;

/// Every suite, in run order.
pub const SUITES: [&str; 6] = ["snapshot", "boot", "live", "keep", "crash", "multi"];

/// Command-line options of `ara-s5-play host`.
pub struct HostArgs {
    pub scene: PathBuf,
    pub out: PathBuf,
    pub reps: usize,
    pub patches: usize,
    pub suites: Vec<String>,
    /// Also measure players that create a Vulkan device at startup.
    pub gpu: bool,
}

/// State shared by the suites: the listener and the edit world.
pub struct Ctx {
    host: Host,
    edit: World,
    reps: usize,
}

pub fn run(args: HostArgs) -> Result<()> {
    let t = Instant::now();
    let text = std::fs::read(&args.scene)?;
    let read_ms = ms(t.elapsed());
    let doc: SceneDoc = serde_json::from_slice(&text)?;
    let mut edit = doc.into_world()?;
    let load_ms = ms(t.elapsed());
    let entities = entity_count(&mut edit);
    println!(
        "edit world: {entities} entities from {} ({} bytes) in {load_ms:.1} ms",
        args.scene.display(),
        text.len()
    );

    let host = Host::new()?;
    let exe_bytes = std::fs::metadata(host.exe())?.len();
    let mut report = json!({
        "scene": {
            "path": args.scene,
            "entities": entities,
            "fileBytes": text.len(),
            "readMs": read_ms,
            "parseAndSpawnMs": load_ms - read_ms,
            "editHash": hash_hex(state_hash(&mut edit)),
        },
        "playerExeBytes": exe_bytes,
        "reps": args.reps,
        "nagle": crate::rpc::nagle(),
    });
    let mut ctx = Ctx {
        host,
        edit,
        reps: args.reps,
    };
    for suite in &args.suites {
        let t = Instant::now();
        report[suite.as_str()] = match suite.as_str() {
            "snapshot" => boot::snapshot_suite(&mut ctx)?,
            "boot" => boot::boot_suite(&mut ctx, args.gpu)?,
            "live" => live::live_suite(&mut ctx, args.patches)?,
            "keep" => live::keep_suite(&mut ctx)?,
            "crash" => faults::crash_suite(&mut ctx)?,
            "multi" => faults::multi_suite(&mut ctx)?,
            other => bail!("unknown suite {other}; known: {SUITES:?}"),
        };
        println!("  [{suite} done in {:.1} s]", t.elapsed().as_secs_f64());
    }
    if let Some(dir) = args.out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&args.out, serde_json::to_vec_pretty(&report)?)?;
    println!("report: {}", args.out.display());
    Ok(())
}
