//! Character animation controllers authored through the CLI's typed animation
//! commands. Clip records bind the model's animations by name.
use super::Ctx;
use anyhow::{Context, Result};
use ara_anim::{
    document::AnimAsset,
    graph::{
        BlendSample, Comparison, Condition, ControllerData, Node, NodeKind, Parameter,
        ParameterKind, State, Transition,
    },
};
use ara_schema::AssetRef;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

/// Clip timebase of controller durations (ticks per second).
const TIMEBASE: f32 = 48_000.0;

/// A locomotion clip and the ground speed (m/s) it was animated at.
pub struct Gait<'a> {
    /// Animation name in the model.
    pub clip: &'a str,
    /// Ground speed the clip matches.
    pub speed: f32,
}

/// Writes `path` (document `id`) holding one locomotion controller `controller`:
/// `idle` ↔ a 1D speed blend of `gaits`, 0.2 s cross-fades, plus one clip record per
/// animation it uses. `model` is the glTF the clips come from (for cycle lengths).
pub fn locomotion(
    ctx: &Ctx,
    path: &str,
    id: &str,
    controller: &str,
    model: &Path,
    idle: &str,
    gaits: &[Gait<'_>],
) -> Result<()> {
    let lengths = clip_lengths(model)?;
    let record = |name: &str| format!("an_{}", name.to_ascii_lowercase().replace(['_', ' '], ""));
    let reference = |name: &str| AssetRef(format!("@{path}#{}", record(name)));
    let clip = |name: &str| Node {
        kind: NodeKind::Clip,
        clip: Some(reference(name)),
        ..Default::default()
    };
    let speed = |comparison, value: f32| Condition {
        parameter: "speed".into(),
        comparison,
        float_value: value,
        ..Default::default()
    };
    let mut nodes = BTreeMap::from([("idle".to_owned(), clip(idle))]);
    let mut samples = Vec::new();
    let mut cycle = 0.0;
    for gait in gaits {
        let key = gait.clip.to_ascii_lowercase();
        nodes.insert(key.clone(), clip(gait.clip));
        samples.push(BlendSample {
            node: key,
            x: gait.speed,
            y: 0.0,
        });
        cycle += lengths
            .get(gait.clip)
            .with_context(|| format!("{} has no animation {}", model.display(), gait.clip))?;
    }
    let cycle = cycle / gaits.len().max(1) as f32;
    nodes.insert(
        "move".into(),
        Node {
            kind: NodeKind::Blend1d,
            parameter_x: "speed".into(),
            samples,
            ..Default::default()
        },
    );
    let data = ControllerData {
        parameters: BTreeMap::from([(
            "speed".into(),
            Parameter {
                kind: ParameterKind::Float,
                ..Default::default()
            },
        )]),
        initial: "idle".into(),
        fixed_step: 800,
        extract_root_motion: false,
        nodes,
        states: BTreeMap::from([
            (
                "idle".into(),
                State {
                    node: "idle".into(),
                    ..Default::default()
                },
            ),
            (
                "move".into(),
                State {
                    node: "move".into(),
                    duration: (cycle * TIMEBASE).round() as u32,
                    ..Default::default()
                },
            ),
        ]),
        transitions: vec![
            Transition {
                id: "start".into(),
                from: "idle".into(),
                to: "move".into(),
                duration: 9600,
                conditions: vec![speed(Comparison::Greater, 0.15)],
                ..Default::default()
            },
            Transition {
                id: "stop".into(),
                from: "move".into(),
                to: "idle".into(),
                duration: 9600,
                conditions: vec![speed(Comparison::Less, 0.075)],
                ..Default::default()
            },
        ],
    };
    let mut records = vec![(
        controller.to_owned(),
        AnimAsset {
            name: "Locomotion".into(),
            controller: Some(data),
            ..Default::default()
        },
    )];
    for name in std::iter::once(idle).chain(gaits.iter().map(|g| g.clip)) {
        records.push((
            record(name),
            AnimAsset {
                name: name.into(),
                ..Default::default()
            },
        ));
    }
    ctx.new_document(path, "animation", id)?;
    let calls = records
        .into_iter()
        .map(|(id, asset)| {
            ara_json::to_value(&asset)
                .map(|asset| ("animation.create", json!({"id": id, "asset": asset})))
                .map_err(anyhow::Error::msg)
        })
        .collect::<Result<Vec<_>>>()?;
    ctx.apply(
        path,
        "Author locomotion controller and clip bindings",
        calls,
    )
}

/// Every animation's length in seconds (largest sampler input time), by name.
pub fn clip_lengths(model: &Path) -> Result<BTreeMap<String, f32>> {
    let bytes = std::fs::read(model)?;
    let doc: Value = if bytes.starts_with(b"glTF") {
        let len = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as usize;
        serde_json::from_slice(&bytes[20..20 + len])?
    } else {
        serde_json::from_slice(&bytes)?
    };
    let mut out = BTreeMap::new();
    for animation in doc["animations"].as_array().into_iter().flatten() {
        let name = animation["name"].as_str().unwrap_or_default().to_owned();
        let length = animation["samplers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| doc["accessors"][s["input"].as_u64()? as usize]["max"][0].as_f64())
            .fold(0.0f64, f64::max);
        out.insert(name, length as f32);
    }
    Ok(out)
}
