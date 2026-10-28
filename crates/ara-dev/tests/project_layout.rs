//! Repository fixtures must exercise the layout users create today. Legacy inputs
//! belong only to named migration/rejection tests, never a blanket file exemption.
#![allow(clippy::unwrap_used)]

use std::{collections::BTreeMap, fs, path::Path, process::Command};

use quote::ToTokens;
use syn::{Expr, visit::Visit};

#[path = "project_layout/source.rs"]
mod source;

#[test]
fn checked_in_projects_and_test_generators_use_the_current_layout() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(&root)
        .output()
        .expect("enumerate repository fixtures, including uncommitted additions");
    assert!(output.status.success());
    let mut errors = Vec::new();
    let mut projects = 0;
    let mut generators = 0;
    for name in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        let name = std::str::from_utf8(name).unwrap();
        let path = root.join(name);
        // A renamed fixture has both an index entry and its new worktree path.
        if !path.is_file() {
            continue;
        }
        if path
            .file_name()
            .is_some_and(|name| name == "ara.project.json")
        {
            projects += 1;
            let text = fs::read_to_string(&path).unwrap();
            match ara_json::parse(&text) {
                Ok(marker) if ara_doc::detect_project_layout(&marker).is_current() => {
                    if !path
                        .parent()
                        .unwrap()
                        .join(ara_doc::AUTHORING_ROOT)
                        .is_dir()
                    {
                        errors.push(format!("{name}: missing {}/", ara_doc::AUTHORING_ROOT));
                    }
                }
                value => errors.push(format!("{name}: project fixture is not v4: {value:?}")),
            }
        } else if name.ends_with(".rs")
            && name != "crates/ara-dev/tests/project_layout.rs"
            && !name.starts_with("crates/ara-dev/tests/project_layout/")
        {
            let text = fs::read_to_string(&path).unwrap();
            if !text.contains("ara.project.json") && !text.contains("MARKER") {
                continue;
            }
            let file = syn::parse_file(&text).unwrap_or_else(|error| panic!("{name}: {error}"));
            let mut audit = Audit {
                file: name,
                scope: Vec::new(),
                locals: BTreeMap::new(),
                errors: &mut errors,
                generators: &mut generators,
            };
            audit.visit_file(&file);
        } else if name.ends_with(".cs") && name.contains("/tests/") {
            source::csharp(
                name,
                &fs::read_to_string(&path).unwrap(),
                &mut generators,
                &mut errors,
            );
        }
    }
    assert!(projects >= 8, "repository project scan unexpectedly empty");
    assert!(
        generators >= 50,
        "generator scan unexpectedly empty: {generators}"
    );
    assert!(
        errors.is_empty(),
        "{} fixture layout violations:\n{}",
        errors.len(),
        errors.join("\n")
    );
    eprintln!("checked {projects} project markers and {generators} in-code marker writes");
}

struct Audit<'a> {
    file: &'a str,
    scope: Vec<String>,
    locals: BTreeMap<String, Expr>,
    errors: &'a mut Vec<String>,
    generators: &'a mut usize,
}

impl Audit<'_> {
    fn arguments<'a>(&mut self, args: impl IntoIterator<Item = &'a Expr>) {
        let args: Vec<_> = args.into_iter().collect();
        for pair in args.windows(2) {
            if source::marker(pair[0], &self.locals, 0) {
                *self.generators += 1;
                let function = self.scope.join("::");
                let layout = source::layout(pair[1], &self.locals, 0);
                if !source::allowed(self.file, &function, layout) {
                    self.errors.push(format!(
                        "{}::{function}: {layout:?} marker from {}",
                        self.file,
                        pair[1].to_token_stream()
                    ));
                }
            }
        }
    }

    fn function(&mut self, name: String, block: &syn::Block) {
        let previous = std::mem::take(&mut self.locals);
        self.scope.push(name);
        self.visit_block(block);
        self.scope.pop();
        self.locals = previous;
    }
}

impl<'ast> Visit<'ast> for Audit<'_> {
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        self.scope.push(node.ident.to_string());
        syn::visit::visit_item_mod(self, node);
        self.scope.pop();
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        self.scope.push(node.self_ty.to_token_stream().to_string());
        syn::visit::visit_item_impl(self, node);
        self.scope.pop();
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.function(node.sig.ident.to_string(), &node.block);
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.function(node.sig.ident.to_string(), &node.block);
    }

    fn visit_local(&mut self, node: &'ast syn::Local) {
        if let (syn::Pat::Ident(name), Some(value)) = (&node.pat, &node.init) {
            self.locals
                .insert(name.ident.to_string(), (*value.expr).clone());
        }
        syn::visit::visit_local(self, node);
    }

    fn visit_expr_for_loop(&mut self, node: &'ast syn::ExprForLoop) {
        // The tuple initializer is audited once. Its iteration bindings must not
        // accidentally resolve to a same-named path from an enclosing scope.
        let previous = self.locals.clone();
        self.visit_expr(&node.expr);
        fn unbind(pattern: &syn::Pat, locals: &mut BTreeMap<String, Expr>) {
            match pattern {
                syn::Pat::Ident(name) => {
                    locals.remove(&name.ident.to_string());
                }
                syn::Pat::Tuple(tuple) => {
                    for pattern in &tuple.elems {
                        unbind(pattern, locals);
                    }
                }
                _ => {}
            }
        }
        unbind(&node.pat, &mut self.locals);
        self.visit_block(&node.body);
        self.locals = previous;
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let Expr::Path(path) = node.func.as_ref()
            && source::writer(&path.path.segments.last().unwrap().ident.to_string())
        {
            self.arguments(&node.args);
        }
        syn::visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if source::writer(&node.method.to_string()) {
            self.arguments(&node.args);
        }
        syn::visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_tuple(&mut self, node: &'ast syn::ExprTuple) {
        self.arguments(&node.elems);
        syn::visit::visit_expr_tuple(self, node);
    }
}

#[test]
fn source_guard_rejects_missing_versions_per_write_and_resolves_local_values() {
    let file = syn::parse_file(
        r##"fn fixture(root: Path) {
        let path = root.join("ara.project.json");
        let good = json!({"layoutVersion": ara_doc::PROJECT_LAYOUT_VERSION});
        fs::write(&path, ara_json::to_canonical_string(&good));
        fs::write(&path, json!({"name": "old"}).to_string());
        fs::write(&path, json!({"settings": {"layoutVersion": 4}}).to_string());
        fs::write(&path, r#"{"layoutVersion":3}"#);
        for (path, text) in [("ara.project.json", r#"{}"#)] { write(path, text); }
    }"##,
    )
    .unwrap();
    let mut errors = Vec::new();
    let mut generators = 0;
    Audit {
        file: "new_fixture.rs",
        scope: Vec::new(),
        locals: BTreeMap::new(),
        errors: &mut errors,
        generators: &mut generators,
    }
    .visit_file(&file);
    assert_eq!(generators, 5);
    assert_eq!(errors.len(), 4, "{errors:?}");
    assert!(errors.iter().all(|error| error.contains("Legacy")));
}
