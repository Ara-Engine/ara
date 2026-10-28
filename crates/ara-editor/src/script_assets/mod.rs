//! Typed source authoring commands shared by native menus and MCP agents.
mod files;
mod jobs;
mod publish;
mod source_jobs;
mod templates;
pub(crate) mod worker;
use ara_host::{Cx, Method, Service, parse};
use ara_proto::Scope;
use ara_schema::{Schema, SchemaDb};
use files::FileOp;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

/// Create source code, compile it and optionally attach its reflected Behaviour.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScriptCreate {
    /// Calling editor or agent session.
    session: String,
    /// Authored-root-relative or storage C# path; the class name follows its file stem.
    path: String,
    /// behaviour (default) or system. Systems cannot attach to entities.
    template: Option<templates::Template>,
    /// Entity addresses to attach after successful compilation.
    attach_to: Option<Vec<String>>,
    /// Scene containing attachTo; defaults to the project's main scene.
    doc: Option<String>,
    /// Compile immediately; false lets an inline rename choose the final class first.
    compile: Option<bool>,
}
/// Create a component script for entities; omitted paths get a unique gameplay name.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NewScript {
    /// Calling editor or agent session.
    session: String,
    /// C# reference or storage path; defaults to an unused Assets/Gameplay/NewBehaviour.cs.
    path: Option<String>,
    /// Behaviour by default; Systems cannot be attached to entities.
    template: Option<templates::Template>,
    /// Entities that receive the component after successful compilation.
    attach_to: Vec<String>,
    /// Scene containing attachTo; defaults to the main scene.
    doc: Option<String>,
}
/// Rename a source file and declaration together before background compilation.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScriptRename {
    /// Calling editor or agent session.
    session: String,
    /// Existing C# source path.
    path: String,
    /// New C# source path; declaration and file name change together.
    to: String,
    /// Entities to attach after the renamed script compiles.
    attach_to: Option<Vec<String>>,
    /// Scene containing attachTo.
    doc: Option<String>,
}
/// Create a folder through the shared authoring file-operation endpoint.
#[derive(Schema, Deserialize)]
#[ara(params)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FolderCreate {
    /// Calling editor or agent session.
    #[expect(dead_code, reason = "validated by host")]
    session: String,
    /// Project-relative folder path.
    path: String,
}

#[derive(Default)]
pub(crate) struct ScriptService {
    authoring: source_jobs::Authoring,
    jobs: BTreeMap<String, jobs::Compile>,
    watch_session: Option<String>,
    signature: Option<u64>,
    sources: Option<std::sync::mpsc::Receiver<u64>>,
}

impl Service for ScriptService {
    fn name(&self) -> &'static str {
        "script-assets"
    }
    fn methods(&self, db: &mut SchemaDb) -> Vec<Method> {
        vec![
            Method::new::<NewScript>(
                db,
                "component.new_script",
                "Create a C# script, compile it and attach it to entities once compiled.",
            )
            .scope(Scope::EditCode)
            .action("ara_session", "component.new_script"),
            Method::new::<ScriptCreate>(
                db,
                "script.create",
                "Create a C# Behaviour or System, compile it and optionally attach it to entities.",
            )
            .scope(Scope::EditCode)
            .action("ara_session", "script.create"),
            Method::new::<ScriptRename>(
                db,
                "script.rename",
                "Rename a C# file and its declaration, then compile it.",
            )
            .scope(Scope::EditCode)
            .action("ara_session", "script.rename"),
            Method::new::<FolderCreate>(
                db,
                "asset.folder.create",
                "Create a project folder through a typed file operation.",
            )
            .scope(Scope::EditAssets)
            .action("ara_session", "asset.folder.create"),
        ]
    }
    fn call(
        &mut self,
        cx: &mut Cx<'_>,
        method: &str,
        params: Value,
    ) -> Result<Value, Vec<ara_diag::Diagnostic>> {
        let fail = |e: String| vec![crate::invalid(e)];
        if method == "component.new_script" {
            let p: NewScript = parse(params)?;
            if p.attach_to.is_empty() {
                return Err(fail("Choose at least one entity for the new script".into()));
            }
            let path = p.path.unwrap_or_else(|| {
                (1..)
                    .map(|n| {
                        if n == 1 {
                            "Assets/Gameplay/NewBehaviour.cs".into()
                        } else {
                            format!("Assets/Gameplay/NewBehaviour{n}.cs")
                        }
                    })
                    .find(|path: &String| {
                        source_jobs::storage_path(path).is_ok_and(|storage| {
                            !self.pending_source(cx, &storage) && !cx.root().join(storage).exists()
                        })
                    })
                    .expect("a finite project has an unused script name")
            });
            return self.call(cx, "script.create", json!({"session":p.session,"path":path,"template":p.template,"attachTo":p.attach_to,"doc":p.doc}));
        }
        if method == "asset.folder.create" {
            let p: FolderCreate = parse(params)?;
            let ops = vec![FileOp::Folder {
                path: p.path.clone(),
            }];
            files::apply(cx.root(), &ops).map_err(fail)?;
            return Ok(json!({"path":p.path,"files":ops}));
        }
        if method == "script.rename" {
            let p: ScriptRename = parse(params)?;
            let from = source_jobs::storage_path(&p.path)?;
            let path = source_jobs::storage_path(&p.to)?;
            script_name(&from).map_err(fail)?;
            script_name(&path).map_err(fail)?;
            let attach = p.attach_to.unwrap_or_default();
            if !attach.is_empty() && cx.session().is_none_or(|s| !s.allows(Scope::EditScene)) {
                return Err(fail("Attaching scripts requires edit:scene scope".into()));
            }
            return self.queue_source(
                cx,
                p.session,
                source_jobs::Action::Rename { from, path },
                p.doc,
                attach,
                true,
            );
        }
        let mut p: ScriptCreate = parse(params)?;
        p.path = source_jobs::storage_path(&p.path)?;
        script_name(&p.path).map_err(fail)?;
        let template = p.template.unwrap_or_default();
        let attach = p.attach_to.unwrap_or_default();
        if !attach.is_empty() && cx.session().is_none_or(|s| !s.allows(Scope::EditScene)) {
            return Err(fail("Attaching scripts requires edit:scene scope".into()));
        }
        if matches!(template, templates::Template::System) && !attach.is_empty() {
            return Err(fail(
                "Systems run over queries; choose Behaviour to attach a script".into(),
            ));
        }
        let doc = p.doc.or_else(|| cx.main_scene().map(str::to_owned));
        if !attach.is_empty() {
            let document = doc
                .as_deref()
                .and_then(|id| cx.store().workspace().find(id))
                .and_then(|id| cx.store().workspace().doc(id.as_str()))
                .ok_or_else(|| fail("Attach needs a scene document".into()))?;
            let mut diagnostics = Vec::new();
            let resolved = ara_doc::prefab::resolve(
                document,
                cx.store().schema(),
                cx.store().workspace(),
                &Default::default(),
                &mut diagnostics,
            );
            if diagnostics.iter().any(ara_diag::Diagnostic::is_error) {
                return Err(diagnostics);
            }
            if attach.iter().any(|id| !resolved.contains_key(id)) {
                return Err(fail("An attachTo entity does not exist".into()));
            }
        }
        self.queue_source(
            cx,
            p.session,
            source_jobs::Action::Create {
                path: p.path,
                template,
            },
            doc,
            attach,
            p.compile.unwrap_or(true),
        )
    }
    fn tick(&mut self, cx: &mut Cx<'_>) {
        self.poll_sources(cx);
        self.poll(cx);
        self.watch(cx);
    }
}

fn script_name(path: &str) -> Result<&str, String> {
    let file = Path::new(path);
    if file.components().any(|part|matches!(part,std::path::Component::Normal(name) if name.to_str().is_some_and(|name|name.starts_with('.')))) {
        return Err("Scripts must be outside hidden and generated-output folders".into());
    }
    let name = file
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("Script needs a C# class name")?;
    if file.extension().is_none_or(|e| e != "cs") || !templates::identifier(name) {
        return Err("Use a C# identifier such as PlayerControl.cs".into());
    }
    Ok(name)
}
fn rename_identifier(text: &str, from: &str, to: &str) -> String {
    text.split_inclusive(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .map(|part| {
            let end = part.trim_end_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_');
            if end == from {
                format!("{to}{}", &part[end.len()..])
            } else {
                part.to_owned()
            }
        })
        .collect()
}
