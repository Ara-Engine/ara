//! `Components.g.cs` and `Units.g.cs`: typed C# views of the engine components in
//! the Schema DB. Units, ranges, replication, defaults and docs become attributes and
//! XML docs, so the inspector, agents and IntelliSense read the same metadata.

use std::fmt::Write as _;

use ara_schema::{Audience, FieldDesc, Interpolate, SchemaDb, TypeDesc, Unit, ValueType};
use serde_json::Value;

use super::cs::{HEADER, doc, literal, number, pascal_camel};

/// The C# storage type of a reflected engine value.
pub(super) fn cs_type(db: &SchemaDb, ty: &ValueType) -> Result<String, String> {
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
        ValueType::Entity | ValueType::String | ValueType::Asset(_) => "string?".into(),
        ValueType::Json => "JsonElement".into(),
        ValueType::Option(t) => format!("{}?", cs_type(db, t)?.trim_end_matches('?')),
        ValueType::List(t) | ValueType::Keyed(t) => format!("{}[]", cs_type(db, t)?),
        ValueType::Map(t) => format!("Dictionary<string, {}>", cs_type(db, t)?),
        ValueType::Named(n) if db.get(n).is_some() => n.clone(),
        other => return Err(format!("`{other}` has no generated C# type")),
    })
}

/// A JSON default as C# attribute arguments (`1, 1, 1`, `true`, `"x"`).
fn default_args(v: &Value) -> Option<String> {
    match v {
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n)
            if n.as_u64().is_some_and(|n| n > (1 << 53))
                || n.as_i64().is_some_and(|n| n < -(1 << 53)) =>
        {
            None
        }
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

pub(super) fn field_attributes(db: &SchemaDb, f: &FieldDesc) -> Vec<String> {
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
    if !f.required {
        let default = db.field_default(f);
        let args = if matches!(
            f.ty,
            ValueType::Option(_)
                | ValueType::List(_)
                | ValueType::Keyed(_)
                | ValueType::Map(_)
                | ValueType::Json
        ) {
            None
        } else {
            default_args(&default)
        };
        attrs.push(match args {
            Some(args) => format!("Default({args})"),
            None => format!("Default(Json = {})", literal(&ara_json_text(&default))),
        });
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
pub(super) fn field_remarks(db: &SchemaDb, f: &FieldDesc) -> String {
    let mut parts = Vec::new();
    if let Some(u) = f.unit {
        parts.push(format!("Unit: {}.", u.symbol()));
    }
    if let Some(r) = f.range {
        parts.push(format!("Range: {r}."));
    }
    let default = db.field_default(f);
    if f.required {
        parts.push("Required.".into());
    } else {
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

pub(super) fn write_enum(o: &mut String, t: &TypeDesc) {
    doc(o, "", &t.doc);
    let _ = writeln!(o, "public enum {} : uint\n{{", t.name);
    for (i, v) in t.variants().iter().enumerate() {
        doc(o, "    ", &v.doc);
        let _ = writeln!(o, "    {} = {i},", pascal_camel(&v.name));
    }
    o.push_str("}\n\n");
}

/// Renders every engine component and its reachable value types.
pub fn render(db: &SchemaDb, native: &[&str]) -> Result<String, String> {
    super::values::render(db, native)
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
