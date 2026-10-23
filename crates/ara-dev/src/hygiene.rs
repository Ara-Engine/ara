//! Cheap repository checks run before any compilation.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use syn::visit::Visit;
use toml_edit::DocumentMut;

pub(crate) fn pins(text: &str) -> Result<Vec<String>> {
    let doc = text.parse::<DocumentMut>().context("parse manifest")?;
    let mut found = Vec::new();
    fn scan(table: &dyn toml_edit::TableLike, prefix: &str, out: &mut Vec<String>) {
        for (name, item) in table.iter() {
            let key = format!("{prefix}.{name}");
            if matches!(
                name,
                "dependencies" | "dev-dependencies" | "build-dependencies"
            ) {
                if let Some(deps) = item.as_table_like() {
                    for (dep, spec) in deps.iter() {
                        let version = spec
                            .as_str()
                            .or_else(|| spec.get("version").and_then(|v| v.as_str()));
                        if version
                            .is_some_and(|v| v.split(',').any(|s| s.trim_start().starts_with('=')))
                        {
                            out.push(format!("{key}.{dep}: {}", version.unwrap_or_default()));
                        }
                    }
                }
            } else if let Some(child) = item.as_table_like() {
                scan(child, &key, out);
            }
        }
    }
    scan(doc.as_table(), "manifest", &mut found);
    Ok(found)
}

pub(crate) fn agents_limit(text: &str) -> Result<(), String> {
    let lines = text.lines().count();
    if lines > 150 {
        Err(format!("{lines} lines (maximum 150)"))
    } else {
        Ok(())
    }
}

pub(crate) fn manifests(root: &Path, changed: &[PathBuf]) -> Result<Vec<String>> {
    let mut problems = Vec::new();
    for path in changed
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n == "Cargo.toml"))
    {
        if root.join(path).is_file() {
            for pin in pins(&fs::read_to_string(root.join(path))?)? {
                problems.push(format!("{}: {pin}", path.display()));
            }
        }
    }
    Ok(problems)
}

pub(crate) fn agents(root: &Path, files: &[PathBuf]) -> Result<Vec<String>> {
    let mut problems = Vec::new();
    for path in files
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n == "AGENTS.md"))
    {
        if root.join(path).is_file()
            && let Err(error) = agents_limit(&fs::read_to_string(root.join(path))?)
        {
            problems.push(format!("{}: {error}", path.display()));
        }
    }
    Ok(problems)
}

/// Inspect syntax rather than comments/grep context, including inline test modules.
pub(crate) fn importer_versions(root: &Path, files: &[PathBuf]) -> Result<Vec<String>> {
    let mut problems = Vec::new();
    for path in files
        .iter()
        .filter(|p| p.starts_with("crates") && p.extension().is_some_and(|s| s == "rs"))
    {
        if !root.join(path).is_file() {
            continue;
        }
        let source = fs::read_to_string(root.join(path))?;
        if !(source.contains("ImportInput")
            || source.contains("ImportSettings")
            || source.contains(".version()"))
        {
            continue;
        }
        let ast = syn::parse_file(&source).with_context(|| format!("parse {}", path.display()))?;
        let integration = path.components().any(|p| p.as_os_str() == "tests");
        let mut visitor = ImporterVersions {
            in_test: integration,
            problems: Vec::new(),
        };
        visitor.visit_file(&ast);
        problems.extend(
            visitor
                .problems
                .into_iter()
                .map(|p| format!("{}: {p}", path.display())),
        );
    }
    Ok(problems)
}

fn string(expr: &syn::Expr) -> Option<String> {
    match expr {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(s),
            ..
        }) => Some(s.value()),
        syn::Expr::MethodCall(call)
            if matches!(
                call.method.to_string().as_str(),
                "into" | "to_owned" | "to_string"
            ) =>
        {
            string(&call.receiver)
        }
        syn::Expr::Call(call) if call.args.len() == 1 => call.args.first().and_then(string),
        syn::Expr::Reference(expr) => string(&expr.expr),
        _ => None,
    }
}

struct ImporterVersions {
    in_test: bool,
    problems: Vec<String>,
}

impl<'ast> Visit<'ast> for ImporterVersions {
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        let before = self.in_test;
        self.in_test |= node.attrs.iter().any(|a| matches!(&a.meta, syn::Meta::List(m) if m.path.is_ident("cfg") && m.tokens.to_string() == "test"));
        syn::visit::visit_item_mod(self, node);
        self.in_test = before;
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        let before = self.in_test;
        self.in_test |= node.attrs.iter().any(|a| a.path().is_ident("test"));
        syn::visit::visit_item_fn(self, node);
        self.in_test = before;
    }

    fn visit_expr_struct(&mut self, node: &'ast syn::ExprStruct) {
        if self.in_test
            && node.path.segments.last().is_some_and(|s| {
                matches!(
                    s.ident.to_string().as_str(),
                    "ImportInput" | "ImportSettings"
                )
            })
        {
            let field = |name: &str| {
                node.fields
                    .iter()
                    .find(|f| matches!(&f.member, syn::Member::Named(n) if n == name))
                    .map(|f| &f.expr)
            };
            let importer = field("importer");
            let id = importer.and_then(string);
            if importer.is_some()
                && id.as_ref().is_none_or(|id| id.starts_with("ara."))
                && let Some(version) = field("version").and_then(string)
            {
                let importer = id.as_deref().unwrap_or("importer expression");
                self.problems.push(format!("{importer} hard-codes version {version:?}; obtain it from the importer/registry"));
            }
        }
        syn::visit::visit_expr_struct(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if self.in_test
            && node.method == "get"
            && node.args.len() == 2
            && let (Some(id), Some(version)) = (
                node.args.first().and_then(string),
                node.args.last().and_then(string),
            )
            && id.starts_with("ara.")
        {
            self.problems.push(format!("registry lookup {id} hard-codes version {version:?}; use installed importer metadata"));
        }
        syn::visit::visit_expr_method_call(self, node);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        use syn::parse::Parser;
        if self.in_test
            && node.path.is_ident("assert_eq")
            && let Ok(args) =
                syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated
                    .parse2(node.tokens.clone())
            && args.len() >= 2
        {
            let left = &args[0];
            let right = &args[1];
            let importer_version = |expr: &syn::Expr| {
                matches!(expr,
                syn::Expr::MethodCall(call) if call.method == "version" && matches!(&*call.receiver,
                    syn::Expr::Path(path) if path.path.segments.iter().any(|s| s.ident.to_string().to_ascii_lowercase().contains("importer"))))
            };
            if (importer_version(left) && string(right).is_some())
                || (importer_version(right) && string(left).is_some())
            {
                self.problems.push("importer version assertion uses a literal; compare against installed registry metadata".into());
            }
        }
        syn::visit::visit_macro(self, node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_cover_string_inline_target_workspace_and_build_dependencies_only() {
        let pins = pins(
            r#"
[package]
version = "=not-a-dependency"
[dependencies]
good = "1.2"
bad = "=1.2"
range = { version = ">=1, =1.2", features = ["a"] }
inherit.workspace = true
[target.'cfg(windows)'.build-dependencies]
bad = { version = " =2" }
[workspace.dependencies.child]
version = "=3"
"#,
        )
        .unwrap();
        assert_eq!(pins.len(), 4, "{pins:?}");
        assert!(
            super::pins("[dependencies]\ngood = { version = \">=1, <2\" }")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn agents_limit_counts_lines_with_or_without_final_newline() {
        assert!(agents_limit(&"line\n".repeat(150)).is_ok());
        assert!(agents_limit(&"line\n".repeat(151)).is_err());
        assert!(agents_limit("line\n".repeat(151).trim_end()).is_err());
    }

    #[test]
    fn importer_hygiene_distinguishes_builtin_versions_from_fake_contract_fixtures() {
        let file = syn::parse_file(
            r#"
fn production() { let x = ImportInput { importer: "ara.image".into(), version: "1".into() }; }
#[cfg(test)] mod tests {
fn fixture() {
let a = ImportInput { importer: "ara.image".into(), version: "1".into() };
let b = ImportSettings { importer: "test".into(), version: "1".into() };
let c = ImportInput { importer: "ara.image".into(), version: ImageImporter.version().into() };
let d = ImportInput { importer: ImageImporter.id().into(), version: "1".into() };
assert_eq!(ImageImporter.version(), "1");
}
}
"#,
        )
        .unwrap();
        let mut visitor = ImporterVersions {
            in_test: false,
            problems: vec![],
        };
        visitor.visit_file(&file);
        assert_eq!(visitor.problems.len(), 3);
    }
}
