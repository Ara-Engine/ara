//! `Components.g.cs` and `Units.g.cs`: blittable C# views of the engine components in
//! the Schema DB. Units, ranges, replication, defaults and docs become attributes and
//! XML docs, so the inspector, agents and IntelliSense read the same metadata.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use ara_schema::{
    Audience, FieldDesc, Interpolate, SchemaDb, Shape, StructLayout, TypeDesc, Unit, ValueType,
};
use serde_json::Value;

use super::cs::{HEADER, doc, doc_tag, literal, number, pascal_camel};

/// The C# type of a blittable schema type.
fn cs_type(db: &SchemaDb, ty: &ValueType) -> Result<String, String> {
    Ok(match ty {
        ValueType::Bool => "bool".into(),
        ValueType::U8 => "byte".into(),
        ValueType::U16 => "ushort".into(),
        ValueType::I32 => "int".into(),
        ValueType::U32 => "uint".into(),
        ValueType::I64 => "long".into(),
        ValueType::U64 => "ulong".into(),
        ValueType::F32 => "float".into(),
        ValueType::F64 => "double".into(),
        ValueType::Vec2 => "Vector2".into(),
        ValueType::Vec3 => "Vector3".into(),
        ValueType::Vec4 => "Vector4".into(),
        ValueType::Quat => "Quaternion".into(),
        ValueType::Color => "Color".into(),
        ValueType::Entity => "Entity".into(),
        ValueType::Named(n) if db.get(n).is_some() => n.clone(),
        other => return Err(format!("`{other}` has no blittable C# view")),
    })
}

/// A JSON default as C# attribute arguments (`1, 1, 1`, `true`, `"x"`).
fn default_args(v: &Value) -> Option<String> {
    match v {
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => n.as_f64().map(number),
        Value::String(s) => Some(literal(s)),
        Value::Array(items) => items
            .iter()
            .map(|i| i.as_f64().map(number))
            .collect::<Option<Vec<_>>>()
            .map(|v| v.join(", ")),
        _ => None,
    }
}

fn field_attributes(f: &FieldDesc) -> Vec<String> {
    let mut attrs = Vec::new();
    if let Some(u) = f.unit {
        attrs.push(format!("Unit({})", literal(u.symbol())));
    }
    if let Some(r) = f.range {
        attrs.push(match (r.min, r.max) {
            (Some(lo), Some(hi)) => format!("Range({}, {})", number(lo), number(hi)),
            (Some(lo), None) => format!("Min({})", number(lo)),
            (None, Some(hi)) => format!("Max({})", number(hi)),
            (None, None) => return attrs,
        });
    }
    let rep = f.replicate;
    if rep != Default::default() {
        let mut args = vec![format!("Audience.{:?}", rep.audience)];
        if let Some(q) = rep.quantize {
            args.push(format!("Quantize = {}", number(q)));
        }
        if rep.interpolate != Interpolate::None {
            args.push(format!("Interpolate = Interpolate.{:?}", rep.interpolate));
        }
        if rep.rollback {
            args.push("Rollback = true".into());
        }
        attrs.push(format!("Replicate({})", args.join(", ")));
    }
    if let Some(args) = f.default.as_ref().and_then(default_args) {
        attrs.push(format!("Default({args})"));
    }
    for alias in &f.aliases {
        attrs.push(format!("FormerlySerializedAs({})", literal(alias)));
    }
    if let Some(msg) = &f.deprecated {
        attrs.push(format!("Obsolete({})", literal(msg)));
    }
    attrs
}

/// The `<remarks>` line: what the attributes say, in words, for tooltips.
fn field_remarks(db: &SchemaDb, f: &FieldDesc) -> String {
    let mut parts = Vec::new();
    if let Some(u) = f.unit {
        parts.push(format!("Unit: {}.", u.symbol()));
    }
    if let Some(r) = f.range {
        parts.push(format!("Range: {r}."));
    }
    let default = db.field_default(f);
    if !default.is_object() {
        parts.push(format!("Default: {}.", ara_json_text(&default)));
    }
    match f.replicate.audience {
        Audience::None => {}
        Audience::Owner => parts.push("Replicated to the owning client.".into()),
        Audience::Relevant => parts.push("Replicated to relevant clients.".into()),
    }
    parts.join(" ")
}

fn ara_json_text(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

fn write_enum(o: &mut String, t: &TypeDesc) {
    doc(o, "", &t.doc);
    let _ = writeln!(o, "public enum {} : uint\n{{", t.name);
    for (i, v) in t.variants().iter().enumerate() {
        doc(o, "    ", &v.doc);
        let _ = writeln!(o, "    {} = {i},", pascal_camel(&v.name));
    }
    o.push_str("}\n\n");
}

/// Writes one blittable struct. `index` is the script component index for engine
/// components, `None` for nested value structs.
fn write_struct(
    o: &mut String,
    db: &SchemaDb,
    t: &TypeDesc,
    layout: &StructLayout,
    index: Option<usize>,
) -> Result<(), String> {
    doc(o, "", &t.doc);
    let _ = writeln!(
        o,
        "[StructLayout(LayoutKind.Explicit, Size = {})]",
        layout.size
    );
    match index {
        Some(_) => {
            let _ = writeln!(
                o,
                "[EngineComponent({}, Module = {})]",
                literal(&t.name),
                literal(&t.module)
            );
            let _ = writeln!(o, "public partial struct {0} : IComponent<{0}>\n{{", t.name);
        }
        None => {
            let _ = writeln!(o, "public partial struct {}\n{{", t.name);
        }
    }
    for (f, fl) in t.fields().iter().zip(&layout.fields) {
        doc(o, "    ", &f.doc);
        doc_tag(o, "    ", "remarks", &field_remarks(db, f));
        let mut attrs = vec![format!("FieldOffset({})", fl.offset)];
        attrs.extend(field_attributes(f));
        let _ = writeln!(
            o,
            "    [{}] public {} {};",
            attrs.join(", "),
            cs_type(db, &f.ty)?,
            pascal_camel(&f.name)
        );
    }
    if let Some(i) = index {
        let _ = writeln!(
            o,
            "\n    /// <summary>Script component index: engine components are numbered in binding order.</summary>\n    public static uint TypeIndex => {i};"
        );
    }
    o.push_str("}\n\n");
    Ok(())
}

/// Named types reachable from `layouts` (nested structs and enums), sorted by name.
fn nested_types(db: &SchemaDb, layouts: &[StructLayout]) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut stack: Vec<String> = layouts
        .iter()
        .flat_map(|l| l.fields.iter().flat_map(|f| f.ty.named()))
        .map(str::to_owned)
        .collect();
    while let Some(n) = stack.pop() {
        if !seen.insert(n.clone()) {
            continue;
        }
        if let Some(t) = db.get(&n) {
            stack.extend(
                t.fields()
                    .iter()
                    .flat_map(|f| f.ty.named())
                    .map(str::to_owned),
            );
        }
    }
    seen
}

/// Renders `Components.g.cs` for the engine components (`layouts`, in binding order).
pub fn render(db: &SchemaDb, layouts: &[StructLayout]) -> Result<String, String> {
    let mut o = String::from(HEADER);
    o.push_str(
        "using System.Numerics;\nusing System.Runtime.InteropServices;\n\nnamespace Ara;\n\n",
    );
    for (i, l) in layouts.iter().enumerate() {
        let t = db
            .get(&l.name)
            .ok_or_else(|| format!("unknown type `{}`", l.name))?;
        write_struct(&mut o, db, t, l, Some(i))?;
    }
    for name in nested_types(db, layouts) {
        let t = db
            .get(&name)
            .ok_or_else(|| format!("unknown type `{name}`"))?;
        match &t.shape {
            Shape::Enum { .. } => write_enum(&mut o, t),
            Shape::Struct { .. } => {
                let l = db.struct_layout(&name)?;
                write_struct(&mut o, db, t, &l, None)?;
            }
        }
    }
    o.push_str("/// <summary>Engine component names in binding order (index = <c>TypeIndex</c>).</summary>\n");
    o.push_str("public static class EngineComponents\n{\n    /// <summary>Schema names, by script component index.</summary>\n    public static readonly string[] Names = [");
    let names: Vec<String> = layouts.iter().map(|l| literal(&l.name)).collect();
    o.push_str(&names.join(", "));
    o.push_str("];\n}\n");
    Ok(o)
}

/// Renders `Units.g.cs`: the unit symbols `[Unit]` accepts.
pub fn render_units() -> String {
    let mut o = String::from(HEADER);
    o.push_str("namespace Ara;\n\n/// <summary>Unit symbols for <see cref=\"UnitAttribute\"/>, as stored in the Schema DB.</summary>\npublic static class Units\n{\n");
    for u in Unit::ALL {
        let _ = writeln!(
            o,
            "    /// <summary><c>{0}</c></summary>\n    public const string {1:?} = {2};",
            super::cs::xml(u.symbol()),
            u,
            literal(u.symbol())
        );
    }
    o.push_str("}\n");
    o
}
