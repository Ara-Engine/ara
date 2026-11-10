//! Bounds use the exact retained native geometry and instance transform.
use super::*;
use ara_runtime::extract::Geometry;
use std::collections::BTreeMap;

pub(super) fn collect(
    wires: &mut Vec<Segment>,
    counts: &mut Counts,
    view: &DocumentView,
    source: Source<'_>,
) {
    let Some(scene) = source.scene.filter(|_| source.current) else {
        counts.unavailable_bounds = view
            .entities()
            .values()
            .filter(|entity| entity.record.contains_key("MeshRenderer"))
            .count();
        return;
    };
    let draws = source.frame.map(|frame| {
        frame
            .draws
            .iter()
            .map(|draw| (draw.entity, draw))
            .collect::<BTreeMap<_, _>>()
    });
    let imports = view.entities().values().any(|entity| {
        entity
            .record
            .get("MeshRenderer")
            .and_then(|mesh| mesh.get("model"))
            .is_some_and(|model| !model.is_null())
    });
    for instance in &scene.instances {
        if draws.is_none() && imports {
            counts.unavailable_bounds += 1;
            continue;
        }
        if let Some(draw) = draws.as_ref().and_then(|draws| draws.get(&instance.entity)) {
            let imported = view
                .entities()
                .get(&draw.address)
                .and_then(|entity| entity.record.get("MeshRenderer"))
                .and_then(|mesh| mesh.get("model"))
                .is_some_and(|model| !model.is_null());
            match &draw.geometry {
                Geometry::Shape { .. } if imported => {
                    counts.pending_models += 1;
                    continue;
                }
                Geometry::Model { model, primitive } if *primitive >= model.primitives.len() => {
                    counts.invalid_bounds += 1;
                    continue;
                }
                _ => {}
            }
        }
        let Some(geometry) = scene.geometry.get(instance.geometry) else {
            counts.invalid_bounds += 1;
            continue;
        };
        let bounds = instance.bounds(geometry);
        if !bounds.iter().all(|point| point.is_finite()) || !bounds[0].cmple(bounds[1]).all() {
            counts.invalid_bounds += 1;
            continue;
        }
        box_lines(
            wires,
            bounds[0],
            bounds[1],
            glam::Mat4::IDENTITY,
            Tint::Bounds,
        );
        counts.mesh_bounds += 1;
    }
}
pub(super) fn box_lines(
    wires: &mut Vec<Segment>,
    min: Vec3,
    max: Vec3,
    world: glam::Mat4,
    tint: Tint,
) {
    let corners: [Vec3; 8] = std::array::from_fn(|index| {
        world.transform_point3(Vec3::new(
            if index & 1 == 0 { min.x } else { max.x },
            if index & 2 == 0 { min.y } else { max.y },
            if index & 4 == 0 { min.z } else { max.z },
        ))
    });
    for index in 0..8 {
        for bit in [1, 2, 4] {
            if index & bit == 0 {
                wires.push(Segment {
                    from: corners[index],
                    to: corners[index | bit],
                    tint,
                });
            }
        }
    }
}
