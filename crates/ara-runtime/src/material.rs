//! Native material graphs lower to immutable PBR data at content boundaries.
//! The workspace remains authoritative; renderer uploads never contain another AST.
mod expr;
mod lower;
#[cfg(test)]
mod tests;
mod uv;

use crate::{
    Result, RuntimeError,
    assets::AssetSource,
    model::{ModelMaterial, ModelTexture},
};
use ara_asset::Digest;
use ara_doc::Workspace;
use ara_material::{Binding, Expr, InstanceAsset, Material, ParameterValue};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// A texture's independently published generation, shared across material instances.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TextureGeneration {
    /// Source import sidecar's stable asset ID.
    pub asset: String,
    /// Published import action digest.
    pub digest: Digest,
}

/// Immutable, exactly representable PBR material data for a graph or material instance.
/// Unsupported reachable expressions diagnose instead of silently approximating them.
#[derive(Debug)]
pub struct MaterialAsset {
    /// Stable graph or material-instance asset ID.
    pub asset: String,
    /// Definition/override/texture generation identity, excluding graph editor layout.
    pub digest: Digest,
    /// Surface factors and texture slots into `textures`.
    pub material: ModelMaterial,
    /// Cooked texture bytes, ready for renderer residency without source decoding.
    pub textures: Vec<ModelTexture>,
    /// Source generation for each texture, allowing reuse across cheap instances.
    pub texture_generations: Vec<TextureGeneration>,
    /// Decode the raw base-color sample from sRGB after filtering (`sampleColor`).
    pub base_color_decode_srgb: bool,
    /// Decode the raw emissive sample from sRGB after filtering (`sampleColor`).
    pub emissive_decode_srgb: bool,
}

/// Resolved native material generations. Failed reloads retain their last good data.
#[derive(Default, Clone)]
pub struct Materials {
    entries: BTreeMap<String, Arc<MaterialAsset>>,
    wanted: BTreeSet<String>,
}

impl Materials {
    /// The latest successfully resolved material.
    pub fn get(&self, asset: &str) -> Option<&Arc<MaterialAsset>> {
        self.entries.get(asset)
    }

    /// Loads a graph/instance on a control loader, retaining the prior generation on error.
    pub fn load(
        &mut self,
        workspace: &Workspace,
        source: &dyn AssetSource,
        asset: &str,
    ) -> Result<bool> {
        self.wanted.insert(asset.to_owned());
        let prepared = MaterialAsset::compile(workspace, source, asset)?;
        if self
            .entries
            .get(asset)
            .is_some_and(|entry| entry.digest == prepared.digest)
        {
            return Ok(false);
        }
        self.entries.insert(asset.to_owned(), Arc::new(prepared));
        Ok(true)
    }

    /// Every requested asset, including failures eligible for later recovery.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.wanted.iter().map(String::as_str)
    }
}

impl MaterialAsset {
    /// Resolves authoring defaults and instance values, then lowers only expressions
    /// representable by the renderer's PBR contract. No graph parsing or asset IO is
    /// needed during frame extraction.
    pub fn compile(workspace: &Workspace, source: &dyn AssetSource, asset: &str) -> Result<Self> {
        let mut material = definition(workspace, asset)?;
        material
            .resolve_assets(workspace.index())
            .map_err(diagnostics)?;
        ara_material::check(&material).map_err(diagnostics)?;
        // Artistic parameter values are fixed for this immutable asset generation.
        // Texture parameters remain symbolic until the loader resolves published bytes.
        let numeric: Vec<_> = material
            .parameters
            .iter()
            .filter(|(_, p)| !p.ty.is_texture())
            .map(|(id, p)| (id.clone(), p.clone()))
            .collect();
        for (id, parameter) in numeric {
            let values = match parameter.default {
                ParameterValue::Numeric(values) => values,
                ParameterValue::Bool(value) => vec![f64::from(u8::from(value))],
                ParameterValue::Texture(_) => {
                    return Err(RuntimeError(format!(
                        "{asset}#{id}: expected a numeric parameter"
                    )));
                }
            };
            material.parameters.remove(&id);
            material.bindings.insert(
                id,
                Binding {
                    name: parameter.name,
                    order: parameter.order,
                    ty: parameter.ty,
                    expr: Expr::Literal { values },
                },
            );
        }
        let material = ara_material::fold_constants(&material).map_err(diagnostics)?;
        let prepared = lower::surface(&material, source)?;
        let identity = ara_json::to_value(&(&material, &prepared.generations))?;
        let digest = Digest::of(ara_json::to_canonical_string(&identity).as_bytes());
        Ok(Self {
            asset: asset.to_owned(),
            digest,
            material: prepared.material,
            textures: prepared.textures,
            texture_generations: prepared.generations,
            base_color_decode_srgb: prepared.base_color_decode_srgb,
            emissive_decode_srgb: prepared.emissive_decode_srgb,
        })
    }
}

fn definition(workspace: &Workspace, asset: &str) -> Result<Material> {
    let document = workspace
        .doc(asset)
        .ok_or_else(|| RuntimeError(format!("{asset}: material document is not loaded")))?;
    if document.kind == "materialinstance" {
        let instance =
            InstanceAsset::from_document(document).map_err(|d| RuntimeError(d.to_string()))?;
        let definition = crate::assets::asset_id(&instance.definition)?;
        let graph = workspace.doc(definition).ok_or_else(|| {
            RuntimeError(format!("{asset}: definition {definition} is not loaded"))
        })?;
        let graph =
            ara_graph::Graph::from_document(graph).map_err(|d| RuntimeError(d.to_string()))?;
        let mut material = Material::from_graph(&graph).map_err(diagnostics)?;
        let resolved = instance.resolve(&material).map_err(diagnostics)?;
        for (id, value) in resolved.overrides {
            let parameter = material.parameters.get_mut(&id).ok_or_else(|| {
                RuntimeError(format!("{asset}#{id}: override parameter is missing"))
            })?;
            parameter.default = value;
        }
        Ok(material)
    } else {
        let graph =
            ara_graph::Graph::from_document(document).map_err(|d| RuntimeError(d.to_string()))?;
        Material::from_graph(&graph).map_err(diagnostics)
    }
}

fn diagnostics(errors: Vec<ara_diag::Diagnostic>) -> RuntimeError {
    RuntimeError(
        errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; "),
    )
}

pub(crate) fn references(workspace: &Workspace) -> Result<BTreeSet<String>> {
    fn record(
        value: &serde_json::Map<String, serde_json::Value>,
        references: &mut BTreeSet<String>,
    ) -> Result<()> {
        if let Some(renderer) = value.get("MeshRenderer") {
            let renderer: crate::components::MeshRenderer =
                serde_json::from_value(renderer.clone())?;
            for entry in renderer.material_overrides {
                references.insert(crate::assets::asset_id(&entry.material)?.to_owned());
            }
        }
        // Prefab instance overrides contain partial component fields, so collect
        // their typed reference values without materializing incomplete renderers.
        if let Some(overrides) = value.get("overrides") {
            collect_renderer_overrides(overrides, references);
        }
        Ok(())
    }
    let mut references = BTreeSet::new();
    for (_, document) in workspace.docs() {
        for value in document.records().values() {
            record(value, &mut references)?;
        }
    }
    Ok(references)
}

fn collect_renderer_overrides(value: &serde_json::Value, assets: &mut BTreeSet<String>) {
    if let Some(values) = value.as_object() {
        for (key, value) in values {
            if key.contains("MeshRenderer") {
                collect_material_refs(value, assets);
            } else {
                collect_renderer_overrides(value, assets);
            }
        }
    }
}

fn collect_material_refs(value: &serde_json::Value, assets: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::String(reference) => {
            if let Ok(parsed) = ara_schema::grammar::parse_ref(reference)
                && let Some(id) = parsed.id
                && (id.starts_with("gr_") || id.starts_with("mi_"))
            {
                assets.insert(id.to_owned());
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                collect_material_refs(value, assets);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values() {
                collect_material_refs(value, assets);
            }
        }
        _ => {}
    }
}

pub(crate) fn patch_references(ops: &[ara_doc::Op]) -> BTreeSet<String> {
    let mut assets = BTreeSet::new();
    for op in ops {
        match op {
            ara_doc::Op::Set {
                path,
                value: Some(value),
                ..
            } if path.to_string().contains("MeshRenderer") => {
                collect_material_refs(value, &mut assets)
            }
            ara_doc::Op::Create { record, .. } => {
                if let Some(value) = record.get("MeshRenderer") {
                    collect_material_refs(value, &mut assets);
                }
                if let Some(value) = record.get("overrides") {
                    collect_renderer_overrides(value, &mut assets);
                }
            }
            _ => {}
        }
    }
    assets
}
