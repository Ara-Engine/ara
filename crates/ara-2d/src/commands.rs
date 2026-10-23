//! Typed authoring commands. Submit all samples of a stroke in one ara-cmd Batch.
use crate::{Camera2d, Flipbook, Light2d, Sprite, Tile, Tilemap, tilemap};
use ara_cmd::{Command, EditCtx, Registry};
use ara_diag::Diagnostic;
use ara_schema::{EntityRef, FieldPath, Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeSet, VecDeque};

fn invalid(e: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::error(ara_diag::Code(3601), e.to_string())
}

/// A signed source-cell address.
#[derive(Debug, Schema, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Cell {
    /// Signed column / axial q.
    pub x: i32,
    /// Signed row / axial r.
    pub y: i32,
}
/// A tile-paint sample in a stroke transaction.
#[derive(Debug, Schema, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PaintCell {
    /// Signed column / axial q.
    pub x: i32,
    /// Signed row / axial r.
    pub y: i32,
    /// Occupied tile definition; zero erases the cell.
    pub tile: u32,
    /// Semantic auto-tile terrain group.
    pub terrain: u32,
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
}
/// Bounded flood fill of cells matching the seed's tile and terrain.
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
    /// New semantic terrain group.
    pub terrain: u32,
}
/// Applies all terrain rules in one transaction.
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
) -> Result<(ara_cmd::Target, Tilemap, usize), Diagnostic> {
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
        .ok_or_else(|| invalid("unknown tile layer"))?;
    Ok((target, map, index))
}
fn write(
    ctx: &mut EditCtx<'_, '_>,
    target: &ara_cmd::Target,
    layer: &str,
    cells: impl IntoIterator<Item = (String, Tile)>,
) -> Result<u32, Diagnostic> {
    let mut count = 0;
    for (key, tile) in cells {
        let path = FieldPath::field("Tilemap")
            .child("layers")
            .key(layer)
            .child("cells")
            .key(key);
        ctx.set_entity(
            target,
            &path,
            if tile.tile == 0 {
                None
            } else {
                Some(serde_json::to_value(tile).map_err(invalid)?)
            },
        )?;
        count += 1;
    }
    Ok(count)
}
/// Paints a bounded stroke; one call or enclosing Batch is one undo item.
pub struct Paint;
impl Command for Paint {
    type Params = TilePaintParams;
    type Output = u32;
    const NAME: &'static str = "tilemap.paint";
    const SUMMARY: &'static str = "Paint tile cells as one undoable stroke.";
    fn examples() -> Vec<Value> {
        vec![
            json!({"entity":"e_map1","layer":"l_ground","cells":[{"x":0,"y":0,"tile":1,"terrain":1}]}),
        ]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<u32, Diagnostic> {
        if p.cells.len() > 65536 {
            return Err(invalid("stroke exceeds 65536 cells"));
        }
        let (target, mut map, index) = load(ctx, &p.entity, &p.layer)?;
        let mut changes = std::collections::BTreeMap::new();
        for cell in p.cells {
            let key = tilemap::key(cell.x, cell.y);
            let tile = Tile {
                tile: cell.tile,
                terrain: cell.terrain,
            };
            if tile.tile == 0 {
                map.layers[index].cells.remove(&key);
            } else {
                map.layers[index].cells.insert(key.clone(), tile);
            }
            changes.insert(key, tile);
        }
        map.validate().map_err(invalid)?;
        write(ctx, &target, &p.layer, changes)
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
        Paint::run(
            ctx,
            TilePaintParams {
                entity: p.entity,
                layer: p.layer,
                cells: p
                    .cells
                    .into_iter()
                    .map(|c| PaintCell {
                        x: c.x,
                        y: c.y,
                        tile: 0,
                        terrain: 0,
                    })
                    .collect(),
            },
        )
    }
}
/// Flood fills a bounded connected region, with grid/hex adjacency.
pub struct Fill;
impl Command for Fill {
    type Params = TileFillParams;
    type Output = u32;
    const NAME: &'static str = "tilemap.fill";
    const SUMMARY: &'static str = "Flood fill matching cells within explicit bounds.";
    fn examples() -> Vec<Value> {
        vec![
            json!({"entity":"e_map1","layer":"l_ground","seed":{"x":0,"y":0},"min":{"x":-4,"y":-4},"max":{"x":4,"y":4},"tile":1,"terrain":1}),
        ]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<u32, Diagnostic> {
        let (_, map, index) = load(ctx, &p.entity, &p.layer)?;
        let width = i64::from(p.max.x) - i64::from(p.min.x) + 1;
        let height = i64::from(p.max.y) - i64::from(p.min.y) + 1;
        if width <= 0
            || height <= 0
            || width > 65536
            || height > 65536
            || width * height > 65536
            || p.seed.x < p.min.x
            || p.seed.y < p.min.y
            || p.seed.x > p.max.x
            || p.seed.y > p.max.y
        {
            return Err(invalid(
                "fill requires valid bounds of at most 65536 cells containing the seed",
            ));
        }
        let layer = &map.layers[index];
        let original = layer
            .cells
            .get(&tilemap::key(p.seed.x, p.seed.y))
            .copied()
            .unwrap_or_default();
        let mut queue = VecDeque::from([(p.seed.x, p.seed.y)]);
        let mut seen = BTreeSet::new();
        let mut cells = Vec::new();
        while let Some((x, y)) = queue.pop_front() {
            if x < p.min.x
                || y < p.min.y
                || x > p.max.x
                || y > p.max.y
                || !seen.insert((x, y))
                || layer
                    .cells
                    .get(&tilemap::key(x, y))
                    .copied()
                    .unwrap_or_default()
                    != original
            {
                continue;
            }
            cells.push(PaintCell {
                x,
                y,
                tile: p.tile,
                terrain: p.terrain,
            });
            for &(dx, dy) in map.layout.neighbors() {
                if let (Some(x), Some(y)) = (x.checked_add(dx), y.checked_add(dy)) {
                    queue.push_back((x, y));
                }
            }
        }
        Paint::run(
            ctx,
            TilePaintParams {
                entity: p.entity,
                layer: p.layer,
                cells,
            },
        )
    }
}
/// Applies rule tiles simultaneously, preserving terrain and undo history.
pub struct ApplyRules;
impl Command for ApplyRules {
    type Params = TileRuleParams;
    type Output = u32;
    const NAME: &'static str = "tilemap.rule.apply";
    const SUMMARY: &'static str = "Apply terrain rules to a layer in one transaction.";
    fn examples() -> Vec<Value> {
        vec![json!({"entity":"e_map1","layer":"l_ground"})]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<u32, Diagnostic> {
        let (target, map, index) = load(ctx, &p.entity, &p.layer)?;
        let changes = tilemap::apply_rules(&map, &map.layers[index]).map_err(invalid)?;
        write(ctx, &target, &p.layer, changes)
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
                ctx.set_entity(&target, &FieldPath::field($component), Some(serde_json::to_value(p.value).map_err(invalid)?))
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
