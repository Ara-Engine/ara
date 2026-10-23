//! Persisted instance overrides use the same document/command/undo pipeline as graphs.

use std::collections::BTreeMap;

use ara_cmd::{Command, EditCtx, Registry};
use ara_diag::Diagnostic;
use ara_doc::{AssetId, Document, RecordId};
use ara_schema::{AssetRef, DocKindDesc, FieldPath, Schema, SchemaDb};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Instance, Material, ParameterValue, codes};

/// A saved material instance; all authoritative overrides are document fields.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "i"), name = "MaterialInstance", module = "material")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstanceAsset {
    /// Definition graph as a full project-relative `@path#gr_id` reference.
    #[ara(asset_kind = "graph")]
    pub definition: AssetRef,
    /// Typed numeric, boolean and symbolic texture overrides; defaults stay in the graph.
    #[serde(default)]
    pub overrides: BTreeMap<String, ParameterValue>,
}

/// Registers graph storage and the `materialinstance` document kind.
/// Call before material command registration; graph registration is idempotent here.
pub fn register(db: &mut SchemaDb) -> Result<(), Diagnostic> {
    if db.kind("graph").is_none() {
        ara_graph::register(db)?;
    }
    db.register::<InstanceAsset>();
    if db.kind("materialinstance").is_none() {
        db.add_kind(DocKindDesc {
            name: "materialinstance".into(),
            version: 1,
            doc: "A material definition reference and cheap per-object parameter overrides.".into(),
            id_prefix: "mi".into(),
            extension: "materialinstance".into(),
            records: vec!["MaterialInstance".into()],
            single_root: true,
        })?;
    }
    Ok(())
}

impl InstanceAsset {
    /// Creates an import/initial snapshot; later changes use typed instance commands.
    pub fn to_document(&self, id: AssetId) -> Result<Document, Diagnostic> {
        if id.prefix() != "mi" {
            return Err(Diagnostic::error(
                codes::OVERRIDE,
                "instance asset ID must start mi_",
            ));
        }
        let record = ara_json::to_value(self).map_err(error)?;
        let mut doc = Document::new("materialinstance", id);
        doc.insert_unchecked(
            record_id()?,
            record
                .as_object()
                .cloned()
                .ok_or_else(|| error("instance must be an object"))?,
        );
        Ok(doc)
    }
    /// Decodes an instance snapshot from its one canonical record.
    pub fn from_document(doc: &Document) -> Result<Self, Diagnostic> {
        if doc.kind != "materialinstance" || doc.records().len() != 1 {
            return Err(error("expected one material instance record"));
        }
        let record = doc
            .record("i_values")
            .ok_or_else(|| error("instance is missing i_values"))?;
        serde_json::from_value(Value::Object(record.clone())).map_err(error)
    }
    /// Resolves validated overrides against the referenced definition.
    pub fn resolve(&self, material: &Material) -> Result<Instance, Vec<Diagnostic>> {
        let reference =
            ara_schema::grammar::parse_ref(&self.definition.0).map_err(|e| vec![error(e)])?;
        if reference.path.is_none() || reference.id != Some(material.id.as_str()) {
            return Err(vec![error(
                "instance definition must be @path#id for this material",
            )]);
        }
        let instance = Instance {
            overrides: self.overrides.clone(),
        };
        instance.validate(material)?;
        Ok(instance)
    }
}

fn error(e: impl ToString) -> Diagnostic {
    Diagnostic::error(codes::OVERRIDE, e.to_string())
}
fn record_id() -> Result<RecordId, Diagnostic> {
    RecordId::parse("i_values").map_err(error)
}

/// Edits one saved material instance override.
#[derive(Schema, Deserialize)]
#[ara(params, name = "MaterialInstanceSetParams", module = "material")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetOverrideParams {
    /// Stable definition parameter ID.
    pub node: String,
    /// Numeric, boolean or symbolic texture value; static switches require booleans.
    pub values: ParameterValue,
}

/// Removes a saved override, restoring the definition default.
#[derive(Schema, Deserialize)]
#[ara(params, name = "MaterialInstanceClearParams", module = "material")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClearOverrideParams {
    /// Stable definition parameter ID.
    pub node: String,
}

/// Validated instance override mutation; records a field-granular kernel op.
pub struct SetOverride;
impl Command for SetOverride {
    type Params = SetOverrideParams;
    type Output = Value;
    const NAME: &'static str = "material.instance.override";
    const SUMMARY: &'static str = "Set a validated material instance override.";
    fn examples() -> Vec<Value> {
        vec![json!({"node":"n_rough","values":[0.4]})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<Value, Diagnostic> {
        let mut asset = InstanceAsset::from_document(ctx.read().doc())?;
        let mut values = p.values;
        crate::assets::repair(&mut values, ctx.read().workspace().index())?;
        asset.overrides.insert(p.node.clone(), values.clone());
        let read = ctx.read();
        let id = read.workspace().find(&asset.definition.0).ok_or_else(|| {
            error("load the material definition before editing instance overrides")
        })?;
        let graph = read
            .workspace()
            .doc(id.as_str())
            .ok_or_else(|| error("material definition is unloaded"))?;
        let graph = ara_graph::Graph::from_document(graph)?;
        let m = Material::from_graph(&graph).map_err(|d| {
            d.into_iter()
                .next()
                .unwrap_or_else(|| error("invalid definition"))
        })?;
        asset.resolve(&m).map_err(|d| {
            d.into_iter()
                .next()
                .unwrap_or_else(|| error("invalid override"))
        })?;
        ctx.set(
            &record_id()?,
            FieldPath::field("overrides").key(p.node),
            Some(json!(values)),
        )?;
        Ok(Value::Null)
    }
}

/// Restores a definition default through an invertible field removal.
pub struct ClearOverride;
impl Command for ClearOverride {
    type Params = ClearOverrideParams;
    type Output = Value;
    const NAME: &'static str = "material.instance.clear";
    const SUMMARY: &'static str = "Restore a material definition parameter default.";
    fn examples() -> Vec<Value> {
        vec![json!({"node":"n_rough"})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<Value, Diagnostic> {
        InstanceAsset::from_document(ctx.read().doc())?;
        ctx.set(
            &record_id()?,
            FieldPath::field("overrides").key(p.node),
            None,
        )?;
        Ok(Value::Null)
    }
}

pub(crate) fn register_commands(registry: &mut Registry, db: &mut SchemaDb) {
    registry
        .command::<SetOverride>(db)
        .command::<ClearOverride>(db);
}
