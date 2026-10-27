//! Typed, reflection-free JSON codecs for every generated engine value.
use super::{
    components::{cs_type, field_attributes, field_remarks, write_enum},
    cs::{HEADER, doc, doc_tag, literal, pascal_camel},
};
use ara_schema::{SchemaDb, Shape, TypeDesc, ValueType};
use std::{collections::BTreeSet, fmt::Write as _};

fn member(t: &TypeDesc, field: &str) -> String {
    let name = pascal_camel(field);
    if name == t.name {
        format!("{name}Asset")
    } else {
        name
    }
}

fn managed(db: &SchemaDb, ty: &ValueType) -> bool {
    match ty {
        ValueType::Named(n) => db
            .get(n)
            .is_some_and(|t| t.fields().iter().any(|f| managed(db, &f.ty))),
        ValueType::String
        | ValueType::Asset(_)
        | ValueType::Entity
        | ValueType::Json
        | ValueType::Option(_)
        | ValueType::List(_)
        | ValueType::Keyed(_)
        | ValueType::Map(_) => true,
        _ => false,
    }
}

fn read(db: &SchemaDb, ty: &ValueType, v: &str, depth: usize) -> Result<String, String> {
    use ValueType::*;
    Ok(match ty {
        Bool => format!("{v}.GetBoolean()"),
        U8 => format!("{v}.GetByte()"),
        U16 => format!("{v}.GetUInt16()"),
        I32 => format!("{v}.GetInt32()"),
        U32 => format!("{v}.GetUInt32()"),
        I64 => format!("{v}.GetInt64()"),
        U64 => format!("{v}.GetUInt64()"),
        F32 => format!("{v}.GetSingle()"),
        F64 => format!("{v}.GetDouble()"),
        String | Asset(_) | Entity => {
            format!("{v}.ValueKind == JsonValueKind.Null ? null : {v}.GetString()")
        }
        Json => format!("{v}.Clone()"),
        Vec2 | Vec3 | Vec4 | Quat | Color => {
            let n = ty.vector_len().expect("vector type");
            format!(
                "new {}({})",
                cs_type(db, ty)?,
                (0..n)
                    .map(|i| format!("{v}[{i}].GetSingle()"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        Named(n) => format!("Read{n}({v})"),
        Option(t) => format!(
            "{v}.ValueKind == JsonValueKind.Null ? null : ({})({})",
            cs_type(db, ty)?,
            read(db, t, v, depth + 1)?
        ),
        List(t) | Keyed(t) => {
            let item = format!("item{depth}");
            format!(
                "{v}.EnumerateArray().Select(static {item} => {}).ToArray()",
                read(db, t, &item, depth + 1)?
            )
        }
        Map(t) => {
            let item = format!("item{depth}");
            format!(
                "{v}.EnumerateObject().ToDictionary(static {item} => {item}.Name, static {item} => {})",
                read(db, t, &format!("{item}.Value"), depth + 1)?
            )
        }
    })
}

fn write_value(
    out: &mut String,
    db: &SchemaDb,
    ty: &ValueType,
    v: &str,
    depth: usize,
) -> Result<(), String> {
    use ValueType::*;
    let _ = db;
    match ty {
        Bool => {
            let _ = writeln!(out, "        w.WriteBooleanValue({v});");
        }
        U8 | U16 | I32 | U32 | I64 | U64 => {
            let _ = writeln!(out, "        w.WriteNumberValue({v});");
        }
        F32 | F64 => {
            let _ = writeln!(out, "        WriteFloat(w, {v});");
        }
        String | Asset(_) | Entity => {
            let _ = writeln!(out, "        w.WriteStringValue({v});");
        }
        Json => {
            let _ = writeln!(
                out,
                "        if ({v}.ValueKind == JsonValueKind.Undefined) w.WriteNullValue(); else {v}.WriteTo(w);"
            );
        }
        Vec2 | Vec3 | Vec4 | Quat | Color => {
            out.push_str("        w.WriteStartArray();\n");
            let fields: &[&str] = if *ty == Color {
                &["R", "G", "B", "A"]
            } else {
                &["X", "Y", "Z", "W"][..ty.vector_len().expect("vector")]
            };
            for f in fields {
                let _ = writeln!(out, "        WriteFloat(w, {v}.{f});");
            }
            out.push_str("        w.WriteEndArray();\n");
        }
        Named(n) => {
            let _ = writeln!(out, "        Write{n}(w, {v});");
        }
        Option(t) => {
            let _ = writeln!(out, "        if ({v} is {{ }} option{depth})\n        {{");
            write_value(out, db, t, &format!("option{depth}"), depth + 1)?;
            out.push_str("        } else w.WriteNullValue();\n");
        }
        List(t) | Keyed(t) | Map(t) => {
            let map = matches!(ty, Map(_));
            let _ = writeln!(
                out,
                "        w.WriteStart{}();\n        if ({v} is not null) foreach (var item{depth} in {v}{})\n        {{",
                if map { "Object" } else { "Array" },
                if map {
                    ".OrderBy(static p => p.Key, StringComparer.Ordinal)"
                } else {
                    ""
                }
            );
            if map {
                let _ = writeln!(out, "        w.WritePropertyName(item{depth}.Key);");
            }
            write_value(
                out,
                db,
                t,
                &format!("item{depth}{}", if map { ".Value" } else { "" }),
                depth + 1,
            )?;
            let _ = writeln!(
                out,
                "        }}\n        w.WriteEnd{}();",
                if map { "Object" } else { "Array" }
            );
        }
    }
    Ok(())
}

fn codec(out: &mut String, db: &SchemaDb, t: &TypeDesc, native: bool) -> Result<(), String> {
    let name = &t.name;
    let _ = writeln!(out, "    internal static {name} Read{name}(JsonElement v)");
    if let Shape::Enum { variants } = &t.shape {
        out.push_str("        => v.GetString() switch\n        {\n");
        for variant in variants {
            let _ = writeln!(
                out,
                "            {} => {name}.{},",
                literal(&variant.name),
                pascal_camel(&variant.name)
            );
        }
        out.push_str(
            "            _ => throw new JsonException(\"unknown enum variant\")\n        };\n",
        );
        let _ = writeln!(
            out,
            "    internal static void Write{name}(Utf8JsonWriter w, {name} v) => w.WriteStringValue(v switch\n    {{"
        );
        for variant in variants {
            let _ = writeln!(
                out,
                "        {name}.{} => {},",
                pascal_camel(&variant.name),
                literal(&variant.name)
            );
        }
        out.push_str("        _ => throw new JsonException(\"unknown enum value\")\n    });\n");
        return Ok(());
    }
    out.push_str("        => new()\n        {\n");
    for f in t.fields() {
        let value = format!(
            "Field(v, {}, {})",
            literal(&f.name),
            literal(&serde_json::to_string(&db.field_default(f)).map_err(|e| e.to_string())?)
        );
        let expr = if native && f.ty == ValueType::Entity {
            format!("new Entity({value}.GetUInt64())")
        } else {
            read(db, &f.ty, &value, 0)?
        };
        let _ = writeln!(out, "            {} = {expr},", member(t, &f.name));
    }
    out.push_str("        };\n");
    let _ = writeln!(
        out,
        "    internal static void Write{name}(Utf8JsonWriter w, {name} v)\n    {{\n        w.WriteStartObject();"
    );
    for f in t.fields() {
        let _ = writeln!(out, "        w.WritePropertyName({});", literal(&f.name));
        out.push_str("        {\n");
        let v = format!("v.{}", member(t, &f.name));
        if native && f.ty == ValueType::Entity {
            let _ = writeln!(out, "        w.WriteNumberValue({v}.Bits);");
        } else {
            write_value(out, db, &f.ty, &v, 0)?;
        }
        out.push_str("        }\n");
    }
    out.push_str("        w.WriteEndObject();\n    }\n");
    Ok(())
}

/// Engine component structs plus their reachable value records and codecs.
pub fn render(db: &SchemaDb, native: &[&str]) -> Result<String, String> {
    let names = super::component_names(db, native);
    let mut reachable = BTreeSet::new();
    let mut stack: Vec<_> = names.iter().map(|s| (*s).to_owned()).collect();
    while let Some(n) = stack.pop() {
        if !reachable.insert(n.clone()) {
            continue;
        }
        let t = db.get(&n).ok_or_else(|| format!("unknown type {n}"))?;
        stack.extend(
            t.fields()
                .iter()
                .flat_map(|f| f.ty.named())
                .map(str::to_owned),
        );
    }
    let mut out = String::from(HEADER);
    out.push_str("using System.Numerics;\nusing System.Runtime.InteropServices;\nusing System.Text.Json;\nusing Ara.Hosting;\n\nnamespace Ara;\n\n");
    for n in &reachable {
        let t = db.get(n).expect("reachable registered type");
        if matches!(t.shape, Shape::Enum { .. }) {
            write_enum(&mut out, t);
            continue;
        }
        let index = names.iter().position(|s| *s == n);
        let direct = native.contains(&n.as_str());
        let layout = db
            .struct_layout(n)
            .ok()
            .filter(|_| direct || !t.fields().iter().any(|f| managed(db, &f.ty)));
        doc(&mut out, "", &t.doc);
        if let Some(l) = &layout {
            let _ = writeln!(
                out,
                "[StructLayout(LayoutKind.Explicit, Size = {})]",
                l.size.max(1)
            );
        }
        if index.is_some() {
            let _ = writeln!(
                out,
                "[EngineComponent({}, Module = {})]",
                literal(n),
                literal(&t.module)
            );
        }
        let _ = writeln!(
            out,
            "public partial struct {n}{}\n{{",
            if index.is_some() {
                format!(" : IComponent<{n}>")
            } else {
                String::new()
            }
        );
        for f in t.fields() {
            doc(&mut out, "    ", &f.doc);
            doc_tag(&mut out, "    ", "remarks", &field_remarks(db, f));
            let mut attrs = field_attributes(db, f);
            if let Some(l) = &layout {
                attrs.insert(
                    0,
                    format!(
                        "FieldOffset({})",
                        l.field(&f.name).expect("layout field").offset
                    ),
                );
            }
            if !attrs.is_empty() {
                let _ = writeln!(out, "    [{}]", attrs.join(", "));
            }
            let ty = if direct && f.ty == ValueType::Entity {
                "Entity".into()
            } else {
                cs_type(db, &f.ty)?
            };
            let _ = writeln!(out, "    public {ty} {};", member(t, &f.name));
        }
        if let Some(i) = index {
            out.push_str("    /// <summary>Stable engine component index.</summary>\n");
            let _ = writeln!(out, "    public static uint TypeIndex => {i};");
            if !direct {
                let _ = writeln!(
                    out,
                    "    /// <inheritdoc/>\n    public static ref {n} Access(Entity entity, bool write) => ref EngineAccess.Get(entity, TypeIndex, write, EngineValueCodecs.Read{n}, EngineValueCodecs.Write{n});\n    /// <inheritdoc/>\n    public static void Insert(in Commands commands, Entity entity, in {n} value) => commands.InsertJson(entity, TypeIndex, EngineAccess.Encode(value, EngineValueCodecs.Write{n}));\n    /// <inheritdoc/>\n    public static bool Contains(Entity entity) => Host.ComponentHas(entity.Bits, TypeIndex);"
                );
            }
        }
        out.push_str("}\n\n");
    }
    out.push_str("/// <summary>Engine components in stable binding order.</summary>\npublic static class EngineComponents\n{\n    /// <summary>Schema names by TypeIndex.</summary>\n    public static readonly string[] Names = [");
    out.push_str(
        &names
            .iter()
            .map(|n| literal(n))
            .collect::<Vec<_>>()
            .join(", "),
    );
    out.push_str("];\n    internal static readonly string[] DefaultJson = [\n");
    for name in &names {
        let defaults: serde_json::Map<_, _> = db
            .get(name)
            .expect("component")
            .fields()
            .iter()
            .map(|f| (f.name.clone(), db.field_default(f)))
            .collect();
        let _ = writeln!(
            out,
            "        {},",
            literal(&serde_json::to_string(&defaults).map_err(|e| e.to_string())?)
        );
    }
    out.push_str("    ];\n}\n\ninternal static class EngineValueCodecs\n{\n    static JsonElement Field(JsonElement v, string name, string fallback)\n    {\n        if (v.TryGetProperty(name, out var field)) return field;\n        using var doc = JsonDocument.Parse(fallback);\n        return doc.RootElement.Clone();\n    }\n");
    for n in reachable {
        codec(
            &mut out,
            db,
            db.get(&n).expect("reachable"),
            native.contains(&n.as_str()),
        )?;
    }
    out.push_str("    static void WriteFloat(Utf8JsonWriter w, float value)\n    {\n        if (value == 0 && float.IsNegative(value)) w.WriteRawValue(\"-0.0\"); else w.WriteNumberValue(value);\n    }\n    static void WriteFloat(Utf8JsonWriter w, double value)\n    {\n        if (value == 0 && double.IsNegative(value)) w.WriteRawValue(\"-0.0\"); else w.WriteNumberValue(value);\n    }\n");
    out.push_str("}\n");
    Ok(out)
}
