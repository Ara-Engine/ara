//! The Rust → C# generator: `Ara.Runtime/Generated/*.g.cs`.
//!
//! - `Abi.g.cs`: the ABI from [`crate::abi`] (structs, tables, constants, `AbiInfo`).
//! - `Components.g.cs`: engine components from the Schema DB as blittable structs,
//!   with units, ranges, replication and docs as attributes and XML docs.
//! - `Units.g.cs`, `Codes.g.cs`: unit symbols and host diagnostic codes.
//!
//! The output is checked in and reviewed; `tests/generated.rs` fails when it is stale
//! (`ARA_UPDATE_GENERATED=1` rewrites it).

mod abi;
mod components;
mod cs;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use ara_diag::Diagnostic;
use ara_schema::{SchemaDb, StructLayout};

use crate::codes;

/// One generated file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenFile {
    /// File name inside the output directory.
    pub name: &'static str,
    /// Contents (LF line endings).
    pub text: String,
}

/// The canonical layouts of the engine components `names`, in binding order.
pub fn engine_layouts(db: &SchemaDb, names: &[&str]) -> Result<Vec<StructLayout>, Diagnostic> {
    names
        .iter()
        .map(|n| match db.get(n) {
            Some(t) if t.is_component() => db
                .struct_layout(n)
                .map_err(|e| Diagnostic::error(codes::NOT_BLITTABLE, e).record(*n)),
            _ => Err(Diagnostic::error(
                codes::UNKNOWN_COMPONENT,
                format!("`{n}` is not a registered component"),
            )),
        })
        .collect()
}

/// Every schema component with a blittable canonical layout, sorted by name.
/// Native engine views must separately validate their Rust layouts with EngineBindings.
pub fn blittable_components(db: &SchemaDb) -> Vec<&str> {
    db.components()
        .filter(|t| db.struct_layout(&t.name).is_ok())
        .map(|t| t.name.as_str())
        .collect()
}

/// Generates every C# file for engine components `engine` (binding order).
pub fn csharp(db: &SchemaDb, engine: &[&str]) -> Result<Vec<GenFile>, Diagnostic> {
    let layouts = engine_layouts(db, engine)?;
    let fail = |e: String| Diagnostic::error(codes::BAD_DESCRIPTOR, e);
    Ok(vec![
        GenFile {
            name: "Abi.g.cs",
            text: abi::render(&layouts).map_err(fail)?,
        },
        GenFile {
            name: "Components.g.cs",
            text: components::render(db, &layouts).map_err(fail)?,
        },
        GenFile {
            name: "Units.g.cs",
            text: components::render_units(),
        },
        GenFile {
            name: "Codes.g.cs",
            text: render_codes(),
        },
    ])
}

fn render_codes() -> String {
    let mut o = String::from(cs::HEADER);
    o.push_str("namespace Ara.Hosting;\n\n/// <summary>Diagnostic codes of the C# host (ARA0900-0999), shared with Rust.</summary>\npublic static class AraCodes\n{\n");
    for (code, name, doc) in codes::ALL {
        cs::doc(&mut o, "    ", doc);
        let _ = writeln!(
            o,
            "    public const string {} = \"{code}\";",
            cs::pascal_snake(name)
        );
    }
    o.push_str("}\n");
    o
}

/// Writes `files` into `dir`.
pub fn write(dir: &Path, files: &[GenFile]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for f in files {
        std::fs::write(dir.join(f.name), &f.text)?;
    }
    Ok(())
}

/// Files in `dir` whose contents differ from `files` (missing files count).
pub fn stale(dir: &Path, files: &[GenFile]) -> Vec<PathBuf> {
    files
        .iter()
        .map(|f| dir.join(f.name))
        .zip(files)
        .filter(|(path, f)| std::fs::read_to_string(path).ok().as_deref() != Some(&f.text))
        .map(|(path, _)| path)
        .collect()
}
