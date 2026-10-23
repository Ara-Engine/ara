//! Release benchmark: DSP cost per graph voice for each example graph (ns per 128-frame
//! block and per voice), with 1 and 100 concurrent voices. Writes `evidence/cost.json`.
//!
//! `cargo run -p ara-audio-graph --release --example graph_cost`
use ara_audio::{Clip, Frame, QUANTUM, SignalSource};
use ara_audio_graph::load;
use std::time::Instant;

fn clips(reference: &str) -> ara_audio::Result<Clip> {
    let seconds = if reference.contains("loop") {
        1.0
    } else {
        0.25
    };
    let frames = (0..(48000.0 * seconds) as usize)
        .map(|i| {
            let t = i as f32 / 48000.0;
            let s = (t * 220.0 * std::f32::consts::TAU).cos() * (-t * 4.0).exp() * 0.5;
            Frame::new(s, s)
        })
        .collect();
    Clip::new(48000, frames, 1 << 24)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut rows = Vec::new();
    for name in ["footsteps", "wind", "engine", "ui_click", "rifle"] {
        let path = format!("{}/graphs/{name}.audiograph", env!("CARGO_MANIFEST_DIR"));
        let program = load(
            &std::fs::read_to_string(path)?,
            "gr_bench",
            48000,
            &mut clips,
        )
        .map_err(|d| format!("{name}: {d:?}"))?;
        let mut row = serde_json::json!({
            "graph": name,
            "nodes": program.len(),
            "arenaBytes": program.arena_bytes(),
            "estimatedNsPerBlock": program.estimated_cost(),
        });
        for voices in [1_usize, 100] {
            let mut instances: Vec<_> =
                (0..voices).map(|i| program.instantiate(i as u64)).collect();
            let mut block = [Frame::ZERO; QUANTUM];
            let blocks = 2000_u64;
            let mut rendered = 0_u64;
            let start = Instant::now();
            for b in 0..blocks {
                let frame = b * QUANTUM as u64;
                for (voice, handle) in &mut instances {
                    if b % 40 == 0 {
                        // Triggered one-shots get retriggered so they keep doing work.
                        let _ = handle.trigger("step", frame);
                        let _ = handle.trigger("fire", frame);
                    }
                    // Self-ending one-shots (UI click) only count blocks they actually render.
                    if !voice.ended() {
                        voice.render(&mut block, frame);
                        rendered += 1;
                    }
                }
            }
            let ns = start.elapsed().as_nanos() as f64 / rendered.max(1) as f64;
            if let Some(o) = row.as_object_mut() {
                o.insert(format!("nsPerBlockPerVoice{voices}"), ns.round().into());
            }
        }
        rows.push(row);
    }
    let value = serde_json::json!({
        "graphs": rows,
        "quantumFrames": QUANTUM,
        "sampleRate": 48000,
        "machine": "Windows 11 dev box, shared with ~8 concurrent agent builds; release profile",
        "note": "Per-voice DSP cost excludes the engine's resampling/panning (see ara-acoustics evidence).",
    });
    let json = ara_json::to_canonical_string(&ara_json::sort_keys(&value));
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("evidence");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("cost.json"), &json)?;
    println!("{json}");
    Ok(())
}
