use crate::{Result, invalid};
use ara_doc::{AssetId, Document, RecordId, Workspace};
use ara_schema::{DocKindDesc, Schema, SchemaDb};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Stable grid address: X/Z in metres; Y remains unpartitioned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CellKey {
    /// Signed horizontal grid coordinate.
    pub x: i32,
    /// Signed depth grid coordinate.
    pub z: i32,
    /// Manifest layer record ID, represented separately by `Cell::layer`.
    pub slot: u32,
}

/// Root world settings, kept in the manifest's single `w_` record.
#[derive(Debug, Clone, Default, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "w"), module = "world")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct WorldSettings {
    /// Grid cell width and depth.
    #[ara(unit = Meters)]
    pub cell_size: f32,
}

/// Independently loaded and visible data layer, such as art, night or quest state.
#[derive(Debug, Clone, Default, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "l"), module = "world")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Layer {
    /// Human-readable label.
    pub name: String,
    /// Load cells in this layer when a source is within range.
    pub loaded: bool,
    /// Presentation visibility; hiding preserves simulation residency.
    pub visible: bool,
    /// Distance outside a cell's bounds at which it becomes wanted.
    #[ara(unit = Meters)]
    pub range: f32,
    /// Fixed host ticks a cell remains resident after leaving the wanted set.
    pub grace_ticks: u32,
}

/// One grid/layer partition, with a scene plus optional per-entity scene files.
#[derive(Debug, Clone, Default, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "c"), module = "world")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Cell {
    /// Grid X coordinate.
    pub x: i32,
    /// Grid Z coordinate.
    pub z: i32,
    /// Stable layer record ID in this manifest.
    pub layer: String,
    /// Unique slot differentiating partitions at the same coordinates.
    pub slot: u32,
    /// Cell scene `@path#documentId` references; every file has one owner.
    pub documents: Vec<String>,
    /// Optional standalone actor scene files, kept at readable stable paths.
    pub actors: Vec<String>,
}

impl Cell {
    /// Stable streaming and pack address independent of document filenames.
    pub fn key(&self) -> CellKey {
        CellKey {
            x: self.x,
            z: self.z,
            slot: self.slot,
        }
    }
}

/// Validated immutable view of a canonical root document.
#[derive(Debug)]
pub struct Manifest {
    /// Root document identity.
    pub id: AssetId,
    /// Settings record identity.
    pub settings_id: RecordId,
    /// Grid dimensions.
    pub settings: WorldSettings,
    /// Data layers keyed by their stable record IDs.
    pub layers: BTreeMap<String, Layer>,
    /// Partitions keyed by stable cell record IDs.
    pub cells: BTreeMap<String, Cell>,
}

impl Manifest {
    /// Reads a manifest without requiring its cell documents to be loaded.
    pub fn read(doc: &Document) -> Result<Self> {
        if doc.kind != "world" || !doc.conflicts.is_empty() {
            return Err(invalid("expected a conflict-free world manifest"));
        }
        let mut settings = None;
        let mut layers = BTreeMap::new();
        let mut cells: BTreeMap<String, Cell> = BTreeMap::new();
        for (id, record) in doc.records() {
            let value = serde_json::Value::Object(record.clone());
            match id.prefix() {
                "w" if settings.is_none() => {
                    settings = Some((id.clone(), serde_json::from_value(value)?))
                }
                "l" => {
                    layers.insert(id.to_string(), serde_json::from_value(value)?);
                }
                "c" => {
                    cells.insert(id.to_string(), serde_json::from_value(value)?);
                }
                _ => return Err(invalid("unknown manifest record or multiple settings")),
            }
        }
        let (settings_id, settings): (_, WorldSettings) =
            settings.ok_or_else(|| invalid("missing world settings"))?;
        if !settings.cell_size.is_finite() || settings.cell_size <= 0.0 {
            return Err(invalid("cellSize must be finite and positive"));
        }
        let layers: BTreeMap<String, Layer> = layers;
        for layer in layers.values() {
            if !layer.range.is_finite() || layer.range < 0.0 {
                return Err(invalid("layer range must be finite and nonnegative"));
            }
        }
        let mut keys = BTreeSet::new();
        let mut documents = BTreeSet::new();
        let mut paths = BTreeSet::new();
        for cell in cells.values() {
            if !layers.contains_key(&cell.layer) || !keys.insert(cell.key()) {
                return Err(invalid("unknown layer or duplicate cell key"));
            }
            for reference in cell.documents.iter().chain(&cell.actors) {
                let (path, id) = document_ref(reference)?;
                if !documents.insert(id) || !paths.insert(path) {
                    return Err(invalid("a document belongs to more than one cell"));
                }
            }
        }
        Ok(Self {
            id: doc.id.clone(),
            settings_id,
            settings,
            layers,
            cells,
        })
    }

    /// Resolves manifest-owned cell files in an authoring workspace.
    pub fn documents<'a>(
        &'a self,
        workspace: &'a Workspace,
        cell: &str,
    ) -> Result<Vec<(&'a str, &'a Document)>> {
        let cell = self
            .cells
            .get(cell)
            .ok_or_else(|| invalid("unknown cell"))?;
        cell.documents
            .iter()
            .chain(&cell.actors)
            .map(|reference| {
                let (path, id) = document_ref(reference)?;
                let doc = workspace
                    .doc(id)
                    .ok_or_else(|| invalid(format!("cell document {reference} is not loaded")))?;
                if doc.kind != "scene" || workspace.path_of(id) != Some(path) {
                    return Err(invalid("cell reference path/identity mismatch"));
                }
                if cell.actors.contains(reference) && doc.children(None).len() != 1 {
                    return Err(invalid("actor files require exactly one root subtree"));
                }
                Ok((path, doc))
            })
            .collect()
    }
}

pub(crate) fn document_ref(reference: &str) -> Result<(&str, &str)> {
    let parts = ara_schema::grammar::parse_ref(reference).map_err(invalid)?;
    let path = parts
        .path
        .ok_or_else(|| invalid("cell references need a path"))?;
    ara_pack::validate_path(path).map_err(|e| invalid(e.to_string()))?;
    let id = parts
        .id
        .ok_or_else(|| invalid("cell references need a stable document ID"))?;
    AssetId::parse(id).map_err(invalid)?;
    Ok((path, id))
}

/// Registers the manifest kind and its records; cell/entity files use `scene`.
pub fn register(db: &mut SchemaDb) {
    ara_doc::register(db);
    db.register::<WorldSettings>();
    db.register::<Layer>();
    db.register::<Cell>();
    let _ = db.add_kind(DocKindDesc {
        name: "world".into(),
        version: 1,
        doc: "Partitioned world manifest.".into(),
        id_prefix: "wd".into(),
        extension: "world".into(),
        records: vec!["WorldSettings".into(), "Layer".into(), "Cell".into()],
        single_root: false,
    });
}
