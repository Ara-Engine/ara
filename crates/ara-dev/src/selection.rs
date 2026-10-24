//! Source changes propagate; test-only edits and equivalent dependency moves do not.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use quote::ToTokens;
use serde::Serialize;
use serde_json::Value;

use crate::{
    git,
    manifest::{self, Lock, Manifest},
    workspace::Metadata,
};

#[derive(Default, Serialize)]
pub(crate) struct Selection {
    pub(crate) affected: BTreeSet<String>,
    pub(crate) changed_libraries: BTreeSet<String>,
    pub(crate) test_only: BTreeSet<String>,
    pub(crate) reasons: Vec<String>,
}

pub(crate) fn compute(
    root: &Path,
    base: &str,
    changed: &[PathBuf],
    metadata: &Metadata,
) -> Result<Selection> {
    let mut paths = BTreeSet::from([
        PathBuf::from("Cargo.toml"),
        "Cargo.lock".into(),
        ".cargo/config.toml".into(),
    ]);
    paths.extend(
        changed
            .iter()
            .filter(|p| p.extension().is_some_and(|e| e == "rs"))
            .cloned(),
    );
    for package in &metadata.packages {
        paths.insert(package.manifest_path.strip_prefix(root)?.to_owned());
    }
    let old_files = git::read_at(root, base, &paths.into_iter().collect::<Vec<_>>())?;
    let old_root = manifest::parse(
        old_files
            .get(Path::new("Cargo.toml"))
            .and_then(Option::as_deref)
            .context("base Cargo.toml")?,
    )?;
    let new_root = manifest::parse(&fs::read_to_string(root.join("Cargo.toml"))?)?;
    let old_lock = Lock::parse(
        old_files
            .get(Path::new("Cargo.lock"))
            .and_then(Option::as_deref)
            .unwrap_or(""),
    )?;
    let new_lock = Lock::parse(&fs::read_to_string(root.join("Cargo.lock"))?)?;
    let lock_changes = old_lock.changed_external(&new_lock)?;
    let mut snapshots = BTreeMap::new();
    let mut result = Selection::default();
    for package in &metadata.packages {
        let path = package.manifest_path.strip_prefix(root)?;
        let directory = path.parent().context("manifest directory")?;
        let previous = old_files
            .get(path)
            .and_then(Option::as_deref)
            .map(|text| Manifest::read(text, &old_root, &old_lock, directory))
            .transpose()?;
        let current = Manifest::read(
            &fs::read_to_string(&package.manifest_path)?,
            &new_root,
            &new_lock,
            directory,
        )?;
        if previous
            .as_ref()
            .is_none_or(|old| !old.equivalent(&current))
        {
            result.changed_libraries.insert(package.name.clone());
            result.reasons.push(format!(
                "{}: package or resolved dependency settings changed",
                package.name
            ));
        } else if previous
            .as_ref()
            .is_some_and(|old| old.bindings != current.bindings)
        {
            // Compile every target to validate renamed imports, without retesting
            // consumers of the same resolved packages and feature sets.
            result.test_only.insert(package.name.clone());
        }
        if !lock_changes.is_empty()
            && (old_lock.reaches(&package.name, &lock_changes)?
                || new_lock.reaches(&package.name, &lock_changes)?)
        {
            result.changed_libraries.insert(package.name.clone());
            result.reasons.push(format!(
                "{}: transitive dependency resolution changed",
                package.name
            ));
        }
        snapshots.insert(package.name.as_str(), (previous, current));
    }
    let mut global = build_settings(&old_root) != build_settings(&new_root);
    if global {
        result
            .reasons
            .push("workspace build settings changed".into());
    }
    for path in changed {
        if path == Path::new("Cargo.toml") || path == Path::new("Cargo.lock") || documentation(path)
        {
            continue;
        }
        // Managed assemblies/generators are build inputs of the Rust script host.
        if path.starts_with("dotnet") && metadata.packages.iter().any(|p| p.name == "ara-script") {
            result.changed_libraries.insert("ara-script".into());
            result
                .reasons
                .push("managed input changed: ara-script and its consumers".into());
            continue;
        }
        if path == Path::new(".cargo/config.toml") {
            let old =
                manifest::parse(old_files.get(path).and_then(Option::as_deref).unwrap_or(""))?;
            let current =
                manifest::parse(&fs::read_to_string(root.join(path)).unwrap_or_default())?;
            if without_linkers(old.clone()) == without_linkers(current.clone()) && old != current {
                // A linker-only change is exercised by final executable packages,
                // including the native/.NET/Vulkan interop paths, not every library test.
                result.test_only.extend(
                    metadata
                        .packages
                        .iter()
                        .filter(|p| p.targets.iter().any(|t| t.kind.iter().any(|k| k == "bin")))
                        .map(|p| p.name.clone()),
                );
                result
                    .reasons
                    .push("linker changed: validate executable packages".into());
            } else if old != current {
                global = true;
                result.reasons.push("Cargo configuration changed".into());
            }
            continue;
        }
        let Some(package) = metadata.owner(root, path) else {
            global = true;
            result
                .reasons
                .push(format!("shared or deleted build input: {}", path.display()));
            continue;
        };
        if path == package.manifest_path.strip_prefix(root)? {
            continue;
        }
        let relative = root
            .join(path)
            .strip_prefix(
                package
                    .manifest_path
                    .parent()
                    .context("package directory")?,
            )?
            .to_owned();
        let test_only = ["tests", "examples", "benches"]
            .iter()
            .any(|p| relative.starts_with(p));
        if path.extension().is_some_and(|e| e == "rs") {
            let (old_manifest, current_manifest) = &snapshots[package.name.as_str()];
            let old = old_files.get(path).and_then(Option::as_deref);
            let current = fs::read_to_string(root.join(path)).ok();
            if let (Some(old), Some(current), Some(old_manifest)) = (old, current, old_manifest)
                && equivalent_rust(old, &current, old_manifest, current_manifest)
            {
                continue;
            }
        }
        if test_only {
            result.test_only.insert(package.name.clone());
        } else {
            result.changed_libraries.insert(package.name.clone());
        }
    }
    result.affected = if global {
        metadata.packages.iter().map(|p| p.name.clone()).collect()
    } else {
        let mut affected = metadata.reverse_closure(&result.changed_libraries);
        affected.extend(result.test_only.iter().cloned());
        affected
    };
    result.reasons.sort();
    result.reasons.dedup();
    Ok(result)
}

fn documentation(path: &Path) -> bool {
    path.starts_with("docs")
        || path.starts_with("reports")
        || path.starts_with("spikes")
        || path.file_name().is_some_and(|n| n == "AGENTS.md")
        // Crate Markdown can be compiled input (notably ara-diag's catalog).
        || !path.starts_with("crates") && path.extension().is_some_and(|e| e == "md")
}

fn build_settings(root: &Value) -> Value {
    let mut root = root.clone();
    if let Some(workspace) = root.get_mut("workspace").and_then(Value::as_object_mut) {
        workspace.remove("dependencies");
        workspace.remove("package");
        workspace.remove("members");
        workspace.remove("exclude");
        workspace.remove("default-members");
    }
    root
}

fn without_linkers(mut root: Value) -> Value {
    if let Some(targets) = root.get_mut("target").and_then(Value::as_object_mut) {
        for target in targets.values_mut().filter_map(Value::as_object_mut) {
            target.remove("linker");
        }
        targets.retain(|_, v| v.as_object().is_none_or(|v| !v.is_empty()));
        if targets.is_empty() {
            root.as_object_mut()
                .expect("TOML root table")
                .remove("target");
        }
    }
    root
}

/// Only remove verified aliases to the same locked package and feature set.
/// All remaining Rust tokens must match; parse failures are conservatively changes.
fn equivalent_rust(old: &str, current: &str, baseline: &Manifest, manifest: &Manifest) -> bool {
    fn normalized(text: &str, local: &Manifest, baseline: &Manifest) -> Option<String> {
        let mut file = syn::parse_file(text).ok()?;
        let mut aliases: BTreeSet<_> = local
            .bindings
            .iter()
            .filter(|(name, binding)| baseline.bindings.get(*name) == Some(*binding))
            .map(|(name, _)| name.clone())
            .collect();
        file.items = file
            .items
            .into_iter()
            .filter_map(|item| {
                let syn::Item::ExternCrate(import) = &item else {
                    return Some(item);
                };
                let name = import
                    .rename
                    .as_ref()
                    .map_or(&import.ident, |(_, alias)| alias)
                    .to_string();
                let source = import.ident.to_string();
                let equivalent = local
                    .bindings
                    .get(&source)
                    .is_some_and(|binding| baseline.bindings.get(&name) == Some(binding));
                let plain = import.attrs.iter().all(|attr| {
                    let tokens = attr.meta.to_token_stream().to_string();
                    if tokens.replace(' ', "") == "cfg(test)" {
                        return true;
                    }
                    if !attr.path().is_ident("doc") {
                        return false;
                    }
                    let syn::Meta::NameValue(value) = &attr.meta else {
                        return false;
                    };
                    let syn::Expr::Lit(value) = &value.value else {
                        return false;
                    };
                    let syn::Lit::Str(value) = &value.lit else {
                        return false;
                    };
                    let value = value.value();
                    !value.contains(['\n', '`']) && !value.starts_with("    ")
                });
                if !equivalent || !plain {
                    return Some(item);
                }
                aliases.insert(name.clone());
                if matches!(import.vis, syn::Visibility::Inherited) {
                    None
                } else {
                    let visibility = &import.vis;
                    let name = syn::Ident::new(&name, import.ident.span());
                    let attrs: Vec<_> = import
                        .attrs
                        .iter()
                        .filter(|a| !a.path().is_ident("doc"))
                        .collect();
                    Some(syn::parse_quote!(#(#attrs)* #visibility use #name;))
                }
            })
            .collect();
        // Bare dependency re-exports commute; all other Rust item ordering stays intact.
        let mut exports = Vec::new();
        file.items.retain(|item| {
            if let syn::Item::Use(import) = item
                && !matches!(import.vis, syn::Visibility::Inherited)
                && let syn::UseTree::Name(name) = &import.tree
                && aliases.contains(&name.ident.to_string())
            {
                exports.push(item.to_token_stream().to_string());
                return false;
            }
            true
        });
        exports.sort();
        Some(format!("{} {exports:?}", file.to_token_stream()))
    }
    normalized(old, baseline, baseline)
        .zip(normalized(current, manifest, baseline))
        .is_some_and(|(a, b)| a == b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_markdown_remains_a_build_input_except_agent_instructions() {
        assert!(documentation(Path::new("docs/dev/ara-dev.md")));
        assert!(documentation(Path::new("crates/ara-dev/AGENTS.md")));
        assert!(!documentation(Path::new("crates/ara-diag/codes/kernel.md")));
        assert!(!documentation(Path::new("crates/core/README.md")));
    }

    #[test]
    fn alias_normalization_requires_the_same_resolved_dependency_and_rust_tokens() {
        let lock = Lock::parse(
            r#"
[[package]]
name = "core"
version = "0.1.0"
dependencies = ["glam"]
[[package]]
name = "glam"
version = "0.34.1"
source = "registry+example"
"#,
        )
        .unwrap();
        let root = manifest::parse(
            r#"[workspace.dependencies]
glam34 = { package = "glam", version = "0.34.1" }
"#,
        )
        .unwrap();
        let header = "[package]\nname = \"core\"\nversion = \"0.1.0\"\n[dependencies]\n";
        let old = Manifest::read(
            &format!("{header}glam = \"0.34\""),
            &root,
            &lock,
            Path::new("crates/core"),
        )
        .unwrap();
        let current = Manifest::read(
            &format!("{header}glam34.workspace = true"),
            &root,
            &lock,
            Path::new("crates/core"),
        )
        .unwrap();
        assert!(old.equivalent(&current));
        assert!(equivalent_rust(
            "pub use glam; fn value() -> u32 { 1 }",
            "pub extern crate glam34 as glam; fn value() -> u32 { 1 }",
            &old,
            &current
        ));
        assert!(equivalent_rust(
            "fn value() -> u32 { 1 } pub use glam;",
            "pub extern crate glam34 as glam; fn value() -> u32 { 1 }",
            &old,
            &current
        ));
        assert!(!equivalent_rust(
            "pub use glam;",
            "#[cfg(test)] pub extern crate glam34 as glam;",
            &old,
            &current
        ));
        assert!(equivalent_rust(
            "fn value() -> u32 { 1 }",
            "extern crate glam34 as glam; fn value() -> u32 { 1 }",
            &old,
            &current
        ));
        assert!(!equivalent_rust(
            "fn value() -> u32 { 1 }",
            "extern crate glam34 as glam; fn value() -> u32 { 2 }",
            &old,
            &current
        ));
        assert!(!equivalent_rust(
            "",
            "#[macro_use] extern crate glam34 as glam;",
            &old,
            &current
        ));
        assert!(!equivalent_rust(
            "",
            "#[cfg(feature = \"hidden\")] extern crate glam34 as glam;",
            &old,
            &current
        ));
        assert!(!equivalent_rust(
            "",
            "extern crate unrelated as glam;",
            &old,
            &current
        ));
        assert!(!equivalent_rust(
            "",
            "pub extern crate glam34 as glam;",
            &old,
            &current
        ));
    }
}
