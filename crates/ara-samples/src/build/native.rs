//! Real source models stay immutable. Native prefabs compose their selected nodes.
use super::{
    Ctx, bounds,
    place::{Model, Models, create, short, transform_euler},
};
use crate::kit::Part;
use anyhow::{Context, Result, ensure};
use glam::{Mat4, Quat, Vec3};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn source(ctx: &Ctx, asset: &str) -> Result<(String, Value)> {
    let entry = ctx
        .lock
        .get(asset)
        .with_context(|| format!("{asset} is not pinned"))?;
    let file = entry
        .entry(&[".gltf", ".glb"])
        .context("model has no glTF")?;
    let path = entry.relative(&file.path);
    let id = format!("as_{}", short(asset));
    let doc = bounds::read(&ctx.root.join(&path))?;
    let mut opacity = serde_json::Map::new();
    for (index, image) in doc["images"].as_array().into_iter().flatten().enumerate() {
        let Some(uri) = image["uri"].as_str().filter(|uri| uri.contains("_diff_")) else {
            continue;
        };
        for suffix in ["_alpha_", "_opacity_"] {
            let mask = Path::new(&uri.replace("_diff_", suffix)).with_extension("png");
            let relative = Path::new(&file.path)
                .parent()
                .unwrap_or(Path::new(""))
                .join(&mask)
                .to_string_lossy()
                .replace('\\', "/");
            if entry.files.iter().any(|file| file.path == relative) {
                opacity.insert(
                    format!("#{index}"),
                    mask.to_string_lossy().replace('\\', "/").into(),
                );
                break;
            }
        }
    }
    let settings = if opacity.is_empty() {
        json!({})
    } else {
        json!({"opacityMaps":opacity})
    };
    ctx.sidecar(&path, &id, settings)?;
    Ok((format!("@{path}#{id}"), doc))
}

fn emission(renderer: &mut Value, materials: &[&str]) {
    if !materials.is_empty() {
        renderer["emissiveFactor"] = json!([1, 1, 1]);
        renderer["emissiveMaterials"] = json!(materials);
    }
}

pub(super) fn whole(
    ctx: &Ctx,
    models: &mut Models,
    name: &str,
    asset: &str,
    lit: &[&str],
) -> Result<()> {
    let (reference, doc) = source(ctx, asset)?;
    let (min, max) = bounds::bounds(&doc, None)?;
    let mut renderer = json!({"model":reference});
    emission(&mut renderer, lit);
    models.0.insert(
        name.into(),
        Model {
            reference,
            prefab: false,
            renderer,
            parts: vec![],
            min,
            max,
        },
    );
    Ok(())
}

pub(super) fn parts(
    ctx: &Ctx,
    models: &mut Models,
    name: &str,
    asset: &str,
    parts: &[Part<'_>],
    lit: &[&str],
) -> Result<()> {
    ensure!(!parts.is_empty(), "native prefab must contain a part");
    let (reference, doc) = source(ctx, asset)?;
    let path = format!("prefabs/{name}.prefab");
    let id = format!("pf_{}", short(name));
    ctx.new_document(&path, "prefab", &id)?;
    let mut commands = vec![create("e_root", name, json!({"Transform":{}}))];
    let mut children = Vec::new();
    let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for (index, part) in parts.iter().enumerate() {
        let child = format!("e_part{index}");
        children.push(child.clone());
        let mut renderer = json!({"model":reference,"node":part.node});
        emission(&mut renderer, lit);
        let mut call = create(
            &child,
            part.node,
            json!({"Transform":transform_euler(Vec3::from_array(part.at),Vec3::new(0.,part.yaw,0.)),"MeshRenderer":renderer}),
        );
        call.1["parent"] = json!("e_root");
        commands.push(call);
        let (lo, hi) = bounds::bounds(&doc, Some(part.node))?;
        let transform = Mat4::from_rotation_translation(
            Quat::from_rotation_y(part.yaw.to_radians()),
            Vec3::from_array(part.at),
        );
        for corner in 0..8 {
            let point = Vec3::new(
                if corner & 1 == 0 { lo.x } else { hi.x },
                if corner & 2 == 0 { lo.y } else { hi.y },
                if corner & 4 == 0 { lo.z } else { hi.z },
            );
            let point = transform.transform_point3(point);
            min = min.min(point);
            max = max.max(point);
        }
    }
    ctx.apply(
        &path,
        "Compose real kit nodes as native prefab parts",
        commands,
    )?;
    models.0.insert(
        name.into(),
        Model {
            reference: format!("@{path}#{id}"),
            prefab: true,
            renderer: json!({}),
            parts: children,
            min,
            max,
        },
    );
    Ok(())
}
