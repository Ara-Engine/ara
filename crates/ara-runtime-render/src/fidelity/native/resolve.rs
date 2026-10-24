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
        Self::resolve_inner(frame, workspace, schema, library, source, false, &|_| false)
    }

    /// Editor-only incremental resolution: pending terrain maps use neutral
    /// factors and pending foliage parts use the model placeholder. Strict Play
    /// and export resolution still rejects missing generations.
    pub fn resolve_available(
        frame: &RenderFrame,
        workspace: &Workspace,
        schema: &SchemaDb,
        library: &Library,
        source: &dyn AssetSource,
    ) -> Result<Self> {
        Self::resolve_inner(frame, workspace, schema, library, source, true, &|_| false)
    }

    /// Incremental editor resolution with quiet loading bounds for registered,
    /// still-unpublished models, including every part of foliage prefabs. The
    /// read-only callback excludes failed/missing IDs and reimports with last-good.
    pub fn resolve_available_with_pending(
        frame: &RenderFrame,
        workspace: &Workspace,
        schema: &SchemaDb,
        library: &Library,
        source: &dyn AssetSource,
        pending: &dyn Fn(&str) -> bool,
    ) -> Result<Self> {
        Self::resolve_inner(frame, workspace, schema, library, source, true, pending)
    }

    fn resolve_inner(
        frame: &RenderFrame,
        workspace: &Workspace,
        schema: &SchemaDb,
        library: &Library,
        source: &dyn AssetSource,
        available: bool,
        pending: &dyn Fn(&str) -> bool,
    ) -> Result<Self> {
        ara_gpu::cpu_scope!("Native.resolve_content");
        let mut content = Self::default();
        let Some(terrain) = &frame.terrain else {
            return Ok(content);
        };
        for reference in &terrain.data.header.materials {
            let id = asset_id(reference)?;
            let material = match library.materials.get(id) {
                Some(material) => Arc::clone(material),
                None if available => when_published(source, |source| {
                    Ok(Arc::new(MaterialAsset::compile(workspace, source, id)?))
                })?
                .unwrap_or_else(|| {
                    Arc::new(MaterialAsset {
                        asset: id.to_owned(),
                        digest: ara_asset::Digest::of(id.as_bytes()),
                        material: Default::default(),
                        textures: Vec::new(),
                        texture_generations: Vec::new(),
                        base_color_decode_srgb: false,
                        emissive_decode_srgb: false,
                    })
                }),
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
                let parts = parts(
                    reference, workspace, schema, library, source, available, pending,
                )?;
                content
                    .unpublished_foliage
                    .insert(reference.0.to_owned(), parts.missing);
                content
                    .foliage_parts
                    .insert(reference.0.to_owned(), parts.draws);
            }
        }
        Ok(content)
    }
}

struct Parts {
    draws: Vec<Draw>,
    missing: BTreeMap<u64, String>,
}
fn parts(
    reference: &AssetRef,
    workspace: &Workspace,
    schema: &SchemaDb,
    library: &Library,
    source: &dyn AssetSource,
    available: bool,
    pending: &dyn Fn(&str) -> bool,
) -> Result<Parts> {
    let id = asset_id(reference)?;
    let mut scratch = Library::default();
    scratch.models = library.models.clone();
    scratch.materials = library.materials.clone();
    let mut world = World::new();
    let mut matrices = BTreeMap::new();
    let mut models = BTreeMap::new();
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
                if let Some(model) = &renderer.model {
                    models.insert(address.to_owned(), asset_id(model)?.to_owned());
                }
                prepare_part(&mut scratch, &renderer, workspace, source, available)?;
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
        prepare_part(&mut scratch, &renderer, workspace, source, available)?;
        matrices.insert("e_source".into(), Mat4::IDENTITY);
        models.insert("e_source".into(), id.to_owned());
        world.spawn((
            ara_sim::Identity("e_source".into()),
            renderer,
            WorldPose(Pose::IDENTITY),
        ));
    }
    world.insert_resource(scratch);
    let mut frame = RenderFrame::default();
    frame.extract(&world, 0, 1.0);
    if available {
        crate::ModelPlaceholders::default().style(&mut frame, &models, pending, true);
    }
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
    let missing = frame
        .draws
        .iter()
        .filter(|draw| matches!(draw.geometry, ara_runtime::extract::Geometry::Shape { .. }))
        .filter_map(|draw| {
            models
                .get(&draw.address)
                .map(|asset| (draw.entity, asset.to_owned()))
        })
        .collect();
    Ok(Parts {
        draws: frame.draws,
        missing,
    })
}

fn prepare_part(
    library: &mut Library,
    renderer: &MeshRenderer,
    workspace: &Workspace,
    source: &dyn AssetSource,
    available: bool,
) -> Result<()> {
    if available {
        when_published(source, |source| {
            prepare_renderer(library, renderer, workspace, source)
        })?;
        Ok(())
    } else {
        prepare_renderer(library, renderer, workspace, source)
    }
}

pub(super) fn prepare_renderer(
    library: &mut Library,
    renderer: &MeshRenderer,
    workspace: &Workspace,
    source: &dyn AssetSource,
) -> Result<()> {
    if let Some(reference) = &renderer.model {
        let id = asset_id(reference)?;
        // Parts of one prefab share the library's decoded generation. Metadata
        // changes reload once; unchanged parts never read or decode its payload.
        if library.models.get(id).is_none_or(|model| {
            source
                .digest(id)
                .is_some_and(|digest| digest != model.digest)
        }) {
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
