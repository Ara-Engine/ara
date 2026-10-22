//! S3 executable: captures, semantic checks and a release-mode CPU benchmark; no window/server.
mod display;
mod input;
mod runtime;
mod screens;
mod source;
mod style;
mod text;
mod validation;

use crate::{
    runtime::{Ui, Work},
    source::Result,
    text::TextSystem,
};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Serialize)]
struct Timing {
    name: String,
    samples: usize,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    max_ms: f64,
    mean_ms: f64,
    last_work: Work,
    frame_ms: Vec<f64>,
}
fn measure(
    name: &str,
    samples: usize,
    mut tick: impl FnMut(usize) -> Result<Work>,
) -> Result<Timing> {
    for frame in 0..200 {
        std::hint::black_box(tick(frame)?);
    }
    let mut frame_ms = Vec::with_capacity(samples);
    let mut work = Work::default();
    for frame in 0..samples {
        let start = Instant::now();
        work = tick(frame + 200)?;
        frame_ms.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let mut sorted = frame_ms.to_vec();
    sorted.sort_by(f64::total_cmp);
    Ok(Timing {
        name: name.into(),
        samples,
        p50_ms: sorted[samples / 2],
        p95_ms: sorted[(samples * 95 / 100).min(samples - 1)],
        p99_ms: sorted[(samples * 99 / 100).min(samples - 1)],
        max_ms: sorted[samples - 1],
        mean_ms: sorted.iter().sum::<f64>() / samples as f64,
        last_work: work,
        frame_ms,
    })
}
fn invalidate_all(ui: &mut Ui) -> Result<()> {
    for n in &mut ui.nodes {
        n.dirty.style = true;
        n.dirty.layout = true;
        n.dirty.paint = true;
        if let Some(t) = &mut n.text {
            t.dirty = true;
        }
        ui.tree.mark_dirty(n.layout_id)?;
    }
    Ok(())
}
fn benchmark_screen(
    name: &str,
    ui: &mut Ui,
    text: &mut TextSystem,
    hover: &str,
    samples: usize,
    full: bool,
) -> Result<Timing> {
    let hover = ui.index(hover);
    measure(name, samples, |frame| {
        ui.model_changed(&runtime::HudModel {
            health: 80 + (frame % 20) as u32,
        })?;
        ui.set_state(
            hover,
            style::State {
                hover: frame % 2 == 0,
                ..Default::default()
            },
        );
        if full {
            invalidate_all(ui)?;
        }
        let work = ui.frame(text)?;
        std::hint::black_box(ui.display.commands.len());
        Ok(work)
    })
}
fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    std::fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}
fn capture(
    name: &str,
    ui: &mut Ui,
    text: &mut TextSystem,
    out: &Path,
) -> Result<serde_json::Value> {
    let start = Instant::now();
    let work = ui.frame(text)?;
    let cold_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    ui.display.render(
        &text.assets,
        ui.size[0] as u32,
        ui.size[1] as u32,
        &out.join(format!("{name}.png")),
    )?;
    let raster_ms = start.elapsed().as_secs_f64() * 1000.0;
    let inspection = ui.inspect();
    write_json(&out.join(format!("{name}.json")), &inspection)?;
    write_json(&out.join(format!("{name}-display.json")), &ui.display)?;
    Ok(
        serde_json::json!({"screen":name,"nodes":ui.nodes.len(),"display_commands":ui.display.commands.len(),"batch_breaks":ui.display.breaks().len(),"cold_update_ms":cold_ms,"cpu_reference_raster_and_png_ms":raster_ms,"work":work}),
    )
}
fn input_checks(pause: &mut Ui, text: &mut TextSystem) -> Result<serde_json::Value> {
    use input::{Composition, Direction, FocusStack, ImeEvent};
    let resume = pause.index("resume");
    let settings = pause.index("settings");
    let exit = pause.index("exit");
    if input::next_tab(pause, resume, false) != Some(settings)
        || input::next_tab(pause, settings, false) != Some(exit)
        || input::next_tab(pause, exit, true) != Some(settings)
        || input::directional(pause, resume, Direction::Down) != Some(settings)
        || input::directional(pause, settings, Direction::Up) != Some(resume)
    {
        return Err("focus navigation failed".into());
    }
    let r = pause.nodes[settings].rect;
    let hit = pause.hit_test(r.x + r.w / 2.0, r.y + r.h / 2.0);
    if hit != Some(settings) {
        return Err("pointer hit test failed".into());
    }
    let mut stack = FocusStack::default();
    stack.push(1, "resume");
    stack.focus("settings");
    stack.push(2, "volume");
    let restored = stack.pop().ok_or("focus restoration failed")?.to_owned();
    if restored != "settings" {
        return Err("focus restoration failed".into());
    }
    pause.set_state(
        settings,
        style::State {
            focus: true,
            ..Default::default()
        },
    );
    let paint_only = pause.frame(text)?;
    if paint_only.layout_passes != 0 || paint_only.shaped != 0 {
        return Err("focus color change unexpectedly relaid out".into());
    }
    let mut ime = Composition::default();
    ime.event(ImeEvent::Start)?;
    ime.event(ImeEvent::Update {
        preedit: "にほんご",
        selection: 0..12,
    })?;
    let before = serde_json::to_value(&ime)?;
    ime.event(ImeEvent::Commit("日本語"))?;
    let committed = serde_json::to_value(&ime)?;
    ime.event(ImeEvent::Start)?;
    ime.event(ImeEvent::Update {
        preedit: "取消",
        selection: 0..6,
    })?;
    ime.event(ImeEvent::Cancel)?;
    let world_hit = input::ray_to_local(
        [0.0, 0.0, 2.0],
        [0.0, 0.0, -1.0],
        [0.0; 3],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [2.0, 1.5],
        [640.0, 480.0],
    );
    Ok(
        serde_json::json!({"tab_and_shift_tab":true,"directional_gamepad":true,"disabled_skipped":true,"pointer_hit":"settings","focus_restored":restored,"paint_only_work":paint_only,"ime_preedit":before,"ime_committed":committed,"ime_cancelled":ime,"world_center_ray":world_hit}),
    )
}
fn roundtrip(out: &Path) -> Result<serde_json::Value> {
    let original = include_str!("../assets/pause.ui");
    let mut doc = source::SourceDocument::parse(original)?;
    let no_op = doc.serialize().as_bytes() == original.as_bytes();
    let op = doc.command(source::SetAttribute {
        id: "resume",
        attribute: "text",
        value: "Continue expedition",
        revision: 0,
    })?;
    doc.apply(&op)?;
    let exact = doc.serialize() == original.replacen("Resume expedition", "Continue expedition", 1);
    if !no_op || !exact {
        return Err("lossless round-trip failed".into());
    }
    std::fs::write(out.join("pause-edited.ui"), doc.serialize())?;
    doc.apply(&op.inverse(doc.revision()))?;
    let undo_exact = doc.serialize().as_bytes() == original.as_bytes();
    Ok(
        serde_json::json!({"noop_byte_identical":no_op,"one_attribute_only":exact,"undo_byte_identical":undo_exact,"operation":op}),
    )
}
fn main() -> Result<()> {
    let mut out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("evidence");
    let mut samples = 2000;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = PathBuf::from(args.next().ok_or("--out requires a path")?),
            "--samples" => {
                samples = args
                    .next()
                    .ok_or("--samples requires an integer")?
                    .parse()?
            }
            _ => return Err(format!("unknown argument {arg}").into()),
        }
    }
    if samples < 100 {
        return Err("use at least 100 timing samples".into());
    }
    std::fs::create_dir_all(&out)?;
    let mut text = TextSystem::new()?;
    let mut pause = Ui::parse(
        include_str!("../assets/pause.ui"),
        screens::CSS,
        [600.0, 760.0],
    )?;
    let mut hud = Ui::parse(
        include_str!("../assets/hud.ui"),
        screens::CSS,
        [600.0, 360.0],
    )?;
    let mut inspector = Ui::parse(&screens::inspector(), screens::CSS, [600.0, 760.0])?;
    let mut world = Ui::parse(
        include_str!("../assets/world.ui"),
        screens::CSS,
        [640.0, 480.0],
    )?;
    let mut inventory = screens::Inventory::new()?;
    inventory.scroll_to(4310)?;
    let mut lab = Ui::parse(
        include_str!("../assets/text.ui"),
        screens::CSS,
        [1000.0, 1100.0],
    )?;
    let mut ime = input::Composition::default();
    ime.event(input::ImeEvent::Start)?;
    ime.event(input::ImeEvent::Update {
        preedit: "にほんご",
        selection: 0..12,
    })?;
    lab.set_text(lab.index("ime-field"), &ime.preedit)?;
    let mut captures = Vec::new();
    for (name, ui) in [
        ("pause", &mut pause),
        ("hud", &mut hud),
        ("inspector", &mut inspector),
        ("world", &mut world),
        ("inventory", &mut inventory.ui),
        ("text", &mut lab),
    ] {
        captures.push(capture(name, ui, &mut text, &out)?);
    }
    let a11y = screens::accessibility(&inspector, "position");
    write_json(&out.join("inspector-accesskit.json"), &a11y)?;
    let input = input_checks(&mut pause, &mut text)?;
    write_json(&out.join("input.json"), &input)?;
    let roundtrip = roundtrip(&out)?;
    write_json(&out.join("roundtrip.json"), &roundtrip)?;
    let text_checks = validation::text_coverage(&lab)?;
    write_json(&out.join("text-coverage.json"), &text_checks)?;
    let probe = validation::clipping_probe(&mut text, &out)?;
    write_json(&out.join("clip-transform.json"), &probe)?;
    write_json(&out.join("diagnostics.json"), &validation::diagnostics()?)?;
    let mut timings = vec![
        benchmark_screen(
            "pause_full_invalidation_baseline",
            &mut pause,
            &mut text,
            "resume",
            samples,
            true,
        )?,
        benchmark_screen(
            "pause_incremental",
            &mut pause,
            &mut text,
            "resume",
            samples,
            false,
        )?,
        benchmark_screen(
            "hud_incremental",
            &mut hud,
            &mut text,
            "hud-inventory",
            samples,
            false,
        )?,
    ];
    for full in [true, false] {
        timings.push(measure(
            if full {
                "inventory_full_invalidation_baseline"
            } else {
                "inventory_scroll_incremental"
            },
            samples,
            |frame| {
                inventory.scroll_to((frame * 7) % (10_000 - screens::VISIBLE_ROWS))?;
                if full {
                    invalidate_all(&mut inventory.ui)?;
                }
                let work = inventory.ui.frame(&mut text)?;
                std::hint::black_box(inventory.ui.display.commands.len());
                Ok(work)
            },
        )?);
    }
    for t in &timings {
        println!(
            "{}: p50={:.4} ms p95={:.4} ms p99={:.4} ms max={:.4} ms",
            t.name, t.p50_ms, t.p95_ms, t.p99_ms, t.max_ms
        );
    }
    write_json(
        &out.join("measurements.json"),
        &serde_json::json!({"machine":{"cpu":"Ryzen 7 7800X3D","ram_gb":31,"gpu":"RTX 4070 SUPER","os":"Windows 11","toolchain":"Rust 1.96.0","cargo_build_jobs":4},"profile":if cfg!(debug_assertions) {"debug"} else {"release"},"warmup_frames":200,"measurement":"mutation + restyle + Taffy layout + Parley text + display list; excludes raster/PNG and JSON inspection","captures":captures,"timings":timings,"roundtrip":roundtrip,"accesskit_nodes":a11y.nodes.len(),"inventory_model_items":inventory.items.len(),"inventory_visible_rows":screens::VISIBLE_ROWS,"inventory_retained_nodes":inventory.ui.nodes.len()}),
    )?;
    println!("Evidence: {}", out.display());
    Ok(())
}
