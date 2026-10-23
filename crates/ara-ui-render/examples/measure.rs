mod support;
use ara_ui::{Result, Ui};
use ara_ui_text::TextSystem;
use serde_json::json;
use std::time::Instant;
fn measure(
    name: &str,
    limit: f64,
    mut update: impl FnMut(usize) -> Result<ara_ui::Work>,
) -> Result<serde_json::Value> {
    for i in 0..200 {
        update(i)?;
    }
    let mut samples = Vec::with_capacity(5000);
    let mut last = ara_ui::Work::default();
    for i in 0..5000 {
        let start = Instant::now();
        last = update(i)?;
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let mut sorted = samples.clone();
    sorted.sort_by(f64::total_cmp);
    let max = sorted[4999];
    let data = json!({"workload":name,"samples_ms":samples,"p50_ms":sorted[2500],"p95_ms":sorted[4750],"p99_ms":sorted[4950],"max_ms":max,"budget_ms":limit,"pass":max<=limit,"last_work":last});
    println!(
        "{name:28} p50={:.4} p95={:.4} p99={:.4} max={max:.4} ms",
        sorted[2500], sorted[4750], sorted[4950]
    );
    Ok(data)
}
fn invalidate(ui: &mut Ui) -> Result<()> {
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
fn panel(full: bool, text: &mut TextSystem) -> Result<serde_json::Value> {
    let mut ui = support::inspector()?;
    ui.frame(text)?;
    let mass = ui.find("mass").ok_or("mass")?;
    let reset = ui.find("reset").ok_or("reset")?;
    measure(
        if full {
            "inspector/full invalidate"
        } else {
            "inspector/incremental"
        },
        1.0,
        |i| {
            ui.set_text(mass, if i % 2 == 0 { "4.25" } else { "4.26" })?;
            let mut state = ui.nodes[reset].state;
            state.hover = i % 2 == 0;
            ui.set_state(reset, state);
            if full {
                invalidate(&mut ui)?;
            }
            ui.frame(text)
        },
    )
}
fn tree(full: bool, text: &mut TextSystem) -> Result<serde_json::Value> {
    let (mut ui, mut view, model) = support::tree()?;
    view.scroll(&mut ui, &model, 0.0, 468.0)?;
    ui.frame(text)?;
    measure(
        if full {
            "tree100k/full invalidate"
        } else {
            "tree100k/seven-row scroll"
        },
        2.0,
        |i| {
            view.scroll(
                &mut ui,
                &model,
                ((i * 7) % 99_900) as f32 * 26.0 + 0.5,
                468.0,
            )?;
            if full {
                invalidate(&mut ui)?;
            }
            ui.frame(text)
        },
    )
}
fn widgets(text: &mut TextSystem) -> Result<serde_json::Value> {
    let mut ui = support::inspector()?;
    ui.frame(text)?;
    let toggle = ui.find("enabled").ok_or("toggle")?;
    let slider = ui.find("health").ok_or("slider")?;
    measure("widgets/toggle + slider", 1.0, |i| {
        ui.set_checked(toggle, i % 2 == 0)?;
        ui.set_value(slider, (i % 100) as f64 / 100.0)?;
        ui.frame(text)
    })
}
fn editing(text: &mut TextSystem) -> Result<serde_json::Value> {
    let mut ui = support::inspector()?;
    ui.frame(text)?;
    let field = ui.find("mass").ok_or("field")?;
    let mut clipboard = support::Clip::default();
    measure("text-input/edit + selection", 1.0, |i| {
        ui.edit(field, ara_ui_text::EditEvent::SelectAll, &mut clipboard)?;
        ui.edit(
            field,
            ara_ui_text::EditEvent::Insert(if i % 2 == 0 { "4.25" } else { "4.26" }.into()),
            &mut clipboard,
        )?;
        ui.drain_actions().for_each(drop);
        ui.frame(text)
    })
}
fn table(text: &mut TextSystem) -> Result<serde_json::Value> {
    let (mut ui, mut view, model) = support::table()?;
    view.scroll(&mut ui, &model, 0.0, 260.0)?;
    ui.frame(text)?;
    measure("table10k/seven-row scroll", 2.0, |i| {
        view.scroll(&mut ui, &model, ((i * 7) % 9900) as f32 * 26.0, 260.0)?;
        ui.frame(text)
    })
}
fn game(text: &mut TextSystem, hud: bool) -> Result<serde_json::Value> {
    let mut ui = if hud {
        Ui::editor(
            "<ui id='hud'><text id='health' text='Integrity 86 / 100' /><button id='resume' text='Inventory' /></ui>",
            ara_ui::theme::EDITOR_CSS,
            [600.0, 100.0],
        )?
    } else {
        support::pause()?
    };
    ui.frame(text)?;
    let health = ui.find("health").ok_or("health")?;
    let resume = ui.find("resume").ok_or("resume")?;
    measure(
        if hud {
            "HUD/health + hover"
        } else {
            "pause/health + hover"
        },
        1.0,
        |i| {
            ui.set_text(
                health,
                if i % 2 == 0 {
                    "Integrity 86 / 100"
                } else {
                    "Integrity 85 / 100"
                },
            )?;
            let mut state = ui.nodes[resume].state;
            state.hover = i % 2 == 0;
            ui.set_state(resume, state);
            ui.frame(text)
        },
    )
}
fn main() -> Result<()> {
    let mut text = support::text()?;
    let rows = vec![
        panel(true, &mut text)?,
        panel(false, &mut text)?,
        tree(true, &mut text)?,
        tree(false, &mut text)?,
        widgets(&mut text)?,
        editing(&mut text)?,
        table(&mut text)?,
        game(&mut text, false)?,
        game(&mut text, true)?,
    ];
    let evidence = json!({"method":"200 warmups, 5000 wall-clock samples, update only; excludes CPU rasterization, PNG and inspection; shared machine, no outlier removal","jobs":4,"rust":"1.99.1","machine":"Windows 11 / Ryzen 7 7800X3D / RTX 4070 SUPER (GPU unused)","workloads":rows});
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("evidence/timings.json");
    std::fs::write(out, ara_json::to_canonical_string(&evidence))?;
    Ok(())
}
