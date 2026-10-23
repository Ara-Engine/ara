use std::collections::BTreeMap;

use ara_diag::Diagnostic;
use ara_doc::{AssetId, Document, RecordId};
use ara_schema::{AssetRef, DocKindDesc, Schema, SchemaDb};
use serde::{Deserialize, Serialize};

use crate::{TileCoord, invalid};

/// Stable terrain grid settings; adjacent tiles duplicate their shared border.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "h"), name = "TerrainHeader", module = "terrain")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Header {
    /// World X/Z origin in metres.
    #[serde(default)]
    pub origin: [f32; 2],
    /// Metres between adjacent height samples.
    #[serde(default)]
    pub spacing: f32,
    /// Power-of-two cells per tile (16..256); samples are cells + 1.
    #[serde(default)]
    pub cells: u32,
    /// Inclusive minimum tile X/Z coordinates.
    #[serde(default)]
    pub min: TileCoord,
    /// Exclusive maximum tile X/Z coordinates.
    #[serde(default)]
    pub max: TileCoord,
    /// Up to four surface material references, in weight-channel order.
    #[serde(default)]
    pub materials: Vec<AssetRef>,
}

impl Header {
    /// Validates bounded allocation sizes and finite world coordinates.
    pub fn validate(&self) -> Result<(), Diagnostic> {
        if !self.origin.iter().all(|x| x.is_finite())
            || !(0.01..=64.).contains(&self.spacing)
            || !self.cells.is_power_of_two()
            || !(16..=256).contains(&self.cells)
            || self.min.x >= self.max.x
            || self.min.z >= self.max.z
            || [self.min.x, self.min.z, self.max.x, self.max.z]
                .iter()
                .any(|v| v.abs_diff(0) > 32768)
            || self.materials.len() > 4
        {
            return Err(invalid("invalid terrain grid, extent or material count"));
        }
        Ok(())
    }
    /// Tile width in metres.
    pub fn tile_width(&self) -> f32 {
        self.spacing * self.cells as f32
    }
    /// Whether this tile is inside the half-open terrain bounds.
    pub fn contains(&self, c: TileCoord) -> bool {
        c.x >= self.min.x && c.z >= self.min.z && c.x < self.max.x && c.z < self.max.z
    }
    /// Samples shared by every tile in this terrain.
    pub fn samples(&self) -> usize {
        (self.cells as usize + 1).pow(2)
    }
}

/// A non-destructive layer; order followed by stable ID defines composition.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "l"), name = "TerrainLayer", module = "terrain")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Layer {
    /// Display name in terrain tools.
    #[serde(default)]
    pub name: String,
    /// Stable ordering independent of record/map iteration order.
    #[serde(default)]
    pub priority: u32,
    /// Disabled layers are retained for undo and later re-enabling.
    #[serde(default)]
    pub enabled: bool,
    /// Blend opacity from zero to one.
    #[serde(default)]
    pub opacity: f32,
}

/// Operation performed by one recorded brush gesture.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(name = "TerrainOperation", module = "terrain")]
#[serde(rename_all = "camelCase")]
pub enum Operation {
    /// Add a smooth radial displacement in metres.
    #[default]
    Raise,
    /// Paint one normalized splat channel.
    Paint,
    /// Set or clear holes in the selected area.
    Hole,
    /// Add a rotated height stamp sampled from an immutable binary tile.
    Stamp,
    /// Apply a bounded deterministic thermal relaxation kernel.
    Erode,
    /// Replace samples from an imported tile inside an ordered, undoable layer.
    Import,
}

/// One brush dab; the stroke command groups all dabs into one transaction.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(name = "TerrainDab", module = "terrain")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Dab {
    /// World X/Z brush centre in metres.
    #[serde(default)]
    pub center: [f32; 2],
    /// Brush radius in metres.
    #[serde(default)]
    pub radius: f32,
    /// Signed displacement, paint strength or relaxation strength.
    #[serde(default)]
    pub amount: f32,
}

/// A complete gesture, retained as source rather than destructive tile snapshots.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "s"), name = "TerrainStroke", module = "terrain")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Stroke {
    /// Owning layer record ID.
    #[serde(default)]
    pub layer: String,
    /// Gesture ordering within a layer; stable ID breaks concurrent ties.
    #[serde(default)]
    pub sequence: u64,
    /// Typed operation.
    #[serde(default)]
    pub operation: Operation,
    /// Brush samples in gesture order (at most 4096).
    #[serde(default)]
    pub dabs: Vec<Dab>,
    /// Paint channel, from zero to three.
    #[serde(default)]
    pub channel: u32,
    /// Erosion iterations (1..32), otherwise zero.
    #[serde(default)]
    pub iterations: u32,
    /// Immutable stamp payload hash; absent for other operations.
    #[serde(default)]
    pub stamp: Option<String>,
    /// Destination grid tile for an import layer; absent for brush operations.
    #[serde(default)]
    pub source_tile: Option<TileCoord>,
    /// Stamp angle in radians.
    #[serde(default)]
    pub angle: f32,
}

/// Sparse touched-tile delta: a base payload and ordered references to gestures.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "t"), name = "TerrainTile", module = "terrain")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TileRecord {
    /// Stable tile coordinate.
    #[serde(default)]
    pub coord: TileCoord,
    /// Imported base tile in the content-addressed cache; absent means flat ground.
    #[serde(default)]
    pub base: Option<String>,
    /// Gesture IDs touching this tile, in transaction order.
    #[serde(default)]
    pub strokes: Vec<String>,
}

/// Read-only authoring snapshot for worker compilation.
#[derive(Debug, Clone, PartialEq)]
pub struct Terrain {
    /// Asset identity of the canonical manifest.
    pub id: AssetId,
    /// Grid configuration.
    pub header: Header,
    /// Layers keyed by persistent ID.
    pub layers: BTreeMap<String, Layer>,
    /// Gestures keyed by persistent ID.
    pub strokes: BTreeMap<String, Stroke>,
    /// Sparse tile records keyed by persistent ID.
    pub tiles: BTreeMap<String, TileRecord>,
}

/// Registers the terrain document and its reflected records.
pub fn register(db: &mut SchemaDb) -> Result<(), Diagnostic> {
    db.register::<Header>()
        .register::<Layer>()
        .register::<Stroke>()
        .register::<TileRecord>();
    db.add_kind(DocKindDesc {
        name: "terrain".into(),
        version: 1,
        doc: "Streamable layered heightfield manifest.".into(),
        id_prefix: "tr".into(),
        extension: "terrain".into(),
        records: vec![
            "TerrainHeader".into(),
            "TerrainLayer".into(),
            "TerrainStroke".into(),
            "TerrainTile".into(),
        ],
        single_root: false,
    })
}

pub(crate) fn value<T: Serialize>(v: &T) -> Result<serde_json::Value, Diagnostic> {
    serde_json::to_value(v).map_err(|e| invalid(e.to_string()))
}
pub(crate) fn record<T: Serialize>(v: &T) -> Result<ara_doc::Record, Diagnostic> {
    match value(v)? {
        serde_json::Value::Object(r) => Ok(r),
        _ => Err(invalid("expected record")),
    }
}

impl Terrain {
    /// Creates an initial/import snapshot. Live edits must use commands.
    pub fn new(id: AssetId, header: Header) -> Result<Self, Diagnostic> {
        header.validate()?;
        Ok(Self {
            id,
            header,
            layers: BTreeMap::new(),
            strokes: BTreeMap::new(),
            tiles: BTreeMap::new(),
        })
    }
    /// Decodes a kernel document for background evaluation.
    pub fn from_document(doc: &Document) -> Result<Self, Diagnostic> {
        if doc.kind != "terrain" {
            return Err(invalid("expected terrain document"));
        }
        let decode = |id: &str| {
            doc.record(id)
                .cloned()
                .map(serde_json::Value::Object)
                .ok_or_else(|| invalid("missing terrain header"))
        };
        let header =
            serde_json::from_value(decode("h_grid")?).map_err(|e| invalid(e.to_string()))?;
        let mut terrain = Self::new(doc.id.clone(), header)?;
        for (id, r) in doc.records() {
            let v = serde_json::Value::Object(r.clone());
            match id.prefix() {
                "l" => {
                    terrain.layers.insert(
                        id.to_string(),
                        serde_json::from_value(v).map_err(|e| invalid(e.to_string()))?,
                    );
                }
                "s" => {
                    terrain.strokes.insert(
                        id.to_string(),
                        serde_json::from_value(v).map_err(|e| invalid(e.to_string()))?,
                    );
                }
                "t" => {
                    terrain.tiles.insert(
                        id.to_string(),
                        serde_json::from_value(v).map_err(|e| invalid(e.to_string()))?,
                    );
                }
                "h" if id.as_str() == "h_grid" => {}
                _ => return Err(invalid("unknown terrain record")),
            }
        }
        terrain.validate()?;
        Ok(terrain)
    }
    /// Encodes an initial snapshot. Save with `ara_doc::write` for canonical text.
    pub fn to_document(&self) -> Result<Document, Diagnostic> {
        self.validate()?;
        let mut doc = Document::new("terrain", self.id.clone());
        let mut put = |id: &str, r| -> Result<(), Diagnostic> {
            doc.insert_unchecked(RecordId::parse(id).map_err(invalid)?, r);
            Ok(())
        };
        put("h_grid", record(&self.header)?)?;
        for (id, l) in &self.layers {
            put(id, record(l)?)?;
        }
        for (id, s) in &self.strokes {
            put(id, record(s)?)?;
        }
        for (id, t) in &self.tiles {
            put(id, record(t)?)?;
        }
        Ok(doc)
    }
    /// Checks references, bounded operations, cache hashes and unique coordinates.
    pub fn validate(&self) -> Result<(), Diagnostic> {
        self.header.validate()?;
        let mut coords = std::collections::BTreeSet::new();
        for l in self.layers.values() {
            if !(0.0..=1.0).contains(&l.opacity) {
                return Err(invalid("invalid layer opacity"));
            }
        }
        for s in self.strokes.values() {
            self.validate_stroke(s)?;
        }
        for t in self.tiles.values() {
            if !self.header.contains(t.coord)
                || !coords.insert(t.coord)
                || t.base
                    .as_ref()
                    .is_some_and(|h| !crate::cache::valid_hash(h))
                || t.strokes.iter().any(|s| !self.strokes.contains_key(s))
            {
                return Err(invalid(
                    "invalid tile coordinate, payload or stroke reference",
                ));
            }
        }
        Ok(())
    }
    pub(crate) fn validate_stroke(&self, s: &Stroke) -> Result<(), Diagnostic> {
        if !self.layers.contains_key(&s.layer)
            || s.dabs.is_empty()
            || s.dabs.len() > 4096
            || s.channel >= 4
            || !s.angle.is_finite()
            || s.dabs.iter().any(|d| {
                !d.center.iter().all(|v| v.is_finite() && v.abs() < 1e8)
                    || !(0.01..=65536.).contains(&d.radius)
                    || !d.amount.is_finite()
                    || d.amount.abs() > 10000.
            })
            || (s.operation == Operation::Erode && !(1..=32).contains(&s.iterations))
            || ([Operation::Stamp, Operation::Import].contains(&s.operation)
                && s.stamp
                    .as_ref()
                    .is_none_or(|h| !crate::cache::valid_hash(h)))
            || (s.operation == Operation::Import
                && s.source_tile.is_none_or(|c| !self.header.contains(c)))
        {
            return Err(invalid("invalid layer, brush, stamp or erosion parameters"));
        }
        Ok(())
    }
}
