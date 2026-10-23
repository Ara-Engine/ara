//! Typed authoring commands. One call, or one enclosing ara-cmd Batch, is one undo item:
//! submit every sample of a pointer stroke together.
use crate::{Camera2d, Flipbook, Light2d, Sprite, Tilemap, tilemap};
use ara_cmd::{Command, EditCtx, Registry, Target};
use ara_diag::Diagnostic;
use ara_schema::{EntityRef, FieldPath, Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

fn invalid(e: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::error(ara_diag::Code(3601), e.to_string())
}
/// Upper bound on cells changed by one stroke, fill or rule pass.
const MAX_CELLS: usize = 65536;

/// A signed source-cell address.
#[derive(Debug, Clone, Copy, Schema, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Cell {
    /// Signed column / axial q.
    pub x: i32,
    /// Signed row / axial r.
    pub y: i32,
}
/// A tile-paint sample in a stroke transaction.
#[derive(Debug, Clone, Copy, Schema, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PaintCell {
    /// Signed column / axial q.
    pub x: i32,
    /// Signed row / axial r.
    pub y: i32,
    /// Tile definition number; zero erases the cell.
    pub tile: u32,
}
/// A stroke of explicitly addressed tile samples.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TilePaintParams {
    /// Tilemap entity, including prefab addresses.
    pub entity: EntityRef,
    /// Stable layer ID.
    pub layer: String,
    /// Stroke samples in input order; later duplicates win.
    pub cells: Vec<PaintCell>,
    /// Re-run terrain rules on the stroke and its neighbors in the same transaction (default false).
    pub autotile: Option<bool>,
}
/// An eraser stroke.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TileEraseParams {
    /// Tilemap entity.
    pub entity: EntityRef,
    /// Stable layer ID.
    pub layer: String,
    /// Cells to erase atomically.
    pub cells: Vec<Cell>,
    /// Re-run terrain rules on the erased cells' neighbors in the same transaction (default false).
    pub autotile: Option<bool>,
}
/// Bounded flood fill of edge-connected cells matching the seed's tile.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TileFillParams {
    /// Tilemap entity.
    pub entity: EntityRef,
    /// Stable layer ID.
    pub layer: String,
    /// Starting cell.
    pub seed: Cell,
    /// Inclusive minimum coordinate; bounds make empty-space fills finite.
    pub min: Cell,
    /// Inclusive maximum coordinate.
    pub max: Cell,
    /// New tile definition, zero to erase.
    pub tile: u32,
    /// Re-run terrain rules on the filled region and its border (default false).
    pub autotile: Option<bool>,
}
/// Applies all terrain rules to a layer in one transaction.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TileRuleParams {
    /// Tilemap entity.
    pub entity: EntityRef,
    /// Stable layer ID.
    pub layer: String,
}
fn load(
    ctx: &EditCtx<'_, '_>,
    entity: &EntityRef,
    layer: &str,
) -> Result<(Target, Tilemap, usize), Diagnostic> {
    let target = ctx.read().target(&entity.0)?;
    let record = ctx
        .read()
        .entity(&target)
        .ok_or_else(|| invalid("missing entity"))?
        .record;
    let value = record
        .get("Tilemap")
        .ok_or_else(|| invalid("entity has no Tilemap"))?;
    let map: Tilemap = serde_json::from_value(value.clone()).map_err(invalid)?;
    map.validate().map_err(invalid)?;
    let index = map
        .layers
        .iter()
        .position(|l| l.id == layer)
        .ok_or_else(|| invalid(format!("unknown tile layer {layer}")))?;
    Ok((target, map, index))
}
/// Applies a stroke (and optional auto-tiling) to a copy, validates it, then emits one
/// cell op per changed cell so disjoint collaborative edits merge. Returns the count.
fn stroke(
    ctx: &mut EditCtx<'_, '_>,
    entity: &EntityRef,
    layer: &str,
    cells: impl IntoIterator<Item = PaintCell>,
    autotile: bool,
) -> Result<u32, Diagnostic> {
    let (target, mut map, index) = load(ctx, entity, layer)?;
    let original = map.layers[index].cells.clone();
    let mut touched = BTreeSet::new();
    for (n, cell) in cells.into_iter().enumerate() {
        if n >= MAX_CELLS {
            return Err(invalid(format!("stroke exceeds {MAX_CELLS} cells")));
        }
        let key = tilemap::key(cell.x, cell.y);
        let cells = &mut map.layers[index].cells;
        if cell.tile == 0 {
            cells.remove(&key);
        } else {
            cells.insert(key, cell.tile);
        }
        touched.insert((cell.x, cell.y));
    }
    map.validate().map_err(invalid)?;
    if autotile {
        let mut region = touched.clone();
        for &(x, y) in &touched {
            for &(dx, dy) in map.layout.neighbors() {
                if let (Some(nx), Some(ny)) = (x.checked_add(dx), y.checked_add(dy)) {
                    region.insert((nx, ny));
                }
            }
        }
        let changes =
            tilemap::apply_rules(&map, &map.layers[index], Some(&region)).map_err(invalid)?;
        map.layers[index].cells.extend(changes);
    }
    let after = &map.layers[index].cells;
    let changed: BTreeMap<&String, Option<u32>> = original
        .keys()
        .chain(after.keys())
        .filter(|k| original.get(*k) != after.get(*k))
        .map(|k| (k, after.get(k).copied()))
        .collect();
    for (key, tile) in &changed {
        let path = FieldPath::field("Tilemap")
            .child("layers")
            .key(layer)
            .child("cells")
            .key(key.as_str());
        ctx.set_entity(&target, &path, tile.map(Value::from))?;
    }
    Ok(changed.len() as u32)
}
/// Paints a bounded stroke; one call or enclosing Batch is one undo item.
pub struct Paint;
impl Command for Paint {
    type Params = TilePaintParams;
    type Output = u32;
    const NAME: &'static str = "tilemap.paint";
    const SUMMARY: &'static str =
        "Paint tile cells as one undoable stroke, optionally auto-tiling terrain.";
    fn examples() -> Vec<Value> {
        vec![
            json!({"entity":"e_map1","layer":"l_ground","cells":[{"x":0,"y":0,"tile":1},{"x":1,"y":0,"tile":1}],"autotile":true}),
        ]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<u32, Diagnostic> {
        stroke(ctx, &p.entity, &p.layer, p.cells, p.autotile == Some(true))
    }
}
/// Erases a stroke through the same invertible cell ops as painting.
pub struct Erase;
impl Command for Erase {
    type Params = TileEraseParams;
    type Output = u32;
    const NAME: &'static str = "tilemap.erase";
    const SUMMARY: &'static str = "Erase tile cells as one undoable stroke.";
    fn examples() -> Vec<Value> {
        vec![json!({"entity":"e_map1","layer":"l_ground","cells":[{"x":0,"y":0}]})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<u32, Diagnostic> {
        let cells = p.cells.into_iter().map(|c| PaintCell {
            x: c.x,
            y: c.y,
            tile: 0,
        });
        stroke(ctx, &p.entity, &p.layer, cells, p.autotile == Some(true))
    }
}
/// Flood fills a bounded edge-connected region (four neighbors on grids, six on hex).
pub struct Fill;
impl Command for Fill {
    type Params = TileFillParams;
    type Output = u32;
    const NAME: &'static str = "tilemap.fill";
    const SUMMARY: &'static str = "Flood fill matching cells within explicit bounds.";
    fn examples() -> Vec<Value> {
        vec![
            json!({"entity":"e_map1","layer":"l_ground","seed":{"x":0,"y":0},"min":{"x":-4,"y":-4},"max":{"x":4,"y":4},"tile":1}),
        ]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<u32, Diagnostic> {
        let (_, map, index) = load(ctx, &p.entity, &p.layer)?;
        let width = i64::from(p.max.x) - i64::from(p.min.x) + 1;
        let height = i64::from(p.max.y) - i64::from(p.min.y) + 1;
        let inside = |x: i32, y: i32| x >= p.min.x && y >= p.min.y && x <= p.max.x && y <= p.max.y;
        if width <= 0
            || height <= 0
            || width * height > MAX_CELLS as i64
            || !inside(p.seed.x, p.seed.y)
        {
            return Err(invalid(format!(
                "fill requires valid bounds of at most {MAX_CELLS} cells containing the seed"
            )));
        }
        let cells = &map.layers[index].cells;
        let at = |x, y| cells.get(&tilemap::key(x, y)).copied().unwrap_or(0);
        let original = at(p.seed.x, p.seed.y);
        let mut queue = VecDeque::from([(p.seed.x, p.seed.y)]);
        let mut seen = BTreeSet::new();
        let mut fill = Vec::new();
        while let Some((x, y)) = queue.pop_front() {
            if !inside(x, y) || !seen.insert((x, y)) || at(x, y) != original {
                continue;
            }
            fill.push(PaintCell { x, y, tile: p.tile });
            for &(dx, dy) in map.layout.adjacent() {
                if let (Some(x), Some(y)) = (x.checked_add(dx), y.checked_add(dy)) {
                    queue.push_back((x, y));
                }
            }
        }
        stroke(ctx, &p.entity, &p.layer, fill, p.autotile == Some(true))
    }
}
/// Applies rule tiles simultaneously, preserving terrain and undo history.
pub struct ApplyRules;
impl Command for ApplyRules {
    type Params = TileRuleParams;
    type Output = u32;
    const NAME: &'static str = "tilemap.rule.apply";
    const SUMMARY: &'static str = "Apply terrain rules to a whole layer in one transaction.";
    fn examples() -> Vec<Value> {
        vec![json!({"entity":"e_map1","layer":"l_ground"})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<u32, Diagnostic> {
        let (_, map, index) = load(ctx, &p.entity, &p.layer)?;
        let mut cells = Vec::new();
        for (key, tile) in tilemap::apply_rules(&map, &map.layers[index], None).map_err(invalid)? {
            let (x, y) = tilemap::coordinate(&key).map_err(invalid)?;
            cells.push(PaintCell { x, y, tile });
        }
        stroke(ctx, &p.entity, &p.layer, cells, false)
    }
}
macro_rules! setter {
    ($params:ident, $command:ident, $ty:ty, $key:literal, $component:literal, $check:expr) => {
        /// Complete typed component authoring update.
        #[derive(Schema, Deserialize)]
        #[ara(params)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        pub struct $params {
            /// Target entity, including prefab addresses.
            pub entity: EntityRef,
            /// Complete typed component; omitted fields use schema defaults.
            pub value: $ty,
        }
        #[doc = concat!("Undoably replaces ", $component, ".")]
        pub struct $command;
        impl Command for $command {
            type Params = $params; type Output = ();
            const NAME: &'static str = $key;
            const SUMMARY: &'static str = concat!("Set ", $component, " through validated ops.");
            fn examples() -> Vec<Value> { vec![json!({"entity":"e_map1","value":{}})] }
            fn run(ctx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<(), Diagnostic> {
                ($check)(&p.value).map_err(invalid)?;
                let target = ctx.read().target(&p.entity.0)?;
                ctx.set_entity(&target, &FieldPath::field($component), Some(crate::to_value(&p.value).map_err(invalid)?))
            }
        }
    }
}
setter!(
    SpriteSetParams,
    SetSprite,
    Sprite,
    "sprite.set",
    "Sprite",
    |v: &Sprite| -> anyhow::Result<()> {
        anyhow::ensure!(
            v.size
                .iter()
                .chain(v.tint.iter())
                .chain(v.parallax.iter())
                .all(|f| f.is_finite())
                && v.size.iter().all(|v| *v >= 0.),
            "invalid sprite geometry/tint"
        );
        Ok(())
    }
);
setter!(
    Camera2dSetParams,
    SetCamera,
    Camera2d,
    "camera2d.set",
    "Camera2d",
    |v: &Camera2d| v.resolved().map(drop)
);
setter!(
    Light2dSetParams,
    SetLight,
    Light2d,
    "light2d.set",
    "Light2d",
    |v: &Light2d| -> anyhow::Result<()> {
        anyhow::ensure!(
            v.position
                .iter()
                .chain(v.color.iter())
                .all(|f| f.is_finite())
                && v.radius.is_finite()
                && v.radius > 0.,
            "invalid light"
        );
        Ok(())
    }
);
setter!(
    FlipbookSetParams,
    SetFlipbook,
    Flipbook,
    "flipbook.set",
    "Flipbook",
    |v: &Flipbook| v.region_at(0).map(drop)
);
setter!(
    TilemapSetParams,
    SetTilemap,
    Tilemap,
    "tilemap.set",
    "Tilemap",
    Tilemap::validate
);

/// Installs commands in the shared Engine Protocol/MCP catalog.
pub fn register(registry: &mut Registry, db: &mut SchemaDb) {
    crate::register(db);
    registry.command::<Paint>(db);
    registry.command::<Erase>(db);
    registry.command::<Fill>(db);
    registry.command::<ApplyRules>(db);
    registry.command::<SetSprite>(db);
    registry.command::<SetCamera>(db);
    registry.command::<SetLight>(db);
    registry.command::<SetFlipbook>(db);
    registry.command::<SetTilemap>(db);
}
