//! Workspace/pak resolution occurs before GPU publication, never inside a frame.
use super::*;
use anyhow::{bail, ensure};
use ara_doc::{Workspace, prefab::Resolved};
use ara_runtime::{
    animation::Library,
    assets::{AssetSource, asset_id},
    components::MeshRenderer,
    math::Pose,
    sync::WorldPose,
};
use ara_schema::{AssetRef, SchemaDb};
use bevy_ecs::world::World;
use glam::Mat4;
use std::collections::BTreeSet;

impl Content {
    /// Resolves terrain materials and each used foliage model/prefab from the same
    /// immutable workspace and cooked source as Play, including exported pak sources.
    /// A failed replacement returns an error before the last-good GPU view is touched.
    pub fn resolve(
        frame: &RenderFrame,
        workspace: &Workspace,
        schema: &SchemaDb,
        library: &Library,
        source: &dyn AssetSource,
    ) -> Result<Self> {
        let mut content = Self::default();
        let Some(terrain) = &frame.terrain else {
            return Ok(content);
        };
        for reference in &terrain.data.header.materials {
            let id = asset_id(reference)?;
            let material = match library.materials.get(id) {
                Some(material) => Arc::clone(material),
                None => Arc::new(MaterialAsset::compile(workspace, source, id)?),
            };
            content
                .terrain_materials
                .insert(reference.0.to_owned(), material);
        }
        let used: BTreeSet<_> = terrain
            .data
            .foliage
            .iter()
            .map(|i| i.identity[3] as usize)
            .collect();
        for index in used {
            let rule = terrain
                .data
                .foliage_rules
                .get(index)
                .context("invalid foliage rule index")?;
            let reference = &rule.rule.asset;
            if !content.foliage_parts.contains_key(&reference.0) {
                content.foliage_parts.insert(
                    reference.0.to_owned(),
                    parts(reference, workspace, schema, library, source)?,
                );
            }
        }
        Ok(content)
    }
}

fn parts(
    reference: &AssetRef,
    workspace: &Workspace,
    schema: &SchemaDb,
    library: &Library,
    source: &dyn AssetSource,
) -> Result<Vec<Draw>> {
    let id = asset_id(reference)?;
    let mut scratch = Library::default();
    scratch.models = library.models.clone();
    scratch.materials = library.materials.clone();
    let mut world = World::new();
    let mut matrices = BTreeMap::new();
    if let Some(document) = workspace
        .doc(id)
        .filter(|document| document.kind == "prefab")
    {
        let mut diagnostics = Vec::new();
        let entities = workspace.resolve(&document.id, schema, &mut diagnostics);
        ensure!(
            diagnostics.is_empty(),
            "foliage prefab {id}: {}",
            diagnostics
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        );
        for (address, entity) in &entities {
            let matrix = matrix(address, &entities, &mut matrices, &mut BTreeSet::new())?;
            if let Some(value) = entity.record.get("MeshRenderer") {
                let renderer: MeshRenderer = serde_json::from_value(value.clone())?;
                prepare_renderer(&mut scratch, &renderer, workspace, source)?;
                let name = entity
                    .record
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(address);
                world.spawn((
                    ara_sim::Identity(address.to_owned()),
                    ara_sim::SceneMetadata {
                        name: name.to_owned(),
                        ..Default::default()
                    },
                    renderer,
                    WorldPose(Pose::from_matrix(matrix)),
                ));
            }
        }
    } else {
        let renderer = MeshRenderer {
            model: Some(reference.clone()),
            ..Default::default()
        };
        prepare_renderer(&mut scratch, &renderer, workspace, source)?;
        matrices.insert("e_source".into(), Mat4::IDENTITY);
        world.spawn((
            ara_sim::Identity("e_source".into()),
            renderer,
            WorldPose(Pose::IDENTITY),
        ));
    }
    world.insert_resource(scratch);
    let mut frame = RenderFrame::default();
    frame.extract(&world, 0, 1.0);
    // Extract owns selector/pivot/emission/override semantics. Restore the full
    // prefab affine matrix around its selected pivot, preserving parent shear.
    for draw in &mut frame.draws {
        let matrix = matrices
            .get(&draw.address)
            .context("foliage part transform missing")?;
        let extracted = Pose::from_matrix(*matrix).matrix();
        draw.world = *matrix * extracted.inverse() * draw.world;
    }
    ensure!(
        !frame.draws.is_empty(),
        "foliage asset {id} contains no MeshRenderer geometry"
    );
    Ok(frame.draws)
}

pub(super) fn prepare_renderer(
    library: &mut Library,
    renderer: &MeshRenderer,
    workspace: &Workspace,
    source: &dyn AssetSource,
) -> Result<()> {
    if let Some(reference) = &renderer.model {
        let id = asset_id(reference)?;
        if library.models.get(id).is_none() {
            library.models.load(source, id)?;
        }
        let model = library
            .models
            .get(id)
            .context("foliage model generation missing")?;
        model.validate_renderer(renderer)?;
    }
    for entry in &renderer.material_overrides {
        let id = asset_id(&entry.material)?;
        if library.materials.get(id).is_none() {
            library.materials.load(workspace, source, id)?;
        }
    }
    Ok(())
}

fn matrix(
    address: &str,
    entities: &Resolved,
    matrices: &mut BTreeMap<String, Mat4>,
    visiting: &mut BTreeSet<String>,
) -> Result<Mat4> {
    if let Some(&matrix) = matrices.get(address) {
        return Ok(matrix);
    }
    if !visiting.insert(address.to_owned()) {
        bail!("foliage prefab parent cycle at {address}");
    }
    let entity = entities
        .get(address)
        .with_context(|| format!("foliage parent {address} is missing"))?;
    let transform = entity
        .record
        .get("Transform")
        .map(|value| serde_json::from_value::<ara_components::Transform>(value.clone()))
        .transpose()?
        .unwrap_or_default();
    let local = Pose::from_transform(&transform).matrix();
    let world = match &entity.parent {
        Some(parent) => matrix(parent, entities, matrices, visiting)? * local,
        None => local,
    };
    ensure!(
        world.is_finite() && world.determinant().abs() > 1e-12,
        "invalid foliage part transform {address}"
    );
    visiting.remove(address);
    matrices.insert(address.to_owned(), world);
    Ok(world)
}
