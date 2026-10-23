use crate::{Manifest, MoveToCell, Result, SetLayer, TextIndex, WorldCommand, invalid};
use ara_doc::AssetId;
use ara_ops::{Store, TxnMeta};
use ara_schema::{Schema, SchemaDb};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// `world.cells.list` parameters.
#[derive(Debug, Schema, Serialize, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CellsList {
    /// Restrict results to this layer; absent lists every partition.
    pub layer: Option<String>,
}

/// `world.query` parameters, evaluated over the text index including unloaded cells.
#[derive(Debug, Schema, Serialize, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpatialQuery {
    /// Inclusive world-space minimum.
    #[ara(unit = Meters)]
    pub min: [f32; 3],
    /// Inclusive world-space maximum.
    #[ara(unit = Meters)]
    pub max: [f32; 3],
    /// Optional data layer filter.
    pub layer: Option<String>,
    /// Maximum results, bounded to 10,000.
    pub limit: u32,
}

/// Typed dispatcher for editor/CLI/MCP adapters. Reuses the authoritative kernel
/// store, so attribution, stale-base conflicts, undo and the op subscription work.
/// Host adapters must journal the resulting transaction and rebuild the text index
/// on a worker after authoring changes, then publish it at an allowed boundary.
pub struct WorldTools {
    world: AssetId,
    index: TextIndex,
    revision: u64,
}

impl WorldTools {
    /// Installs an immutable text index built from this exact store revision.
    pub fn new(world: AssetId, index: TextIndex, revision: u64) -> Self {
        Self {
            world,
            index,
            revision,
        }
    }

    /// Generates the four tool descriptions and their parameter JSON Schemas.
    pub fn catalog(db: &mut SchemaDb) -> Value {
        db.register::<CellsList>();
        db.register::<SpatialQuery>();
        db.register::<MoveToCell>();
        db.register::<SetLayer>();
        Value::Array([
            ("world.cells.list", "List world cells and data-layer states.", CellsList::describe()),
            ("world.query", "Query entity points across loaded and unloaded text-indexed cells.", SpatialQuery::describe()),
            ("world.entity.move_to_cell", "Atomically move an entity subtree between cells, retaining stable IDs.", MoveToCell::describe()),
            ("world.layer.set", "Set layer loading and visibility through attributed undoable ops.", SetLayer::describe()),
        ].into_iter().map(|(name, description, params)| json!({"name":name,"description":description,"inputSchema":db.type_json_schema(&params)})).collect())
    }

    /// Executes one named tool. Adapters authorize `read` or `edit:scene` before
    /// calling and provide the authenticated transaction metadata. `resident` is a
    /// read-only cell residency callback; it cannot alter text-index query coverage.
    pub fn call(
        &self,
        name: &str,
        params: Value,
        store: &mut Store,
        meta: TxnMeta,
        resident: impl Fn(&str) -> bool,
    ) -> Result<Value> {
        let document = store
            .workspace()
            .doc(self.world.as_str())
            .ok_or_else(|| invalid("world is not loaded"))?;
        match name {
            "world.cells.list" => {
                let p: CellsList = serde_json::from_value(params)?;
                let manifest = Manifest::read(document)?;
                Ok(Value::Array(manifest.cells.iter().filter(|(_,cell)| p.layer.as_ref().is_none_or(|layer| *layer == cell.layer)).map(|(id,cell)| {
                    let layer = &manifest.layers[&cell.layer];
                    json!({"cell":id,"key":cell.key(),"layer":cell.layer,"documents":cell.documents,"actors":cell.actors,"loaded":resident(id),"enabled":layer.loaded,"visible":layer.visible})
                }).collect()))
            }
            "world.query" => {
                if store.revision() != self.revision {
                    return Err(invalid(
                        "text index is stale; publish an index for the current revision",
                    ));
                }
                let p: SpatialQuery = serde_json::from_value(params)?;
                if p.limit > 10_000 {
                    return Err(invalid("query limit exceeds 10000"));
                }
                let rows = self
                    .index
                    .query(p.min, p.max, p.layer.as_deref(), p.limit as usize)?;
                Ok(Value::Array(
                    rows.into_iter()
                        .map(|row| json!({"entity":row,"loaded":resident(&row.cell)}))
                        .collect(),
                ))
            }
            "world.entity.move_to_cell" => {
                let command = WorldCommand::MoveToCell(serde_json::from_value(params)?);
                Ok(serde_json::to_value(
                    command.apply(store, &self.world, meta)?.txn(),
                )?)
            }
            "world.layer.set" => {
                let command = WorldCommand::SetLayer(serde_json::from_value(params)?);
                Ok(serde_json::to_value(
                    command.apply(store, &self.world, meta)?.txn(),
                )?)
            }
            _ => Err(invalid(format!("unknown world tool {name}"))),
        }
    }
}
