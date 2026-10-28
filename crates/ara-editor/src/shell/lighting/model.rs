//! Read only the resolved engine components; camera influence uses the renderer's own blend math.
use super::*;
use ara_components::render::SceneLook;
use ara_doc::prefab::Resolved;

pub(super) fn component(name: &str) -> bool {
    matches!(
        name,
        "Light"
            | "Environment"
            | "SkyAtmosphere"
            | "HeightFog"
            | "PostProcessVolume"
            | "ReflectionProbe"
    )
}
pub(super) fn lighting(record: &ara_doc::Record) -> bool {
    record.keys().any(|name| component(name))
}

pub(super) struct Row {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon: &'static str,
    pub priority: Option<f32>,
}

pub(super) fn pose(matrix: glam::Mat4) -> Option<(Vec3, glam::Quat, Vec3)> {
    let determinant = matrix.determinant();
    if !matrix.is_finite() || !determinant.is_finite() || determinant.abs() < 1e-12 {
        return None;
    }
    if [matrix.x_axis, matrix.y_axis, matrix.z_axis]
        .iter()
        .any(|axis| !axis.truncate().length_squared().is_finite())
    {
        return None;
    }
    let (scale, rotation, translation) = matrix.to_scale_rotation_translation();
    let length = rotation.length_squared();
    if !scale.is_finite()
        || !rotation.is_finite()
        || !translation.is_finite()
        || !length.is_finite()
        || length < 1e-12
    {
        return None;
    }
    Some((scale, rotation.normalize(), translation))
}

pub(super) fn volume(entities: &Resolved, id: &str) -> Option<PostVolume> {
    let row = entities.get(id)?;
    let volume: PostProcessVolume =
        serde::Deserialize::deserialize(row.record.get("PostProcessVolume")?).ok()?;
    let (scale, rotation, translation) = if volume.shape == VolumeShape::Global {
        (Vec3::ONE, glam::Quat::IDENTITY, Vec3::ZERO)
    } else {
        pose(crate::world::world_transform(entities, id)?)?
    };
    if !scale.is_finite() || !rotation.is_finite() || !translation.is_finite() {
        return None;
    }
    Some(PostVolume {
        volume,
        translation: translation.to_array(),
        rotation: rotation.to_array(),
        scale: scale.to_array(),
    })
}

pub(super) fn rows(entities: &Resolved, editor: &Editor, query: &str) -> Vec<Row> {
    let fog = entities
        .iter()
        .find(|(id, row)| {
            !editor.state.hidden.contains(*id) && row.record.contains_key("HeightFog")
        })
        .map(|(id, _)| id.as_str());
    let mut result = Vec::new();
    for (id, row) in entities.iter().filter(|(_, row)| lighting(&row.record)) {
        let name = row.record.get("name").and_then(Value::as_str).unwrap_or(id);
        let kinds: Vec<_> = row
            .record
            .keys()
            .filter(|name| component(name))
            .map(String::as_str)
            .collect();
        let haystack = format!("{id} {name} {}", kinds.join(" ")).to_lowercase();
        if !query.is_empty() && !haystack.contains(query) {
            continue;
        }
        let hidden = editor.state.hidden.contains(id);
        let (mut description, priority, icon) = if row.record.contains_key("PostProcessVolume") {
            match volume(entities, id) {
                Some(volume) => {
                    let shape = match volume.volume.shape {
                        VolumeShape::Global => "Global",
                        VolumeShape::Box => "Local box",
                        VolumeShape::Sphere => "Local sphere",
                    };
                    let influence = if hidden {
                        0.
                    } else {
                        volume.weight_at(editor.state.camera.position().to_array())
                    };
                    (
                        format!(
                            "{shape} · priority {:.1} · weight {:.2} · Scene camera {:.0}% · {} override groups",
                            volume.volume.priority,
                            volume.volume.weight,
                            influence * 100.,
                            volume.volume.overrides.len()
                        ),
                        Some(volume.volume.priority),
                        "sliders-horizontal",
                    )
                }
                None => (
                    "Post-process volume · invalid or degenerate transform; inspect its values"
                        .into(),
                    Some(0.),
                    "sliders-horizontal",
                ),
            }
        } else if let Some(value) = row.record.get("ReflectionProbe") {
            match serde::Deserialize::deserialize(value) {
                Ok(probe) => {
                    let probe: ara_components::render::ReflectionProbe = probe;
                    let detail=probe.invalid().map_or_else(||format!(
                        "Reflection probe · authored bounds {:.1} × {:.1} × {:.1} m · priority {} · blend {:.1} m · {} px per face{}",
                        probe.bounds[0],probe.bounds[1],probe.bounds[2],probe.priority,probe.blend_distance,probe.capture_resolution,
                        if probe.enabled {""}else{" · disabled contribution"}),|reason|format!("Invalid Reflection Probe · {reason}"));
                    (detail, Some(probe.priority as f32), "box")
                }
                Err(_) => (
                    "Reflection Probe values could not be read; inspect its fields".into(),
                    None,
                    "box",
                ),
            }
        } else if row.record.contains_key("HeightFog") {
            (
                if hidden {
                    "Global height fog"
                } else if fog == Some(id.as_str()) {
                    "Global height fog · effective first entity in ID order"
                } else {
                    "Global height fog · a lower-ID visible fog takes precedence"
                }
                .into(),
                None,
                "sun",
            )
        } else if let Some(light) = row.record.get("Light") {
            (
                format!(
                    "{} light · intensity {}",
                    light.get("kind").and_then(Value::as_str).unwrap_or("point"),
                    light
                        .get("intensity")
                        .map_or("schema default".into(), panels::display)
                ),
                None,
                "lightbulb",
            )
        } else if row.record.contains_key("SkyAtmosphere") {
            ("Physical sky atmosphere".into(), None, "sun")
        } else {
            ("Scene environment".into(), None, "sun")
        };
        if hidden {
            description.push_str(" · hidden in Scene viewport");
        }
        result.push(Row {
            id: id.clone(),
            name: name.into(),
            description,
            icon,
            priority,
        });
    }
    result.sort_by(|a, b| match (a.priority, b.priority) {
        (Some(a_priority), Some(b_priority)) => {
            a_priority.total_cmp(&b_priority).then(a.id.cmp(&b.id))
        }
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.id.cmp(&b.id),
    });
    result
}

pub(super) fn blended(
    entities: &Resolved,
    editor: &Editor,
) -> Option<ara_components::post::PostValues> {
    let volumes = entities
        .keys()
        .filter(|id| !editor.state.hidden.contains(*id))
        .filter_map(|id| volume(entities, id))
        .collect();
    SceneLook {
        volumes,
        ..Default::default()
    }
    .post_at(editor.state.camera.position().to_array())
}
