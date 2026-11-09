//! GPU cost of shadows per preset: a 2560x1440 view over a floor with forty boxes and
//! spheres, lit by the sun (its real 0.53 degree disc) and four shadowed area lights
//! (rect, disc, sphere, tube) in volumetric height fog. Presents 60 warm frames per
//! preset (Low to Epic, each preset's own render scale) and prints, per preset, the
//! p50 of the frame's GPU passes and of the shadow-related ones as JSON. Wrap timing
//! runs: `cargo run -p ara-dev -- gpu -- cargo run --release -p ara-runtime-render
//! --example shadow_costs` (`-- hard`: every light shadowed as a point; `-- sun`: the sun alone, no area lights).
use anyhow::Result;
use ara_doc::Workspace;
use ara_runtime::{Runtime, RuntimeConfig, assets::MemoryAssets, extract::RenderFrame};
use ara_runtime_render::{Presenter, QualityChoice, QualityPreset};
use std::{collections::BTreeMap, fmt::Write};

const SIZE: [u32; 2] = [2560, 1440];

/// The four shadowed area lights (records after the floor).
const AREAS: &str = r#",
    "e_rect": {"name": "Rect", "order": "a4",
      "Light": {"kind": "rect", "intensity": 8000, "range": 12, "sourceWidth": 1.6, "sourceHeight": 0.8, "barnDoorLength": 0},
      "Transform": {"position": [-4, 3.5, -2], "rotation": [-90, 0, 0]}},
    "e_disc": {"name": "Disc", "order": "a5",
      "Light": {"kind": "disc", "intensity": 8000, "range": 12, "sourceRadius": 0.5},
      "Transform": {"position": [4, 3.5, -2], "rotation": [-90, 0, 0]}},
    "e_sphere": {"name": "Sphere", "order": "a6",
      "Light": {"kind": "point", "intensity": 8000, "range": 12, "sourceRadius": 0.4},
      "Transform": {"position": [-4, 3, 3]}},
    "e_tube": {"name": "Tube", "order": "a7",
      "Light": {"kind": "point", "intensity": 8000, "range": 12, "sourceRadius": 0.05, "sourceLength": 1.2},
      "Transform": {"position": [4, 3, 3]}}"#;

/// The scene: deterministic rows of boxes and spheres, the lights over them (the area
/// lights only with `lights`).
fn scene(lights: bool) -> String {
    let mut records = String::new();
    for i in 0..40 {
        let (x, z) = ((i % 8) as f32 * 2.2 - 7.7, (i / 8) as f32 * 2.2 - 6.);
        let (shape, size, y) = if i % 2 == 0 {
            (
                "box",
                format!("[0.8, {}, 0.8]", 0.6 + (i % 5) as f32 * 0.5),
                0.3 + (i % 5) as f32 * 0.25,
            )
        } else {
            ("sphere", "[0.9, 0.9, 0.9]".into(), 0.45)
        };
        write!(
            records,
            r#",
    "e_{i}": {{"name": "Prop {i}", "order": "b{i:02}",
      "MeshRenderer": {{"shape": "{shape}", "size": {size}, "color": [0.6, 0.6, 0.6, 1], "roughness": 0.7}},
      "Transform": {{"position": [{x}, {y}, {z}]}}}}"#
        )
        .expect("writing to a string");
    }
    let areas = if lights { AREAS } else { "" };
    format!(
        r#"{{
  "$schema": "ara://schema/scene/1",
  "id": "sc_shadowcosts",
  "records": {{
    "e_camera": {{"name": "Camera", "order": "a0", "Camera": {{"fov": 55}},
      "Transform": {{"position": [0, 5, 12], "rotation": [-22, 0, 0]}}}},
    "e_sun": {{"name": "Sun", "order": "a1", "Light": {{"kind": "directional", "intensity": 60000}},
      "Transform": {{"rotation": [-40, 30, 0]}}}},
    "e_fog": {{"name": "Fog", "order": "a2",
      "HeightFog": {{"density": 0.02, "baseHeight": 0, "falloff": 6, "volumetric": true, "volumetricDistance": 40}}}},
    "e_floor": {{"name": "Floor", "order": "a3",
      "MeshRenderer": {{"shape": "plane", "size": [60, 1, 60], "color": [0.5, 0.5, 0.5, 1], "roughness": 0.8}}}}{areas}{records}
  }}
}}"#
    )
}

fn runtime() -> Result<Runtime> {
    let schema = ara_runtime::schema(&[])?;
    let doc = ara_doc::read(
        &scene(!std::env::args().any(|a| a == "sun")),
        "levels/costs.scene",
        &schema,
        None,
    )
    .doc
    .ok_or_else(|| anyhow::anyhow!("the cost scene did not parse"))?;
    let id = doc.id.clone();
    let mut workspace = Workspace::default();
    workspace.insert("levels/costs.scene", doc);
    Runtime::new(
        schema,
        workspace,
        id,
        Box::new(MemoryAssets::default()),
        RuntimeConfig::default(),
        Vec::new(),
    )
    .map_err(|e| anyhow::anyhow!("{e:?}"))
}

/// Whether a pass belongs to shadowing: map draws, pyramids, virtual pages and their
/// projection.
fn shadow_pass(name: &str) -> bool {
    ["Shadow", "VSM", "Horizon", "Translucent"]
        .iter()
        .any(|p| name.starts_with(p))
}

fn main() -> Result<()> {
    let mut rt = runtime()?;
    let mut frame = RenderFrame::default();
    rt.extract(&mut frame);
    let mut report = BTreeMap::new();
    for preset in [
        QualityPreset::Low,
        QualityPreset::Medium,
        QualityPreset::High,
        QualityPreset::Epic,
    ] {
        let mut presenter = Presenter::new(SIZE, Vec::new(), "target/shadow-costs/gpu");
        presenter.set_quality(QualityChoice::Preset(preset));
        if std::env::args().any(|a| a == "hard") {
            presenter.set_debug(ara_render::RenderDebug {
                soft_shadows: false,
                ..ara_render::RenderDebug::default()
            });
        }
        let mut passes = BTreeMap::<String, Vec<f64>>::new();
        let mut totals = Vec::new();
        for i in 0..90 {
            presenter.present(&frame)?;
            if i < 30 {
                continue;
            }
            let mut total = 0.;
            for timing in presenter.pass_timings() {
                total += timing.milliseconds;
                passes
                    .entry(timing.pass.clone())
                    .or_default()
                    .push(timing.milliseconds);
            }
            totals.push(total);
        }
        let p50 = |mut v: Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v.get(v.len() / 2).copied().unwrap_or(0.)
        };
        let named: BTreeMap<String, f64> = passes
            .into_iter()
            .map(|(name, times)| (name, p50(times)))
            .collect();
        let shadow: f64 = named
            .iter()
            .filter(|(name, _)| shadow_pass(name))
            .map(|(_, ms)| ms)
            .sum();
        report.insert(
            format!("{preset:?}"),
            serde_json::json!({
                "frame_ms": p50(totals),
                "shadow_passes_ms": shadow,
                "passes": named,
            }),
        );
    }
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
