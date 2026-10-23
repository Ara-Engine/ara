//! `Abi.g.cs`: ABI structs with Rust's offsets, the two function-pointer tables, the
//! shared constants and `AbiInfo` (version, layout hash, size checks).

use std::fmt::Write as _;

use ara_schema::StructLayout;

use super::cs::{HEADER, doc, pascal_snake};
use crate::abi::{
    self, ABI_CONSTS, ABI_STRUCTS, AbiStruct, AbiTable, HOST_API, INIT_STRUCTS, PUBLIC_STRUCTS,
    SCRIPT_API,
};

/// Maps a Rust ABI type (as `stringify!` wrote it) to C#.
fn cs_type(rust: &str) -> Result<String, String> {
    let t = rust.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some(inner) = t
        .strip_prefix("*const ")
        .or_else(|| t.strip_prefix("*mut "))
    {
        return Ok(if inner == "c_void" {
            "void*".into()
        } else {
            format!("{}*", cs_type(inner)?)
        });
    }
    let public = PUBLIC_STRUCTS.iter().any(|s| s.name == t);
    let hosting = ABI_STRUCTS
        .iter()
        .chain(INIT_STRUCTS)
        .any(|s| s.name == t)
        || t == HOST_API.name
        || t == SCRIPT_API.name;
    Ok(match t.as_str() {
        "u8" => "byte".into(),
        "u16" => "ushort".into(),
        "u32" => "uint".into(),
        "i32" => "int".into(),
        "u64" => "ulong".into(),
        "i64" => "long".into(),
        "f32" => "float".into(),
        "f64" => "double".into(),
        _ if public => format!("global::Ara.{t}"),
        _ if hosting => format!("global::Ara.Hosting.{t}"),
        _ => return Err(format!("no C# mapping for ABI type `{t}`")),
    })
}

/// `[u8; 64]` → `Some(("u8", 64))`.
fn array(rust: &str) -> Option<(&str, usize)> {
    let inner = rust.trim().strip_prefix('[')?.strip_suffix(']')?;
    let (ty, n) = inner.split_once(';')?;
    Some((ty.trim(), n.trim().parse().ok()?))
}

fn struct_def(out: &mut String, s: &AbiStruct) -> Result<(), String> {
    doc(out, "    ", s.doc);
    let _ = writeln!(
        out,
        "    [StructLayout(LayoutKind.Explicit, Size = {})]",
        s.size
    );
    let _ = writeln!(out, "    public unsafe partial struct {}\n    {{", s.name);
    for f in s.fields {
        doc(out, "        ", f.doc);
        let name = pascal_snake(f.name);
        if f.name.starts_with("reserved") {
            let _ = writeln!(out, "        [EditorBrowsable(EditorBrowsableState.Never)]");
        }
        match array(f.ty) {
            Some((elem, n)) => {
                let _ = writeln!(
                    out,
                    "        [FieldOffset({})] public fixed {} {name}[{n}];",
                    f.offset,
                    cs_type(elem)?
                );
            }
            None => {
                let _ = writeln!(
                    out,
                    "        [FieldOffset({})] public {} {name};",
                    f.offset,
                    cs_type(f.ty)?
                );
            }
        }
    }
    let _ = writeln!(out, "    }}\n");
    Ok(())
}

fn table_def(out: &mut String, t: &AbiTable) -> Result<(), String> {
    doc(out, "    ", t.doc);
    let _ = writeln!(
        out,
        "    [StructLayout(LayoutKind.Explicit, Size = {})]",
        t.fns.len() * size_of::<usize>()
    );
    let _ = writeln!(out, "    public unsafe partial struct {}\n    {{", t.name);
    for (i, f) in t.fns.iter().enumerate() {
        doc(out, "        ", f.doc);
        let mut sig = Vec::new();
        let mut params = Vec::new();
        for (name, ty) in f.params {
            let ty = cs_type(ty)?;
            params.push(format!("{ty} {name}"));
            sig.push(ty);
        }
        sig.push(if f.ret.is_empty() {
            "void".into()
        } else {
            cs_type(f.ret)?
        });
        let _ = writeln!(out, "        // ({})", params.join(", "));
        let _ = writeln!(
            out,
            "        [FieldOffset({})] public delegate* unmanaged<{}> {};",
            i * size_of::<usize>(),
            sig.join(", "),
            pascal_snake(f.name)
        );
    }
    let _ = writeln!(out, "    }}\n");
    Ok(())
}

/// Renders `Abi.g.cs`.
pub fn render(engine: &[StructLayout]) -> Result<String, String> {
    let mut o = String::from(HEADER);
    o.push_str("using System.ComponentModel;\nusing System.Runtime.CompilerServices;\nusing System.Runtime.InteropServices;\n\n");
    o.push_str("namespace Ara\n{\n");
    for s in PUBLIC_STRUCTS {
        struct_def(&mut o, s)?;
    }
    o.push_str("}\n\nnamespace Ara.Hosting\n{\n");
    for s in ABI_STRUCTS.iter().chain(INIT_STRUCTS) {
        struct_def(&mut o, s)?;
    }
    table_def(&mut o, &HOST_API)?;
    table_def(&mut o, &SCRIPT_API)?;

    o.push_str("    /// <summary>Constants shared with the Rust host (`ara_script::abi`).</summary>\n");
    o.push_str("    public static class AbiConst\n    {\n");
    for c in ABI_CONSTS {
        doc(&mut o, "        ", c.doc);
        // `AbiConst::value` holds the constant `as u64`; i32 constants are small
        // non-negative status codes, so their decimal text is the same.
        let (ty, value) = match c.ty {
            "i32" => ("int".to_owned(), c.value.to_string()),
            "u64" => (cs_type(c.ty)?, format!("{}UL", c.value)),
            other => (cs_type(other)?, format!("{}u", c.value)),
        };
        let _ = writeln!(
            o,
            "        public const {ty} {} = {value};",
            pascal_snake(c.name)
        );
    }
    o.push_str("    }\n\n");

    o.push_str("    /// <summary>ABI identity shared with the Rust host.</summary>\n");
    o.push_str("    public static class AbiInfo\n    {\n");
    let _ = writeln!(
        o,
        "        /// <summary>Must equal the host's <c>ABI_VERSION</c>.</summary>\n        public const uint Version = {};",
        abi::ABI_VERSION
    );
    let _ = writeln!(
        o,
        "        /// <summary>Hash of every ABI layout and the engine components this file was generated from.</summary>\n        public const ulong LayoutHash = 0x{:016X}UL;\n",
        abi::layout_hash(engine)
    );
    o.push_str("        /// <summary>Checks that the C# layouts agree with the generated sizes; returns an error or null.</summary>\n");
    o.push_str("        public static string? VerifySizes()\n        {\n");
    let mut checks: Vec<(String, usize)> = abi::structs()
        .map(|s| {
            let ns = if PUBLIC_STRUCTS.iter().any(|p| p.name == s.name) {
                "Ara"
            } else {
                "Ara.Hosting"
            };
            (format!("global::{ns}.{}", s.name), s.size)
        })
        .collect();
    checks.extend(
        engine
            .iter()
            .map(|c| (format!("global::Ara.{}", c.name), c.size)),
    );
    for (ty, size) in checks {
        let _ = writeln!(
            o,
            "            if (Unsafe.SizeOf<{ty}>() != {size}) return \"{ty}: expected {size} bytes, got \" + Unsafe.SizeOf<{ty}>();"
        );
    }
    o.push_str("            return null;\n        }\n    }\n}\n");
    Ok(o)
}
