//! Body-only policy. Keep layout, signatures, generated code and retained closures immutable.
use anyhow::{Context, Result, bail, ensure};
use quote::ToTokens;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
use syn::{
    visit::Visit,
    visit_mut::{self, VisitMut},
};

/// Baseline build inputs. A mismatch requires rebuilding and restarting the warm host.
#[derive(Serialize, Deserialize)]
pub struct Contract {
    root: PathBuf,
    files: BTreeMap<PathBuf, Input>,
}

#[derive(Serialize, Deserialize)]
struct Input {
    bytes: String,
    shape: String,
}

impl Contract {
    /// Capture engine Rust sources and build configuration before the initial build.
    pub fn capture(root: &Path) -> Result<Self> {
        let root = root.canonicalize()?;
        let files = inputs(&root, None)?
            .into_iter()
            .map(|path| {
                let input = fingerprint(&root.join(&path))?;
                Ok((path, input))
            })
            .collect::<Result<_>>()?;
        Ok(Self { root, files })
    }

    /// Refuse declarations/build inputs and bodies with retained, unsupported state.
    /// Reviewed recording entries may change their synchronous frame-local closures.
    /// Run off the frame thread, before loading any patch library.
    pub fn validate(&self) -> Result<()> {
        self.validate_scope(None)
    }

    /// Compiler-facade check for one workspace crate, before and after rustc.
    /// Third-party sources use the frozen lockfile and are outside live patching.
    pub fn validate_source(&self, source: &Path) -> Result<()> {
        let source = source.canonicalize()?;
        if !source.starts_with(self.root.join("crates")) {
            return Ok(());
        }
        let directory = source
            .ancestors()
            .skip(1)
            .find(|p| p.join("Cargo.toml").is_file())
            .context("Rust source has no owning crate")?;
        self.validate_scope(Some(directory.strip_prefix(&self.root)?))
    }

    fn validate_scope(&self, scope: Option<&Path>) -> Result<()> {
        let paths = inputs(&self.root, scope)?;
        let expected: Vec<_> = self
            .files
            .iter()
            .filter(|(path, _)| {
                scope.is_none_or(|s| path.starts_with(s) || !path.starts_with("crates"))
            })
            .collect();
        ensure!(
            paths.len() == expected.len() && paths.iter().all(|p| self.files.contains_key(p)),
            "needs rebuild: Rust source files were added or removed"
        );
        for (path, original) in expected {
            let text = fs::read(self.root.join(path))?;
            if blake3::hash(&text).to_hex().as_str() == original.bytes {
                continue;
            }
            let current = fingerprint_bytes(path, &text)?;
            ensure!(
                current.shape == original.shape,
                "needs rebuild: {} changed a declaration, build input or unsupported function body (async, generics, closures or impl Trait)",
                path.display()
            );
        }
        Ok(())
    }

    /// Store a baseline under the driver's isolated target directory.
    pub fn write(&self, path: &Path) -> Result<()> {
        fs::write(path, serde_json::to_vec(self)?).context("write hotpatch source contract")
    }

    /// Read the driver's baseline, shared by its worker and the warm process.
    pub fn read(path: &Path) -> Result<Self> {
        serde_json::from_slice(&fs::read(path)?).context("read hotpatch source contract")
    }
}

fn inputs(root: &Path, scope: Option<&Path>) -> Result<Vec<PathBuf>> {
    fn walk(root: &Path, directory: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            let path = entry.path();
            if kind.is_symlink() {
                bail!(
                    "needs rebuild: symlink in Rust source tree: {}",
                    path.display()
                );
            }
            if kind.is_dir() {
                if !matches!(
                    entry.file_name().to_str(),
                    Some("target" | ".git" | "cooked" | "node_modules")
                ) {
                    walk(root, &path, paths)?;
                }
            } else if path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                matches!(
                    e,
                    "rs" | "h" | "hpp" | "c" | "cc" | "cpp" | "m" | "mm" | "inc"
                )
            }) || entry.file_name() == "Cargo.toml"
            {
                paths.push(path.strip_prefix(root)?.to_owned());
            }
        }
        Ok(())
    }
    let mut paths = Vec::new();
    walk(
        root,
        &root.join(scope.unwrap_or_else(|| Path::new("crates"))),
        &mut paths,
    )?;
    for name in [
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        ".cargo/config.toml",
    ] {
        if root.join(name).is_file() {
            paths.push(PathBuf::from(name));
        }
    }
    paths.sort();
    Ok(paths)
}

fn fingerprint(path: &Path) -> Result<Input> {
    fingerprint_bytes(path, &fs::read(path)?)
}

fn fingerprint_bytes(path: &Path, bytes: &[u8]) -> Result<Input> {
    let hash = blake3::hash(bytes).to_hex().to_string();
    let shape = if path.extension().is_some_and(|e| e == "rs")
        && path.file_name().is_none_or(|n| n != "build.rs")
    {
        let mut file = syn::parse_file(std::str::from_utf8(bytes)?)
            .with_context(|| format!("parse {}", path.display()))?;
        let source = path.to_string_lossy().replace('\\', "/");
        Bodies { source: &source }.visit_file_mut(&mut file);
        blake3::hash(file.to_token_stream().to_string().as_bytes())
            .to_hex()
            .to_string()
    } else {
        hash.clone()
    };
    Ok(Input { bytes: hash, shape })
}

struct Bodies<'a> {
    source: &'a str,
}
impl Bodies<'_> {
    fn body(&mut self, sig: &syn::Signature, body: &mut syn::Block) {
        // These reviewed entry points hold a Lease until synchronous recording/UI
        // work ends. Their iterator/memoization closures are fresh frame locals;
        // none is the persisted function environment passed through HotFn.
        let scoped = (sig.ident == "record_frame"
            && [
                "crates/ara-render/src/renderer/passes.rs",
                "crates/ara-render-fidelity/src/render.rs",
            ]
            .iter()
            .any(|path| self.source.ends_with(*path)))
            || (sig.ident == "record_post_process"
                && self
                    .source
                    .ends_with("crates/ara-render-fidelity/src/features.rs"))
            || (sig.ident == "draw_panel"
                && self
                    .source
                    .ends_with("crates/ara-editor/src/shell/panels.rs"));
        let mut retained = Retained(false, scoped);
        retained.visit_block(body);
        retained.visit_return_type(&sig.output);
        if sig.constness.is_some()
            || sig.asyncness.is_some()
            || !sig.generics.params.is_empty()
            || retained.0
        {
            return;
        }
        // Nested items can alter layouts too, including items inside if/loop blocks.
        let mut items = Items(Vec::new());
        items.visit_block(body);
        body.stmts = items.0.into_iter().map(syn::Stmt::Item).collect();
        visit_mut::visit_block_mut(self, body);
    }
}
impl VisitMut for Bodies<'_> {
    fn visit_item_impl_mut(&mut self, node: &mut syn::ItemImpl) {
        // Inherited type parameters and existing vtables are retained state too.
        if node.generics.params.is_empty() && node.trait_.is_none() {
            visit_mut::visit_item_impl_mut(self, node);
        }
    }
    fn visit_item_trait_mut(&mut self, _: &mut syn::ItemTrait) {}
    fn visit_item_fn_mut(&mut self, node: &mut syn::ItemFn) {
        self.body(&node.sig, &mut node.block);
    }
    fn visit_impl_item_fn_mut(&mut self, node: &mut syn::ImplItemFn) {
        self.body(&node.sig, &mut node.block);
    }
    fn visit_trait_item_fn_mut(&mut self, node: &mut syn::TraitItemFn) {
        if let Some(body) = &mut node.default {
            self.body(&node.sig, body);
        }
    }
}
struct Retained(bool, bool);
fn pure_macro(name: &str, scoped: bool) -> bool {
    // These expand to literal values or a synchronous scope guard, never new
    // declarations/retained environments. Their nested arguments are checked too.
    (scoped
        && matches!(
            name,
            "cpu_scope" | "env" | "option_env" | "concat" | "stringify"
        ))
        || matches!(
            name,
            "format"
                | "format_args"
                | "println"
                | "eprintln"
                | "write"
                | "writeln"
                | "vec"
                | "matches"
                | "assert"
                | "assert_eq"
                | "assert_ne"
                | "debug_assert"
                | "debug_assert_eq"
                | "debug_assert_ne"
                | "ensure"
                | "bail"
                | "anyhow"
        )
}

fn macro_tokens(tokens: proc_macro2::TokenStream, scoped: bool) -> bool {
    use proc_macro2::TokenTree;
    let tokens: Vec<_> = tokens.into_iter().collect();
    for (index, token) in tokens.iter().enumerate() {
        match token {
            TokenTree::Group(group) if !macro_tokens(group.stream(), scoped) => return false,
            TokenTree::Punct(p) if p.as_char() == '|' && !scoped => return false,
            TokenTree::Ident(ident) => {
                let name = ident.to_string();
                if matches!(
                    name.as_str(),
                    "struct"
                        | "enum"
                        | "type"
                        | "trait"
                        | "impl"
                        | "static"
                        | "thread_local"
                        | "async"
                ) {
                    return false;
                }
                if matches!(tokens.get(index + 1), Some(TokenTree::Punct(p)) if p.as_char() == '!')
                    && matches!(tokens.get(index + 2), Some(TokenTree::Group(_)))
                    && !pure_macro(&name, scoped)
                {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}
impl<'ast> Visit<'ast> for Retained {
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        // Unknown macros may define statics/types or store callbacks. Pure formatting
        // and assertion macros are allowed only without hidden declarations/closures.
        let pure = node
            .path
            .segments
            .last()
            .is_some_and(|s| pure_macro(&s.ident.to_string(), self.1));
        if !pure || !macro_tokens(node.tokens.clone(), self.1) {
            self.0 = true;
        }
    }
    fn visit_expr_closure(&mut self, node: &'ast syn::ExprClosure) {
        self.0 |= !self.1 || node.asyncness.is_some();
        syn::visit::visit_expr_closure(self, node);
    }
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        let method = node.method.to_string();
        if matches!(
            method.as_str(),
            "create" | "allocate" | "destroy" | "retire"
        ) || method.starts_with("create_")
            || method.starts_with("allocate_")
            || method.starts_with("destroy_")
            || method.starts_with("retire_")
        {
            self.0 = true;
        }
        syn::visit::visit_expr_method_call(self, node);
    }
    fn visit_expr_async(&mut self, _: &'ast syn::ExprAsync) {
        self.0 = true;
    }
    fn visit_type_impl_trait(&mut self, _: &'ast syn::TypeImplTrait) {
        self.0 = true;
    }
}
struct Items(Vec<syn::Item>);
impl<'ast> Visit<'ast> for Items {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        self.0.push(item.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn shape(text: &str) -> String {
        fingerprint_bytes(Path::new("a.rs"), text.as_bytes())
            .unwrap()
            .shape
    }
    #[test]
    fn real_fidelity_recording_body_is_live_with_its_literal_macros_and_local_closures() {
        let path = Path::new("crates/ara-render-fidelity/src/render.rs");
        let source = include_str!("../../ara-render-fidelity/src/render.rs");
        let anchor = "let (frame, vp, camera) = self.frame(input)?;";
        assert!(source.contains(anchor));
        let edited = source.replace(
            anchor,
            "let (frame, vp, camera) = self.frame(input)?; let _proof = 7;",
        );
        assert_eq!(
            fingerprint_bytes(path, source.as_bytes()).unwrap().shape,
            fingerprint_bytes(path, edited.as_bytes()).unwrap().shape,
        );
    }
    #[test]
    fn reviewed_recording_locals_can_change_but_persisted_layouts_cannot() {
        let path = Path::new("crates/ara-render-fidelity/src/render.rs");
        let shape = |text: &str| fingerprint_bytes(path, text.as_bytes()).unwrap().shape;
        assert_eq!(
            shape("fn record_frame() { let a = [1].map(|v| v+1); }"),
            shape("fn record_frame() { let a = [1].map(|v| v+2); }")
        );
        assert_ne!(
            shape("fn record_frame() { let a = || { struct S(u32); }; }"),
            shape("fn record_frame() { let a = || { struct S(u64); }; }")
        );
        assert_ne!(
            shape("fn record_frame() { let a = async { 1 }; }"),
            shape("fn record_frame() { let a = async { 2 }; }")
        );
        assert_ne!(
            shape("fn record_frame() { gpu.create_image(1); }"),
            shape("fn record_frame() { gpu.create_image(2); }")
        );
        assert_eq!(
            shape("fn record_frame() { ara_gpu::cpu_scope!(\"a\"); }"),
            shape("fn record_frame() { ara_gpu::cpu_scope!(\"b\"); }")
        );
    }
    #[test]
    fn body_changes_preserve_the_contract_but_layouts_do_not() {
        assert_eq!(
            shape("struct A(u32); fn tick(a: &mut A) { a.0 += 1; }"),
            shape("struct A(u32); fn tick(a: &mut A) { a.0 += 2; }")
        );
        assert_ne!(shape("struct A(u32);"), shape("struct A(u64);"));
        assert_ne!(
            shape("fn tick() { if true { struct A(u32); } }"),
            shape("fn tick() { if true { struct A(u64); } }")
        );
        assert_ne!(shape("fn tick(a: u32) {}"), shape("fn tick(a: u64) {}"));
    }
    #[test]
    fn retained_state_must_rebuild() {
        for (old, new) in [
            ("const fn f() -> usize { 1 }", "const fn f() -> usize { 2 }"),
            ("async fn f() { let a = 1; }", "async fn f() { let a = 2; }"),
            ("fn f<T>() { let a = 1; }", "fn f<T>() { let a = 2; }"),
            ("fn f() { let a = || 1; }", "fn f() { let a = || 2; }"),
            (
                "impl<T> A<T> { fn f() { let a = 1; } }",
                "impl<T> A<T> { fn f() { let a = 2; } }",
            ),
            (
                "impl Trait for A { fn f() { let a = 1; } }",
                "impl Trait for A { fn f() { let a = 2; } }",
            ),
        ] {
            assert_ne!(shape(old), shape(new));
        }
    }

    #[test]
    fn macros_cannot_hide_new_persistent_layouts() {
        assert_ne!(
            shape("fn f() { format!(\"{}\", hidden!(A)); }"),
            shape("fn f() { format!(\"{}\", hidden!(B)); }")
        );
        assert_ne!(
            shape("fn f() { thread_local! { static A: u32 = 1; } }"),
            shape("fn f() { thread_local! { static A: u64 = 1; } }")
        );
        assert_eq!(
            shape("fn f() { println!(\"old\"); }"),
            shape("fn f() { println!(\"new\"); }")
        );
        assert_ne!(
            shape("fn f() { format!(\"{}\", { static A:u32=1; A }); }"),
            shape("fn f() { format!(\"{}\", { static A:u64=1; A }); }")
        );
    }

    #[test]
    fn contract_checks_source_sets_and_build_inputs_in_scratch() {
        let root =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/hotpatch-contract-tests");
        fs::create_dir_all(&root).unwrap();
        let scratch = tempfile::tempdir_in(root).unwrap();
        let source = scratch.path().join("crates/probe/src");
        fs::create_dir_all(&source).unwrap();
        let file = source.join("lib.rs");
        fs::write(&file, "struct A(u32); fn tick(a: &mut A) { a.0 += 1; }").unwrap();
        let contract = Contract::capture(scratch.path()).unwrap();
        fs::write(&file, "struct A(u32); fn tick(a: &mut A) { a.0 += 2; }").unwrap();
        contract.validate().unwrap();
        fs::write(&file, "struct A(u64); fn tick(a: &mut A) { a.0 += 2; }").unwrap();
        assert!(
            contract
                .validate()
                .unwrap_err()
                .to_string()
                .contains("needs rebuild")
        );
    }
}
