//! Source bounds and primitive slots follow the runtime's default-scene traversal
//! and selected-node pivot, without cooking an extra derived model.
use anyhow::{Context, Result, ensure};
use glam::{Mat4, Quat, Vec3};
use serde_json::Value;
use std::path::Path;

pub(super) fn read(path: &Path) -> Result<Value> {
    let bytes = std::fs::read(path)?;
    if bytes.starts_with(b"glTF") {
        ensure!(bytes.len() >= 20, "short GLB");
        let length = u32::from_le_bytes(bytes[12..16].try_into()?) as usize;
        Ok(serde_json::from_slice(
            bytes.get(20..20 + length).context("short GLB JSON")?,
        )?)
    } else {
        Ok(serde_json::from_slice(&bytes)?)
    }
}

pub(super) fn file(path: &Path, node: Option<&str>) -> Result<(Vec3, Vec3)> {
    bounds(&read(path)?, node)
}

fn local(node: &Value) -> Mat4 {
    if let Some(matrix) = node["matrix"]
        .as_array()
        .filter(|values| values.len() == 16)
    {
        let columns: [f32; 16] = std::array::from_fn(|i| matrix[i].as_f64().unwrap_or(0.) as f32);
        return Mat4::from_cols_array(&columns);
    }
    let values = |name: &str, defaults: &[f32]| -> Vec<f32> {
        (0..defaults.len())
            .map(|i| node[name][i].as_f64().map_or(defaults[i], |v| v as f32))
            .collect()
    };
    let t = values("translation", &[0.; 3]);
    let r = values("rotation", &[0., 0., 0., 1.]);
    let s = values("scale", &[1.; 3]);
    Mat4::from_scale_rotation_translation(
        Vec3::from_slice(&s),
        Quat::from_xyzw(r[0], r[1], r[2], r[3]).normalize(),
        Vec3::from_slice(&t),
    )
}

fn traverse(doc: &Value, roots: Vec<(usize, Mat4)>) -> Result<Vec<(usize, Mat4)>> {
    let nodes = doc["nodes"].as_array().context("model has no nodes")?;
    let mut stack: Vec<_> = roots.into_iter().rev().collect();
    let mut rows = Vec::new();
    while let Some((index, world)) = stack.pop() {
        let node = nodes.get(index).context("source node out of range")?;
        rows.push((index, world));
        for child in node["children"].as_array().into_iter().flatten().rev() {
            let index = child.as_u64().context("invalid source child")? as usize;
            stack.push((
                index,
                world * local(nodes.get(index).context("source child out of range")?),
            ));
        }
    }
    Ok(rows)
}

type Nodes = Vec<(usize, Mat4)>;

fn selection(doc: &Value, selector: Option<&str>) -> Result<(Nodes, Vec3)> {
    let scene = doc["scene"].as_u64().unwrap_or(0) as usize;
    let roots = doc["scenes"][scene]["nodes"]
        .as_array()
        .context("default scene has no roots")?
        .iter()
        .map(|value| {
            let index = value.as_u64().context("invalid source root")? as usize;
            Ok((index, local(&doc["nodes"][index])))
        })
        .collect::<Result<Vec<_>>>()?;
    let rows = traverse(doc, roots)?;
    let Some(selector) = selector else {
        return Ok((rows, Vec3::ZERO));
    };
    let mut candidates = rows.iter().filter(|(index, _)| {
        if let Some(name) = selector.strip_prefix("name:") {
            doc["nodes"][*index]["name"].as_str() == Some(name)
        } else if let Ok(selected) = selector.parse::<usize>() {
            *index == selected
        } else {
            doc["nodes"][*index]["name"].as_str() == Some(selector)
        }
    });
    let &(index, world) = candidates
        .next()
        .with_context(|| format!("source node {selector} not found"))?;
    ensure!(
        candidates.next().is_none(),
        "source node {selector} is ambiguous"
    );
    Ok((
        traverse(doc, vec![(index, world)])?,
        -world.transform_point3(Vec3::ZERO),
    ))
}

pub(super) fn bounds(doc: &Value, node: Option<&str>) -> Result<(Vec3, Vec3)> {
    let (rows, offset) = selection(doc, node)?;
    let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for (index, world) in rows {
        let Some(mesh) = doc["nodes"][index]["mesh"].as_u64() else {
            continue;
        };
        for primitive in doc["meshes"][mesh as usize]["primitives"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let accessor = &doc["accessors"][primitive["attributes"]["POSITION"]
                .as_u64()
                .context("missing positions")?
                as usize];
            let point = |name: &str| {
                Vec3::from_array(std::array::from_fn(|i| {
                    accessor[name][i].as_f64().unwrap_or(0.) as f32
                }))
            };
            let (lo, hi) = (point("min"), point("max"));
            for corner in 0..8 {
                let point = Vec3::new(
                    if corner & 1 == 0 { lo.x } else { hi.x },
                    if corner & 2 == 0 { lo.y } else { hi.y },
                    if corner & 4 == 0 { lo.z } else { hi.z },
                );
                let point = world.transform_point3(point) + offset;
                min = min.min(point);
                max = max.max(point);
            }
        }
    }
    ensure!(min.x <= max.x, "no source geometry");
    Ok((min, max))
}

pub(super) fn material_slots(doc: &Value) -> Result<Vec<usize>> {
    let (rows, _) = selection(doc, None)?;
    let mut slots = Vec::new();
    for (index, _) in rows {
        if let Some(mesh) = doc["nodes"][index]["mesh"].as_u64() {
            for primitive in doc["meshes"][mesh as usize]["primitives"]
                .as_array()
                .into_iter()
                .flatten()
            {
                slots.push(primitive["material"].as_u64().unwrap_or(0) as usize);
            }
        }
    }
    Ok(slots)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use serde_json::json;

    #[test]
    fn selected_bounds_retain_parent_rotation_and_scale_at_the_source_pivot() {
        let half = std::f32::consts::FRAC_1_SQRT_2;
        let doc = json!({
            "scene":0, "scenes":[{"nodes":[0,3]}],
            "nodes":[
                {"translation":[10,0,20],"rotation":[0,half,0,half],"scale":[2,2,2],"children":[1]},
                {"name":"piece","translation":[1,0,0],"mesh":0,"children":[2]},
                {"translation":[0,1,0],"mesh":1},
                {"name":"unused","translation":[100,0,0],"mesh":1}
            ],
            "meshes":[
                {"primitives":[{"attributes":{"POSITION":0},"material":4}]},
                {"primitives":[{"attributes":{"POSITION":1},"material":2}]}
            ],
            "accessors":[{"min":[0,0,0],"max":[1,1,1]},{"min":[0,0,0],"max":[0.5,0.5,0.5]}]
        });
        let (min, max) = bounds(&doc, Some("piece")).unwrap();
        assert!(min.abs_diff_eq(Vec3::new(0., 0., -2.), 1e-5));
        assert!(max.abs_diff_eq(Vec3::new(2., 3., 0.), 1e-5));
        assert_eq!(bounds(&doc, Some("1")).unwrap(), (min, max));
        assert_eq!(material_slots(&doc).unwrap(), vec![4, 2, 2]);
    }
}
