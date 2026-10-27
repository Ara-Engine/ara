//! A shared Play process gives each slot an independently owned player loop.
//! Embedded/headless slots create no SDL windows; their runtime, link, input,
//! navigation store, scripts and GPU surfaces are constructed on their own thread.
use crate::{Args, run};
use anyhow::{Context, Result, bail};
use clap::Parser;
use std::{
    thread,
    time::{Duration, Instant},
};

pub(crate) fn run_group(args: Args, launched: Instant, count: usize) -> Result<()> {
    if args.owner.is_none() || args.window || !(1..=17).contains(&count) {
        bail!(
            "Shared Play requires an owner, 1–17 slots, and embedded/headless views; SDL windows use separate processes"
        );
    }
    let mut workers = Vec::with_capacity(count);
    for slot in 0..count {
        let mut player = Args::try_parse_from(std::env::args_os())?;
        player.group_slots = None;
        player.name = format!("{}-{slot}", args.name);
        let worker = thread::Builder::new()
            .name(format!("ara-play-world-{slot}"))
            .spawn(move || run(player, launched))
            .context("Cannot start isolated shared-process player thread")?;
        workers.push((slot, worker));
    }
    while !workers.is_empty() {
        if let Some(index) = workers.iter().position(|(_, worker)| worker.is_finished()) {
            let (slot, worker) = workers.swap_remove(index);
            match worker.join() {
                Ok(result) => result.with_context(|| format!("Shared Play world {slot} failed"))?,
                Err(_) => bail!("Shared Play world {slot} panicked"),
            }
        } else {
            thread::sleep(Duration::from_millis(2));
        }
    }
    Ok(())
}
