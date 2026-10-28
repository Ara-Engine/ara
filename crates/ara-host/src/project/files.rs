//! One semantic authored-file enumeration for the host, asset browser and asset commands.
use ara_diag::{Diagnostic, codes};
use ara_schema::SchemaDb;
use serde::Serialize;
use std::{fs, path::Path};

/// The purpose of one project entry, independent of its browser presentation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AuthoredRole {
    /// An authored directory. Children follow later entries in path order.
    Folder,
    /// Project metadata and root collaboration/configuration support files.
    Metadata,
    /// The shared canonical project-policy document.
    Preferences,
    /// A store-agnostic export profile, including profiles under settings/builds/.
    BuildPreset,
    /// A C# source file.
    Script,
    /// A C# module declaration.
    ScriptModule,
    /// Additional reflected type declarations.
    Schema,
    /// A source-media import document; the source file carries the asset path.
    ImportSidecar,
    /// A document kind from the shared schema registry.
    Document {
        /// Registered document kind.
        name: String,
    },
    /// Source media or another project-owned file without a document schema.
    Source,
    /// A link is visible but never traversed or mutated by asset commands.
    SymbolicLink,
}

/// Separates authored files from project metadata/settings with otherwise identical relative names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AuthoredScope {
    /// Inside the authored root; `path` is also the asset reference path.
    Authored,
    /// Project-root metadata or settings; excluded from game/runtime content.
    Project,
}

/// A project-relative entry, sorted by its canonical '/' path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthoredFile {
    /// Authored-root-relative path, or project-relative path for project scope.
    pub path: String,
    /// Physical project-relative path, always unique even across scopes.
    pub storage_path: String,
    /// Whether this is game-authored data or project configuration.
    pub scope: AuthoredScope,
    /// Semantic role derived from the engine's registered document kinds.
    pub role: AuthoredRole,
}

/// Enumerates authored entries once, without reading source media or following links.
/// Derived roots are skipped only at the project root, so settings/builds/ is authored.
pub fn authored_files(root: &Path, db: &SchemaDb) -> Result<Vec<AuthoredFile>, Diagnostic> {
    let mut out = Vec::new();
    let authored = authored_root(root);
    if authored == root {
        walk(root, root, root, AuthoredScope::Authored, db, &mut out)?;
    } else {
        if authored.is_dir() {
            walk(
                root,
                &authored,
                &authored,
                AuthoredScope::Authored,
                db,
                &mut out,
            )?;
        }
        if root.join("settings").is_dir() {
            walk(
                root,
                root,
                &root.join("settings"),
                AuthoredScope::Project,
                db,
                &mut out,
            )?;
        }
        for name in [
            super::MARKER,
            "AGENTS.md",
            ".mcp.json",
            ".gitignore",
            ".gitattributes",
            "thumbnail.png",
            "ara.imports",
        ] {
            if root.join(name).is_file() {
                out.push(AuthoredFile {
                    path: name.to_owned(),
                    storage_path: name.to_owned(),
                    scope: AuthoredScope::Project,
                    role: AuthoredRole::Metadata,
                });
            }
        }
    }
    out.sort_by(|a, b| a.storage_path.cmp(&b.storage_path));
    Ok(out)
}
fn walk(
    root: &Path,
    reference_root: &Path,
    dir: &Path,
    scope: AuthoredScope,
    db: &SchemaDb,
    out: &mut Vec<AuthoredFile>,
) -> Result<(), Diagnostic> {
    for entry in fs::read_dir(dir).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let kind = entry.file_type().map_err(io_error)?;
        if dir == root
            && (super::SKIPPED.contains(&name.as_ref())
                || ["builds", "packages"].contains(&name.as_ref()))
        {
            continue;
        }
        if kind.is_dir() && (name.starts_with('.') || path.join(super::MARKER).is_file()) {
            continue;
        }
        let storage_path = path
            .strip_prefix(root)
            .map_err(|e| Diagnostic::error(codes::IO, e.to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        let relative = path
            .strip_prefix(reference_root)
            .map_err(|e| Diagnostic::error(codes::IO, e.to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        let scope = if scope == AuthoredScope::Project
            || storage_path == "ara.imports"
            || storage_path == "settings"
            || storage_path.starts_with("settings/")
            || [
                super::MARKER,
                "AGENTS.md",
                ".mcp.json",
                ".gitignore",
                ".gitattributes",
                "thumbnail.png",
            ]
            .contains(&storage_path.as_str())
        {
            AuthoredScope::Project
        } else {
            AuthoredScope::Authored
        };
        let role = if kind.is_symlink() {
            AuthoredRole::SymbolicLink
        } else if kind.is_dir() {
            AuthoredRole::Folder
        } else {
            classify(
                if scope == AuthoredScope::Project {
                    &storage_path
                } else {
                    &relative
                },
                scope,
                db,
            )
        };
        out.push(AuthoredFile {
            path: relative,
            storage_path,
            scope,
            role,
        });
        if kind.is_dir() {
            walk(root, reference_root, &path, scope, db, out)?;
        }
    }
    Ok(())
}
fn classify(path: &str, scope: AuthoredScope, db: &SchemaDb) -> AuthoredRole {
    if path == "settings/project.json" && scope == AuthoredScope::Project {
        return AuthoredRole::Preferences;
    }
    if path.starts_with("settings/builds/")
        && path.ends_with(".export.json")
        && scope == AuthoredScope::Project
    {
        return AuthoredRole::BuildPreset;
    }
    if scope == AuthoredScope::Project
        && [
            super::MARKER,
            "AGENTS.md",
            ".mcp.json",
            ".gitignore",
            ".gitattributes",
            "thumbnail.png",
        ]
        .contains(&path)
    {
        return AuthoredRole::Metadata;
    }
    if path.ends_with(".types.json") {
        return AuthoredRole::Schema;
    }
    if path.ends_with(".import.json") {
        return AuthoredRole::ImportSidecar;
    }
    if path.ends_with(".cs") {
        return AuthoredRole::Script;
    }
    if path.ends_with(".module.json") {
        return AuthoredRole::ScriptModule;
    }
    if let Some(kind) = db
        .kinds()
        .find(|kind| path.ends_with(&format!(".{}", kind.extension)))
    {
        return AuthoredRole::Document {
            name: kind.name.clone(),
        };
    }
    AuthoredRole::Source
}
fn io_error(error: std::io::Error) -> Diagnostic {
    Diagnostic::error(codes::IO, error.to_string())
}

/// Resolves a current project's reference base without changing any files.
/// Open-time validation rejects older layouts before any consumer reaches this adapter.
pub fn authored_root(root: &Path) -> std::path::PathBuf {
    root.join(ara_doc::AUTHORING_ROOT)
}
