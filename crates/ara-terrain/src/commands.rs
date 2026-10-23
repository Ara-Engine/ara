//! All terrain tools enter through one typed stroke transaction per gesture.
use crate::{
    Dab, Header, Layer, Operation, Stroke, Terrain, TileCoord, TileRecord, invalid,
    model::{record, value},
};
use ara_cmd::{Command, EditCtx, Registry};
use ara_diag::Diagnostic;
use ara_doc::RecordId;
use ara_schema::{FieldPath, Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};

/// Grid creation parameters for an empty terrain document.
#[derive(Schema, Deserialize)]
#[ara(params, name = "TerrainCreateParams", module = "terrain")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateParams {
    /// Terrain grid dimensions and material channels.
    pub grid: Header,
}
/// A non-destructive layer to insert.
#[derive(Schema, Deserialize)]
#[ara(params, name = "TerrainLayerParams", module = "terrain")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LayerParams {
    /// Optional concrete or symbolic layer ID.
    pub id: Option<String>,
    /// Layer settings.
    pub layer: Layer,
}
/// A whole gesture, including every dab; do not issue one transaction per mouse sample.
#[derive(Schema, Deserialize)]
#[ara(params, name = "TerrainStrokeParams", module = "terrain")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StrokeParams {
    /// Existing layer ID.
    pub layer: String,
    /// Ordered gesture samples.
    pub dabs: Vec<Dab>,
    /// Material channel for paint; defaults to zero.
    pub channel: Option<u32>,
    /// Thermal iterations, default eight.
    pub iterations: Option<u32>,
    /// Immutable height stamp hash.
    pub stamp: Option<String>,
    /// Stamp rotation in radians.
    pub angle: Option<f32>,
}
/// Change layer settings while retaining its gesture history.
#[derive(Schema, Deserialize)]
#[ara(params, name = "TerrainLayerSetParams", module = "terrain")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LayerSetParams {
    /// Existing layer record ID.
    pub id: String,
    /// Replacement layer controls.
    pub layer: Layer,
}
/// Imports one immutable source tile, preserving subsequent layer edits.
#[derive(Schema, Deserialize)]
#[ara(params, name = "TerrainImportParams", module = "terrain")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportParams {
    /// Existing non-destructive layer receiving this source tile.
    pub layer: String,
    /// Destination tile address.
    pub coord: TileCoord,
    /// Verified source payload key from the terrain cache.
    pub hash: String,
}

/// Registers commands for the shared CLI/MCP/editor registry.
pub fn register(reg: &mut Registry, db: &mut SchemaDb) {
    reg.command::<Create>(db)
        .command::<AddLayer>(db)
        .command::<SetLayer>(db)
        .command::<Raise>(db)
        .command::<Paint>(db)
        .command::<Hole>(db)
        .command::<Stamp>(db)
        .command::<Erode>(db)
        .command::<Import>(db);
}

struct Create;
impl Command for Create {
    type Params = CreateParams;
    type Output = Value;
    const NAME: &'static str = "terrain.create";
    const SUMMARY: &'static str = "Create a tiled terrain grid in an empty terrain document.";
    fn examples() -> Vec<Value> {
        vec![
            json!({"grid":{"origin":[0,0],"spacing":1,"cells":64,"min":{"x":0,"z":0},"max":{"x":64,"z":64},"materials":[]}}),
        ]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: CreateParams) -> Result<Value, Diagnostic> {
        p.grid.validate()?;
        if ctx.read().doc().kind != "terrain" || !ctx.read().doc().records().is_empty() {
            return Err(invalid("terrain.create requires an empty terrain document"));
        }
        let id = ctx.new_id(Some("h_grid"), "h")?;
        ctx.create(id, record(&p.grid)?)?;
        Ok(json!({"id":"h_grid"}))
    }
}
struct AddLayer;
impl Command for AddLayer {
    type Params = LayerParams;
    type Output = Value;
    const NAME: &'static str = "terrain.layer.add";
    const SUMMARY: &'static str = "Add an ordered non-destructive terrain layer.";
    fn examples() -> Vec<Value> {
        vec![json!({"layer":{"name":"Sculpt","priority":0,"enabled":true,"opacity":1}})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: LayerParams) -> Result<Value, Diagnostic> {
        Terrain::from_document(ctx.read().doc())?;
        if !(0.0..=1.0).contains(&p.layer.opacity) {
            return Err(invalid("invalid opacity"));
        }
        let id = ctx.new_id(p.id.as_deref(), "l")?;
        ctx.create(id.clone(), record(&p.layer)?)?;
        Ok(json!({"id":id}))
    }
}
struct SetLayer;
impl Command for SetLayer {
    type Params = LayerSetParams;
    type Output = Value;
    const NAME: &'static str = "terrain.layer.set";
    const SUMMARY: &'static str =
        "Change opacity, visibility or ordering without destroying strokes.";
    fn examples() -> Vec<Value> {
        vec![
            json!({"id":"l_base","layer":{"name":"Base","priority":0,"enabled":false,"opacity":1}}),
        ]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: LayerSetParams) -> Result<Value, Diagnostic> {
        let t = Terrain::from_document(ctx.read().doc())?;
        if !t.layers.contains_key(&p.id) || !(0.0..=1.0).contains(&p.layer.opacity) {
            return Err(invalid("unknown layer or invalid opacity"));
        }
        let id = RecordId::parse(&p.id).map_err(invalid)?;
        for (key, v) in record(&p.layer)? {
            ctx.set(&id, FieldPath::field(key), Some(v))?;
        }
        Ok(json!({"id":id}))
    }
}

fn apply(
    ctx: &mut EditCtx<'_, '_>,
    p: StrokeParams,
    operation: Operation,
    source_tile: Option<TileCoord>,
) -> Result<Value, Diagnostic> {
    let mut t = Terrain::from_document(ctx.read().doc())?;
    let sequence = t
        .strokes
        .values()
        .map(|s| s.sequence)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| invalid("stroke sequence exhausted"))?;
    let s = Stroke {
        layer: p.layer,
        sequence,
        operation,
        dabs: p.dabs,
        channel: p.channel.unwrap_or(0),
        iterations: p.iterations.unwrap_or(8),
        stamp: p.stamp,
        source_tile,
        angle: p.angle.unwrap_or(0.),
    };
    t.validate_stroke(&s)?;
    let width = t.header.tile_width();
    let mut touched = std::collections::BTreeSet::new();
    // Conservative one-tile/erosion halo also invalidates collision, nav and GI borders.
    let halo = t
        .strokes
        .values()
        .filter(|s| s.operation == Operation::Erode)
        .map(|s| s.iterations as f32 * s.dabs.len() as f32 * t.header.spacing)
        .sum::<f32>()
        + if operation == Operation::Erode {
            s.iterations as f32 * t.header.spacing
        } else {
            0.
        };
    for dab in &s.dabs {
        let radius = dab.radius + halo;
        let lo = TileCoord {
            x: ((dab.center[0] - radius - t.header.origin[0]) / width).floor() as i32,
            z: ((dab.center[1] - radius - t.header.origin[1]) / width).floor() as i32,
        };
        let hi = TileCoord {
            x: ((dab.center[0] + radius - t.header.origin[0]) / width).floor() as i32,
            z: ((dab.center[1] + radius - t.header.origin[1]) / width).floor() as i32,
        };
        for z in lo.z.max(t.header.min.z)..=hi.z.min(t.header.max.z - 1) {
            for x in lo.x.max(t.header.min.x)..=hi.x.min(t.header.max.x - 1) {
                touched.insert(TileCoord { x, z });
                if touched.len() > 4096 {
                    return Err(invalid(
                        "stroke touches more than 4096 tiles; split into bounded gestures",
                    ));
                }
            }
        }
    }
    if touched.is_empty() {
        return Err(invalid("brush does not touch terrain"));
    }
    let id = ctx.new_id(None, "s")?;
    ctx.create(id.clone(), record(&s)?)?;
    for coord in &touched {
        if let Some((tid, tile)) = t.tiles.iter_mut().find(|(_, tile)| tile.coord == *coord) {
            tile.strokes.push(id.to_string());
            ctx.set(
                &RecordId::parse(tid).map_err(invalid)?,
                FieldPath::field("strokes"),
                Some(value(&tile.strokes)?),
            )?;
        } else {
            let tid = ctx.new_id(None, "t")?;
            let tile = TileRecord {
                coord: *coord,
                base: None,
                strokes: vec![id.to_string()],
            };
            ctx.create(tid, record(&tile)?)?;
        }
    }
    Ok(json!({"stroke":id,"dirtyTiles":touched.into_iter().collect::<Vec<_>>()}))
}
macro_rules! stroke_command {
    ($ty:ident,$name:literal,$doc:literal,$op:ident)=>{
        struct $ty;
        impl Command for $ty{
            type Params=StrokeParams;type Output=Value;
            const NAME:&'static str=$name;const SUMMARY:&'static str=$doc;
            fn examples()->Vec<Value>{vec![json!({"layer":"l_base","dabs":[{"center":[0,0],"radius":8,"amount":1}]})]}
            fn run(ctx:&mut EditCtx<'_,'_>,p:StrokeParams)->Result<Value,Diagnostic>{apply(ctx,p,Operation::$op,None)}
        }
    }
}
stroke_command!(
    Raise,
    "terrain.raise",
    "Sculpt a radial brush gesture in metres.",
    Raise
);
stroke_command!(
    Paint,
    "terrain.paint",
    "Paint normalized material weights for a complete gesture.",
    Paint
);
stroke_command!(
    Hole,
    "terrain.hole",
    "Cut or restore hole cells for a complete gesture.",
    Hole
);
stroke_command!(
    Stamp,
    "terrain.stamp",
    "Apply a rotated immutable height stamp.",
    Stamp
);
stroke_command!(
    Erode,
    "terrain.erode",
    "Apply bounded deterministic thermal erosion.",
    Erode
);

struct Import;
impl Command for Import {
    type Params = ImportParams;
    type Output = Value;
    const NAME: &'static str = "terrain.import";
    const SUMMARY: &'static str =
        "Reference a binary source heightfield without rewriting it into text.";
    fn examples() -> Vec<Value> {
        vec![json!({"layer":"l_base","coord":{"x":0,"z":0},"hash":"0".repeat(64)})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: ImportParams) -> Result<Value, Diagnostic> {
        let t = Terrain::from_document(ctx.read().doc())?;
        if !t.header.contains(p.coord) || !crate::cache::valid_hash(&p.hash) {
            return Err(invalid("invalid import coordinate or hash"));
        }
        let width = t.header.tile_width();
        let center = [
            t.header.origin[0] + (p.coord.x as f32 + 0.5) * width,
            t.header.origin[1] + (p.coord.z as f32 + 0.5) * width,
        ];
        apply(
            ctx,
            StrokeParams {
                layer: p.layer,
                dabs: vec![Dab {
                    center,
                    radius: width,
                    amount: 1.,
                }],
                channel: None,
                iterations: None,
                stamp: Some(p.hash),
                angle: None,
            },
            Operation::Import,
            Some(p.coord),
        )
    }
}
