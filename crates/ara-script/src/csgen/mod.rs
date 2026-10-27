//! The Rust → C# generator: `Ara.Runtime/Generated/*.g.cs`.
//!
//! - `Abi.g.cs`: the ABI from [`crate::abi`] (structs, tables, constants, `AbiInfo`).
//! - `Components.g.cs`: engine components from the Schema DB as native views and typed accessor structs,
//!   with units, ranges, replication and docs as attributes and XML docs.
//! - `Units.g.cs`, `Codes.g.cs`: unit symbols and host diagnostic codes.
//!
//! The output is checked in and reviewed; `tests/generated.rs` fails when it is stale
//! (`ARA_UPDATE_GENERATED=1` rewrites it).

mod abi;
mod components;
mod cs;
mod values;

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

// Offline export callers historically seed only ara-components. Their shipping
// identity must still cover the complete standard runtime, including accessors.
fn binding_schema<'a>(
    db: &'a SchemaDb,
    native: &[&str],
) -> Result<std::borrow::Cow<'a, SchemaDb>, Diagnostic> {
    if native != ["Transform"] {
        return Ok(std::borrow::Cow::Borrowed(db));
    }
    let builtin = ara_runtime::schema(&[])
        .map_err(|e| Diagnostic::error(codes::BAD_DESCRIPTOR, e.to_string()))?;
    if builtin.types().all(|t| db.get(&t.name).is_some()) {
        return Ok(std::borrow::Cow::Borrowed(db));
    }
    let mut completed = db.clone();
    for t in builtin.types().filter(|t| db.get(&t.name).is_none()) {
        completed.add(t.clone())?;
    }
    Ok(std::borrow::Cow::Owned(completed))
}

/// ABI identity: native columns first, followed by every reachable accessor contract.
pub fn engine_layouts(db: &SchemaDb, names: &[&str]) -> Result<Vec<StructLayout>, Diagnostic> {
    let schema = binding_schema(db, names)?;
    let db = schema.as_ref();
    let mut layouts: Vec<_> = names
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
        .collect::<Result<_, _>>()?;
    // Accessor codecs have a wire contract too. Include reachable records/enums,
    // not just root names, so a nested field change refuses stale C# at startup.
    let components = component_names(db, names);
    let mut reachable = std::collections::BTreeSet::new();
    let mut stack = components
        .iter()
        .map(|n| (*n).to_owned())
        .collect::<Vec<_>>();
    while let Some(name) = stack.pop() {
        if !reachable.insert(name.clone()) {
            continue;
        }
        let t = db.get(&name).ok_or_else(|| {
            Diagnostic::error(
                codes::BAD_DESCRIPTOR,
                format!("unknown binding type `{name}`"),
            )
        })?;
        stack.extend(
            t.fields()
                .iter()
                .flat_map(|f| f.ty.named())
                .map(str::to_owned),
        );
    }
    for name in reachable.iter().filter(|n| !names.contains(&n.as_str())) {
        let mut layout = binding_layout(db, name);
        if let Some(index) = components.iter().position(|n| *n == name) {
            layout.name = format!("component/{index}/{}", layout.name);
        }
        layouts.push(layout);
    }
    Ok(layouts)
}

/// Fingerprint layout for an accessor type (including variable fields and enums).
pub(crate) fn binding_layout(db: &SchemaDb, name: &str) -> StructLayout {
    if let Ok(layout) = db.struct_layout(name) {
        return layout;
    }
    let t = db.get(name).expect("registered binding type");
    StructLayout {
        name: if t.variants().is_empty() {
            name.to_owned()
        } else {
            format!(
                "{name}:{}",
                t.variants()
                    .iter()
                    .map(|v| v.name.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            )
        },
        size: 0,
        align: 1,
        fields: t
            .fields()
            .iter()
            .map(|f| ara_schema::FieldLayout {
                name: f.name.clone(),
                ty: f.ty.clone(),
                offset: 0,
                layout: ara_schema::ValueLayout { size: 0, align: 1 },
            })
            .collect(),
    }
}

/// Native columns first, then every other engine component in schema order.
pub fn component_names<'a>(db: &'a SchemaDb, native: &[&'a str]) -> Vec<&'a str> {
    let builtin = ara_runtime::schema(&[]).expect("engine schemas are valid");
    native
        .iter()
        .copied()
        .chain(db.components().map(|t| t.name.as_str()).filter(|n| {
            !native.contains(n) && (native != ["Transform"] || builtin.get(n).is_some())
        }))
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
    let schema = binding_schema(db, engine)?;
    let db = schema.as_ref();
    let layouts = engine_layouts(db, engine)?;
    let fail = |e: String| Diagnostic::error(codes::BAD_DESCRIPTOR, e);
    Ok(vec![
        GenFile {
            name: "Abi.g.cs",
            text: abi::render(&layouts, engine.len()).map_err(fail)?,
        },
        GenFile {
            name: "Components.g.cs",
            text: components::render(db, engine).map_err(fail)?,
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
