//! Editor presentation uses runtime extraction, without starting a playable world.
use super::*;
use ara_components::{
    post::PostProcessVolume,
    render::{HeightFog, ReflectionCapture, SkyAtmosphere},
};
use ara_doc::{Workspace, prefab::Resolved};
use ara_runtime::{
    animation::Library,
    assets::AssetSource,
    components::{Camera, Environment, Light, MeshRenderer, VisualEffect},
    math::Pose,
    sync::WorldPose,
    terrain::{Terrain, TerrainChunk, TerrainState},
};
use bevy_ecs::{component::Component, world::World};
use serde::de::DeserializeOwned;
use std::collections::BTreeSet;

/// Extracts resolved editor entities with runtime model selection, materials,
/// lights and VFX semantics. `transforms` contains complete preview world matrices,
/// including parent transforms. This is a content boundary, never a frame loop:
/// ready generations come from the project's asset owner. Unpublished models use
/// extraction's placeholder until a later publication; bad published data errors.
/// The caller retains `library` to reuse decoded models and native materials.
/// Pass `terrain` only when its manifest, foliage and world translation still match;
/// this reuses the evaluated generation during unrelated inspector edits/gestures.
/// `hidden` filters editor visibility without changing the complete parent transforms.
pub fn document_frame(
    entities: &Resolved,
    transforms: &BTreeMap<String, glam30::Mat4>,
    workspace: &Workspace,
    library: &mut Library,
    source: &dyn AssetSource,
    terrain: Option<&TerrainState>,
    hidden: &BTreeSet<String>,
) -> Result<RenderFrame> {
    document_frame_selected(
        entities, transforms, workspace, library, source, terrain, hidden, None,
    )
}

/// Extracts only the requested already-resolved entities for an editor value patch.
/// Parent matrices are supplied by the caller; terrain and unrelated assets are
/// never traversed when their entity IDs are absent from `only`.
#[allow(clippy::too_many_arguments)]
pub fn document_frame_selected(
    entities: &Resolved,
    transforms: &BTreeMap<String, glam30::Mat4>,
    workspace: &Workspace,
    library: &mut Library,
    source: &dyn AssetSource,
    terrain: Option<&TerrainState>,
    hidden: &BTreeSet<String>,
    only: Option<&BTreeSet<String>>,
) -> Result<RenderFrame> {
    let mut world = World::new();
    for (address, row) in entities {
        if hidden.contains(address) || only.is_some_and(|only| !only.contains(address)) {
            continue;
        }
        let matrix = transforms
            .get(address)
            .context("editor world transform missing")?;
        let matrix = glam::Mat4::from_cols_array(&matrix.to_cols_array());
        anyhow::ensure!(
            matrix.is_finite() && matrix.determinant().abs() > 1e-12,
            "invalid editor world transform at {address}"
        );
        let pose = Pose::from_matrix(matrix);
        let name = row
            .record
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(address);
        let entity = world
            .spawn((
                ara_sim::Identity(address.to_owned()),
                ara_sim::SceneMetadata {
                    name: name.to_owned(),
                    ..Default::default()
                },
                WorldPose(pose),
                pose.to_transform(),
            ))
            .id();
        let mut entity = world.entity_mut(entity);
        insert::<Camera>(&mut entity, &row.record, "Camera")?;
        insert::<Environment>(&mut entity, &row.record, "Environment")?;
        insert::<SkyAtmosphere>(&mut entity, &row.record, "SkyAtmosphere")?;
        insert::<HeightFog>(&mut entity, &row.record, "HeightFog")?;
        insert::<ReflectionCapture>(&mut entity, &row.record, "ReflectionCapture")?;
        insert::<PostProcessVolume>(&mut entity, &row.record, "PostProcessVolume")?;
        insert::<Light>(&mut entity, &row.record, "Light")?;
        insert::<VisualEffect>(&mut entity, &row.record, "VisualEffect")?;
        insert::<TerrainChunk>(&mut entity, &row.record, "TerrainChunk")?;
        insert::<Terrain>(&mut entity, &row.record, "Terrain")?;
        if row.record.contains_key("Terrain") {
            anyhow::ensure!(
                matrix.abs_diff_eq(glam::Mat4::from_translation(pose.translation), 1e-5),
                "Terrain scene transforms support translation; bake rotation and scale into its grid"
            );
        }
        if let Some(value) = row.record.get("MeshRenderer") {
            let renderer: MeshRenderer = serde_json::from_value(value.to_owned())
                .with_context(|| format!("{address}.MeshRenderer"))?;
            when_published(source, |source| {
                resolve::prepare_renderer(library, &renderer, workspace, source)
            })?;
            entity.insert(renderer);
        }
    }
    world.insert_resource(std::mem::take(library));
    let result = (|| {
        if let Some(terrain) = terrain {
            world.insert_resource(terrain.to_owned());
        } else if let Some(terrain) = ara_runtime::terrain::build(&mut world, workspace)? {
            world.insert_resource(terrain);
        }
        let mut frame = RenderFrame::default();
        frame.extract(&world, 0, 1.0);
        // Pose decomposition matches runtime extraction; restore full affine source
        // matrices around selected pivots so preview parenting retains shear.
        for draw in &mut frame.draws {
            let matrix = transforms
                .get(&draw.address)
                .context("editor draw transform missing")?;
            let matrix = glam::Mat4::from_cols_array(&matrix.to_cols_array());
            draw.world = matrix * Pose::from_matrix(matrix).matrix().inverse() * draw.world;
        }
        Ok(frame)
    })();
    *library = world
        .remove_resource::<Library>()
        .expect("editor extraction retains its library");
    result
}

fn insert<T: Component + DeserializeOwned>(
    entity: &mut bevy_ecs::world::EntityWorldMut<'_>,
    record: &ara_doc::Record,
    name: &str,
) -> Result<()> {
    if let Some(value) = record.get(name) {
        entity.insert(
            serde_json::from_value::<T>(value.to_owned())
                .with_context(|| format!("editor {name}"))?,
        );
    }
    Ok(())
}
