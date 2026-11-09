//! Typed vertex-paint authoring: every edit replaces the entity's `VertexPaint`
//! component through one op, so a stroke is one undoable step.
use super::{PaintedPrimitive, VertexPaint};
use ara_cmd::{Command, EditCtx, Registry};
use ara_diag::{Diagnostic, codes};
use ara_schema::{Color, EntityRef, FieldPath, Schema, SchemaDb};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Registers `vertexPaint.set`, `vertexPaint.fill` and `vertexPaint.clear`.
pub fn register(registry: &mut Registry, db: &mut SchemaDb) {
    db.register::<PaintChannels>();
    registry
        .command::<VertexPaintSet>(db)
        .command::<VertexPaintFill>(db)
        .command::<VertexPaintClear>(db);
}

/// What a vertex-paint command left on the entity.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Painted {
    /// Entity address.
    pub entity: String,
    /// Painted slots after the edit.
    pub primitives: Vec<u32>,
}

/// Which RGBA channels a fill writes (Unreal's paint channel mask).
#[derive(Debug, Clone, Copy, Schema, Deserialize)]
#[ara(module = "render")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PaintChannels {
    /// Write red.
    #[ara(default = true)]
    #[serde(default = "yes")]
    pub r: bool,
    /// Write green.
    #[ara(default = true)]
    #[serde(default = "yes")]
    pub g: bool,
    /// Write blue.
    #[ara(default = true)]
    #[serde(default = "yes")]
    pub b: bool,
    /// Write alpha.
    #[ara(default = true)]
    #[serde(default = "yes")]
    pub a: bool,
}

fn yes() -> bool {
    true
}

impl Default for PaintChannels {
    fn default() -> Self {
        Self {
            r: true,
            g: true,
            b: true,
            a: true,
        }
    }
}

impl PaintChannels {
    /// The channel mask as an array, R first.
    pub fn mask(self) -> [bool; 4] {
        [self.r, self.g, self.b, self.a]
    }
}

/// Parameters of `vertexPaint.set`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params, module = "render")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VertexPaintSetParams {
    /// Painted entity (its MeshRenderer); may be inside a prefab instance.
    pub entity: EntityRef,
    /// Subtree-local primitive slot.
    #[ara(default = 0)]
    #[serde(default)]
    pub primitive: u32,
    /// Source vertex count the colours cover.
    pub vertices: u32,
    /// Linear RGBA8 per source vertex, zlib-compressed and base64 (`VertexPaint` format).
    pub colors: String,
}

/// Parameters of `vertexPaint.fill`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params, module = "render")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VertexPaintFillParams {
    /// Painted entity (its MeshRenderer).
    pub entity: EntityRef,
    /// Subtree-local primitive slot.
    #[ara(default = 0)]
    #[serde(default)]
    pub primitive: u32,
    /// Linear colour written to every vertex's selected channels.
    pub color: Color,
    /// Channels written (absent: all); the others keep their painted (or white) values.
    pub channels: Option<PaintChannels>,
    /// Source vertex count; required when the slot has no paint yet (the editor and
    /// `ara inspect` of the model report it).
    pub vertices: Option<u32>,
}

/// Parameters of `vertexPaint.clear`.
#[derive(Debug, Schema, Deserialize)]
#[ara(params, module = "render")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VertexPaintClearParams {
    /// Painted entity.
    pub entity: EntityRef,
    /// Slot to clear; absent removes the whole `VertexPaint` component.
    pub primitive: Option<u32>,
}

fn invalid(message: impl Into<String>) -> Diagnostic {
    Diagnostic::error(codes::INVALID_PARAMS, message.into())
}

/// The entity's current paint (empty without the component).
fn current(
    ctx: &EditCtx<'_, '_>,
    entity: &str,
) -> Result<(ara_cmd::Target, VertexPaint), Diagnostic> {
    let read = ctx.read();
    let target = read.target(entity)?;
    let record = read
        .entity(&target)
        .ok_or_else(|| invalid("missing target entity"))?;
    if !record.record.contains_key("MeshRenderer") {
        return Err(invalid(format!(
            "`{}` has no MeshRenderer to paint",
            target.address()
        )));
    }
    let paint = record
        .record
        .get("VertexPaint")
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
        .map_err(|e| invalid(e.to_string()))?
        .unwrap_or_default();
    Ok((target, paint))
}

fn store(
    ctx: &mut EditCtx<'_, '_>,
    target: &ara_cmd::Target,
    paint: &VertexPaint,
) -> Result<Painted, Diagnostic> {
    let value = (!paint.primitives.is_empty())
        .then(|| ara_json::to_value(paint))
        .transpose()
        .map_err(|e| invalid(e.to_string()))?;
    ctx.set_entity(target, &FieldPath::field("VertexPaint"), value)?;
    Ok(Painted {
        entity: target.address(),
        primitives: paint.primitives.iter().map(|p| p.primitive).collect(),
    })
}

/// `vertexPaint.set`: replace one primitive's painted colours.
pub struct VertexPaintSet;
impl Command for VertexPaintSet {
    type Params = VertexPaintSetParams;
    type Output = Painted;
    const NAME: &'static str = "vertexPaint.set";
    const SUMMARY: &'static str =
        "Replace one primitive's per-instance vertex colours (a whole paint stroke).";
    fn examples() -> Vec<Value> {
        vec![
            json!({"entity":"e_floor","primitive":0,"vertices":3,"colors":super::encode(&[[255,0,0,255];3])}),
        ]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<Painted, Diagnostic> {
        let (target, mut paint) = current(ctx, &p.entity.0)?;
        let painted = PaintedPrimitive {
            primitive: p.primitive,
            vertices: p.vertices,
            colors: p.colors,
        };
        painted
            .decode()
            .map_err(|e| invalid(e).path("params.colors"))?;
        paint.set(painted);
        store(ctx, &target, &paint)
    }
}

/// `vertexPaint.fill`: set selected channels of every vertex (Unreal's Fill).
pub struct VertexPaintFill;
impl Command for VertexPaintFill {
    type Params = VertexPaintFillParams;
    type Output = Painted;
    const NAME: &'static str = "vertexPaint.fill";
    const SUMMARY: &'static str =
        "Fill selected colour channels of one primitive's per-instance vertex colours.";
    fn examples() -> Vec<Value> {
        vec![
            json!({"entity":"e_floor","color":[0,0,0,1],"vertices":4096}),
            json!({"entity":"e_floor","primitive":1,"color":[1,0,0,1],"channels":{"g":false,"b":false,"a":false}}),
        ]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<Painted, Diagnostic> {
        let (target, mut paint) = current(ctx, &p.entity.0)?;
        if p.color.0.iter().any(|c| !c.is_finite()) {
            return Err(invalid("fill colour must be finite").path("params.color"));
        }
        let mut colors = match (paint.get(p.primitive), p.vertices) {
            (Some(existing), vertices) => {
                let colors = existing.decode().map_err(invalid)?;
                if vertices.is_some_and(|v| v as usize != colors.len()) {
                    return Err(invalid(format!(
                        "slot {} is painted for {} vertices; clear it before refilling with {}",
                        p.primitive,
                        colors.len(),
                        vertices.unwrap_or_default()
                    )));
                }
                colors
            }
            (None, Some(vertices)) => vec![[255; 4]; vertices as usize],
            (None, None) => {
                return Err(invalid(format!(
                    "slot {} has no paint yet: pass `vertices`, its source vertex count",
                    p.primitive
                ))
                .path("params.vertices"));
            }
        };
        let byte = p.color.0.map(|c| (c.clamp(0., 1.) * 255. + 0.5) as u8);
        let mask = p.channels.unwrap_or_default().mask();
        for color in &mut colors {
            for c in 0..4 {
                if mask[c] {
                    color[c] = byte[c];
                }
            }
        }
        paint.set(PaintedPrimitive::new(p.primitive, &colors));
        store(ctx, &target, &paint)
    }
}

/// `vertexPaint.clear`: remove one slot's paint, or the whole component.
pub struct VertexPaintClear;
impl Command for VertexPaintClear {
    type Params = VertexPaintClearParams;
    type Output = Painted;
    const NAME: &'static str = "vertexPaint.clear";
    const SUMMARY: &'static str =
        "Remove per-instance vertex paint from one primitive slot or the whole entity.";
    fn examples() -> Vec<Value> {
        vec![
            json!({"entity":"e_floor"}),
            json!({"entity":"e_floor","primitive":2}),
        ]
    }
    fn run(ctx: &mut EditCtx<'_, '_>, p: Self::Params) -> Result<Painted, Diagnostic> {
        let (target, mut paint) = current(ctx, &p.entity.0)?;
        match p.primitive {
            Some(slot) => paint.primitives.retain(|painted| painted.primitive != slot),
            None => paint.primitives.clear(),
        }
        store(ctx, &target, &paint)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{VertexPaint, decode, encode};
    use ara_cmd::{Batch, Registry, Session};
    use ara_doc::{AssetId, Document, Workspace};
    use ara_ops::{Author, Store};
    use serde_json::json;

    fn paint(store: &Store) -> Option<VertexPaint> {
        let doc = store.workspace().doc("sc_test").expect("scene");
        doc.records()
            .get("e_floor")
            .and_then(|r| r.get("VertexPaint"))
            .map(|v| serde_json::from_value(v.clone()).expect("valid paint"))
    }

    #[test]
    fn fills_sets_and_clears_are_validated_undoable_ops() {
        let mut schema = crate::schema(&[]).expect("schema");
        let mut registry = Registry::builtin(&mut schema);
        super::register(&mut registry, &mut schema);
        let mut workspace = Workspace::default();
        workspace.insert(
            "test.scene",
            Document::new("scene", AssetId::parse("sc_test").expect("id")),
        );
        let mut store = Store::new(schema.clone(), workspace);
        let human = Session::human();
        let apply = |store: &mut Store, cmd: &'static str, args: serde_json::Value| {
            registry
                .apply(store, &Batch::new("sc_test", [(cmd, args)]), &human)
                .map(|_| ())
        };
        apply(
            &mut store,
            "entity.create",
            json!({"id":"e_floor","components":{"Transform":{},"MeshRenderer":{}}}),
        )
        .expect("entity");
        let missing = apply(
            &mut store,
            "vertexPaint.fill",
            json!({"entity":"e_floor","color":[0,0,0,1]}),
        )
        .expect_err("a first fill needs the vertex count");
        assert!(
            missing.iter().any(|d| d.message.contains("vertices")),
            "{missing:?}"
        );
        apply(
            &mut store,
            "vertexPaint.fill",
            json!({"entity":"e_floor","color":[1,0,0,1],"vertices":4}),
        )
        .expect("fill");
        // Only the red channel: green, blue and alpha keep their values.
        apply(
            &mut store,
            "vertexPaint.fill",
            json!({"entity":"e_floor","color":[0,1,1,0],"channels":{"r":false}}),
        )
        .expect("masked fill");
        let colors = paint(&store)
            .expect("painted")
            .get(0)
            .expect("slot 0")
            .decode()
            .expect("valid");
        assert_eq!(colors, vec![[255, 255, 255, 0]; 4]);
        let bad = apply(
            &mut store,
            "vertexPaint.set",
            json!({"entity":"e_floor","vertices":5,"colors":encode(&[[0; 4]; 4])}),
        )
        .expect_err("count mismatch");
        assert!(bad.iter().any(|d| d.message.contains("4 colours for 5")));
        apply(
            &mut store,
            "vertexPaint.set",
            json!({"entity":"e_floor","primitive":2,"vertices":2,"colors":encode(&[[9; 4]; 2])}),
        )
        .expect("set");
        assert_eq!(paint(&store).expect("painted").primitives.len(), 2);
        store.undo_last(&Author::Human).expect("undo set");
        assert_eq!(paint(&store).expect("painted").primitives.len(), 1);
        apply(&mut store, "vertexPaint.clear", json!({"entity":"e_floor"})).expect("clear");
        assert!(
            paint(&store).is_none(),
            "clearing every slot removes the component"
        );
        assert_eq!(decode(&encode(&[])).expect("empty"), Vec::<[u8; 4]>::new());
    }
}
