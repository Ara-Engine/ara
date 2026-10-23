//! Canonical animation documents and typed authoring commands producing kernel ops.

use crate::{
    Error,
    data::{ClipData, SkeletonData},
    graph::ControllerData,
};
use ara_doc::{AssetId, Document, Edit, RecordId};
use ara_schema::{AssetRef, DocKindDesc, FieldPath, Schema, SchemaDb};
use serde::{Deserialize, Serialize};

/// A stable authoring record holding imported data or a controller source graph.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "an"), module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct AnimAsset {
    /// Human-readable authoring name.
    pub name: String,
    /// Skeleton source data, when this record defines a rig.
    pub skeleton: Option<SkeletonData>,
    /// Canonical skeleton reference for clip/controller records.
    #[ara(asset_kind = "animation")]
    pub skeleton_ref: Option<AssetRef>,
    /// Source clip keys; the derived cooked blob lives in the asset cache.
    pub clip: Option<ClipData>,
    /// State-machine and pose-graph source, independent of editor layout.
    pub controller: Option<ControllerData>,
    /// Humanoid role bindings for import-time or runtime retargeting.
    pub rig: Option<crate::retarget::RigMap>,
    /// Procedural contact chains and ground-probe settings.
    pub ik: Option<IkSetup>,
    /// Offline pose/trajectory feature database configuration.
    pub motion_database: Option<crate::matching::MatchConfig>,
    /// Powered bodies, constraints and spring gains for physical animation.
    pub ragdoll: Option<crate::physical::RagdollProfile>,
}

/// Canonical procedural rig setup; goal values are fixed-tick gameplay inputs.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct IkSetup {
    /// Stable pelvis joint ID for full-body and foot-placement solvers.
    pub pelvis: String,
    /// Ordered chains used by body and hand goals.
    pub chains: Vec<crate::ik::ChainData>,
    /// Foot chains and anatomical sole-up vectors.
    pub feet: Vec<crate::ik::Foot>,
    /// Ground probe, slope and pelvis bounds.
    pub ground: crate::ik::FootSettings,
}

/// Registers the animation record, nested data/parameters and `.animation` kind.
pub fn register(db: &mut SchemaDb) -> Result<(), Error> {
    db.register::<AnimAsset>();
    db.register::<crate::physical::PhysicalState>();
    // Clips and controllers are referenced per record: `@anim/fox.animation#an_walk`.
    db.address_records("animation");
    db.add_kind(DocKindDesc {
        name: "animation".into(),
        version: 1,
        doc: "Animation skeletons, clips and controller graphs.".into(),
        id_prefix: "ac".into(),
        extension: "animation".into(),
        records: vec!["AnimAsset".into()],
        single_root: false,
    })
    .map_err(|d| Error(d.to_string()))
}

/// Strictly reads an animation document through the shared document validator.
pub fn read(text: &str, file: &str, db: &SchemaDb) -> Result<Document, Vec<ara_diag::Diagnostic>> {
    let result = ara_doc::read(text, file, db, None);
    if ara_diag::has_errors(&result.diags) {
        return Err(result.diags);
    }
    match result.doc {
        Some(doc) if doc.kind == "animation" => Ok(doc),
        _ => Err(vec![
            ara_diag::Diagnostic::error(
                ara_diag::codes::TYPE_MISMATCH,
                "expected an animation document",
            )
            .file(file),
        ]),
    }
}

/// Writes reflection order/default omission through ara-doc and ara-json.
pub fn write(doc: &Document, db: &SchemaDb) -> String {
    ara_doc::write(doc, db)
}

/// Decodes a typed record after kernel validation and canonical default omission.
pub fn asset(doc: &Document, id: &RecordId) -> Result<AnimAsset, Error> {
    let record = doc
        .record(id.as_str())
        .ok_or_else(|| Error("animation record does not exist".into()))?;
    serde_json::from_value(serde_json::Value::Object(record.clone()))
        .map_err(|e| Error(e.to_string()))
}

/// Typed mutation intent. Submit the returned edit through ara-ops, never write a
/// document or cache from an editor widget or plugin.
#[derive(Debug)]
// Cold authoring intents retain the existing by-value Create API.
#[allow(clippy::large_enum_variant)]
pub enum AnimationCommand {
    /// Creates a stable animation record.
    Create(AnimAsset),
    /// Replaces the rig source after an importer completes.
    SetSkeleton(SkeletonData),
    /// Replaces source clip keys.
    SetClip(ClipData),
    /// Replaces the controller after a graph edit.
    SetController(ControllerData),
    /// Replaces the semantic humanoid bone map.
    SetRig(crate::retarget::RigMap),
    /// Replaces the validated-at-cook procedural setup.
    SetIk(IkSetup),
    /// Replaces the cook-time pose-search feature configuration.
    SetMotionDatabase(crate::matching::MatchConfig),
    /// Replaces the powered-ragdoll source profile.
    SetRagdoll(crate::physical::RagdollProfile),
    /// Changes one schema-addressed field; the kernel validates its typed value.
    SetField {
        /// Stable field path, shared with the inspector and timeline.
        path: FieldPath,
        /// New typed JSON value, or none to remove it to its schema default.
        value: Option<ara_json::Value>,
    },
}
impl AnimationCommand {
    /// Lowers the typed intent into a kernel edit with automatic inverse ops.
    pub fn edit(self, doc: AssetId, id: RecordId) -> Result<Edit, Error> {
        let encode = |value| ara_json::to_value(&value).map_err(|e| Error(e.to_string()));
        let (path, value) = match self {
            Self::Create(asset) => {
                let value = encode(asset)?;
                let record = value
                    .as_object()
                    .ok_or_else(|| Error("animation asset must be an object".into()))?
                    .clone();
                return Ok(Edit::Create { doc, id, record });
            }
            Self::SetSkeleton(data) => (
                "skeleton".parse().map_err(Error)?,
                Some(ara_json::to_value(&data).map_err(|e| Error(e.to_string()))?),
            ),
            Self::SetClip(data) => (
                "clip".parse().map_err(Error)?,
                Some(ara_json::to_value(&data).map_err(|e| Error(e.to_string()))?),
            ),
            Self::SetController(data) => (
                "controller".parse().map_err(Error)?,
                Some(ara_json::to_value(&data).map_err(|e| Error(e.to_string()))?),
            ),
            Self::SetRig(data) => (
                "rig".parse().map_err(Error)?,
                Some(serde_json::to_value(data).map_err(|e| Error(e.to_string()))?),
            ),
            Self::SetIk(data) => (
                "ik".parse().map_err(Error)?,
                Some(serde_json::to_value(data).map_err(|e| Error(e.to_string()))?),
            ),
            Self::SetField { path, value } => (path, value),
            Self::SetRagdoll(data) => (
                "ragdoll".parse().map_err(Error)?,
                Some(serde_json::to_value(data).map_err(|e| Error(e.to_string()))?),
            ),
            Self::SetMotionDatabase(data) => (
                "motionDatabase".parse().map_err(Error)?,
                Some(serde_json::to_value(data).map_err(|e| Error(e.to_string()))?),
            ),
        };
        Ok(Edit::Set {
            doc,
            id,
            path,
            value,
        })
    }
    /// Applies one intent as an attributed atomic transaction in the kernel store.
    pub fn apply(
        self,
        store: &mut ara_ops::Store,
        meta: ara_ops::TxnMeta,
        doc: AssetId,
        id: RecordId,
    ) -> Result<ara_ops::Commit<'_>, Vec<ara_diag::Diagnostic>> {
        let edit = self.edit(doc, id).map_err(|e| {
            vec![ara_diag::Diagnostic::error(
                ara_diag::codes::TYPE_MISMATCH,
                e.to_string(),
            )]
        })?;
        store.transact(meta, [edit])
    }
}
