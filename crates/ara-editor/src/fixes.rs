//! Mechanical diagnostic fixes use ordinary typed commands and reversible component ops.
use ara_cmd::{Command, EditCtx, Registry};
use ara_diag::Diagnostic;
use ara_schema::{EntityRef, FieldPath, Schema, SchemaDb, Segment};
use serde::Deserialize;
use serde_json::{Value, json};
/// Reset an authored value or override to its schema/prefab default.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResetParams {
    /// Entity address.
    entity: EntityRef,
    /// Name or component field path.
    path: String,
}
struct Reset;
impl Command for Reset {
    type Params = ResetParams;
    type Output = Value;
    const NAME: &'static str = "editor.property.reset";
    const SUMMARY: &'static str = "Reset a property or prefab override through a reversible op.";
    fn examples() -> Vec<Value> {
        vec![json!({"entity":"e_crate001","path":"Transform.position"})]
    }
    fn run(cx: &mut EditCtx<'_, '_>, p: ResetParams) -> Result<Value, Diagnostic> {
        let target = cx.read().target(&p.entity.0)?;
        let path: FieldPath = p.path.parse().map_err(crate::invalid)?;
        if path.head() != "name" && !ara_doc::is_component_key(path.head()) {
            return Err(crate::invalid(
                "only name and component fields may be reset",
            ));
        }
        cx.read()
            .schema()
            .resolve_path(cx.read().entity_type()?, &path)?;
        cx.set_entity(&target, &path, None)?;
        Ok(json!({"entity":p.entity.0}))
    }
}
/// Rename an obsolete component key to the schema's suggested spelling.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RenameParams {
    /// Entity address.
    entity: EntityRef,
    /// Existing component field path.
    path: String,
    /// Correct final field name.
    to: String,
}
struct Rename;
impl Command for Rename {
    type Params = RenameParams;
    type Output = Value;
    const NAME: &'static str = "editor.property.rename";
    const SUMMARY: &'static str = "Apply a schema spelling fix as one reversible component edit.";
    fn examples() -> Vec<Value> {
        vec![json!({"entity":"e_crate001","path":"Transform.positon","to":"position"})]
    }
    fn run(cx: &mut EditCtx<'_, '_>, p: RenameParams) -> Result<Value, Diagnostic> {
        let target = cx.read().target(&p.entity.0)?;
        let path: FieldPath = p.path.parse().map_err(crate::invalid)?;
        let head = path.head();
        if !ara_doc::is_component_key(head) || path.segments().len() < 2 {
            return Err(crate::invalid("rename fix must address a component field"));
        }
        let entity = cx
            .read()
            .entity(&target)
            .ok_or_else(|| crate::invalid("entity not found"))?;
        let mut component = entity
            .record
            .get(head)
            .cloned()
            .ok_or_else(|| crate::invalid("component not found"))?;
        let mut value = &mut component;
        let parts = &path.segments()[1..];
        for segment in &parts[..parts.len() - 1] {
            let key = match segment {
                Segment::Field(k) | Segment::Key(k) => k,
            };
            value = value
                .get_mut(key)
                .ok_or_else(|| crate::invalid("fix path no longer exists"))?;
        }
        let object = value
            .as_object_mut()
            .ok_or_else(|| crate::invalid("fix target is not an object"))?;
        let last = match &parts[parts.len() - 1] {
            Segment::Field(k) | Segment::Key(k) => k,
        };
        if object.contains_key(&p.to) {
            return Err(crate::invalid(
                "correctly spelled field already exists; resolve the conflict first",
            ));
        }
        let previous = object
            .shift_remove(last)
            .ok_or_else(|| crate::invalid("field no longer exists"))?;
        object.insert(p.to, previous);
        let component_path = FieldPath::field(head);
        cx.read().check_value(&component_path, &component)?;
        cx.set_entity(&target, &component_path, Some(component))?;
        Ok(json!({"entity":p.entity.0}))
    }
}
pub(crate) fn register(registry: &mut Registry, db: &mut SchemaDb) {
    registry.command::<Reset>(db).command::<Rename>(db);
}
