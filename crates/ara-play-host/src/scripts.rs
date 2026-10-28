//! C# gameplay modules in Play: locating the .NET host build, finding the project's
//! modules from the shared D30 inventory, agent source writes claimed in the
//! players' compile servers before they land (`script.write`: D20 agent edits),
//! C# commands (`play.command`), reload events, and L4 restarts that keep the game
//! state (resume from a portable snapshot of the current tick).
use crate::{PlayService, codes, fail, params::ScriptWriteParams};
use ara_diag::Diagnostic;
use ara_host::Cx;
use ara_play::{Event, ScriptScene, method};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
mod diagnostics;
mod write;
pub(crate) use diagnostics::DiagnosticSources;
pub(crate) use write::SourceWrites;

/// The .NET JIT host and compile server players load C# modules with.
#[derive(Debug, Clone)]
pub struct ScriptPaths {
    /// `Ara.Host` build output (`dotnet/artifacts/bin/Ara.Host/release`).
    pub host_dir: PathBuf,
    /// `Ara.CompileServer.dll`; `None` loads prebuilt module assemblies without reload.
    pub compile_server: Option<PathBuf>,
}

impl ScriptPaths {
    /// Installed .NET tools, then `ARA_DOTNET_ARTIFACTS`, then checkout artifacts.
    pub fn locate(start: &Path) -> Option<Self> {
        let tools = ara_script::project::ToolPaths::locate(start)?;
        Some(Self {
            host_dir: tools.host_dir,
            compile_server: Some(tools.compile_server),
        })
    }
}

/// Published gameplay input directories, including implicit and nested modules.
/// Graph/compilation errors are returned rather than silently dropping modules.
pub fn modules(root: &Path) -> Result<Vec<PathBuf>, Vec<Diagnostic>> {
    let published = ara_script::project::publish(root, ara_script::ModuleProfile::Gameplay)
        .map_err(|d| vec![d])?;
    Ok(published
        .gameplay_specs(root)?
        .into_iter()
        .map(|m| m.dir)
        .collect())
}

/// Keeps a C# module's generated `*.types.json` current (canonical text, written only
/// when it changed, never outside the project).
fn write_types(root: &Path, path: &Path, doc: &Value) -> Result<(), Diagnostic> {
    let problem = |m: String| Diagnostic::warning(codes::PLAY, m);
    let root = std::fs::canonicalize(root).map_err(|e| problem(e.to_string()))?;
    let dir = path
        .parent()
        .and_then(|d| std::fs::canonicalize(d).ok())
        .ok_or_else(|| problem(format!("{} has no module folder", path.display())))?;
    if !dir.starts_with(&root) || path.extension().is_none_or(|e| e != "json") {
        return Err(problem(format!(
            "{} is outside the project",
            path.display()
        )));
    }
    let text = ara_json::to_canonical_string(doc);
    if std::fs::read_to_string(path).ok().as_deref() != Some(text.as_str()) {
        std::fs::write(path, text).map_err(|e| problem(e.to_string()))?;
    }
    Ok(())
}

/// The C# part of a play scene for the project at `root` (a walk for its modules):
/// `None` without modules; an error when it has modules but `paths` is not built.
pub(crate) fn script_scene(
    root: &Path,
    paths: Option<&ScriptPaths>,
) -> Result<Option<ScriptScene>, Vec<Diagnostic>> {
    let modules = modules(root)?;
    if modules.is_empty() {
        return Ok(None);
    }
    let paths = paths.ok_or_else(|| {
        fail(
            codes::REQUEST,
            "the project has C# modules but the .NET host is not built; run `ara doctor` \
             (`dotnet build dotnet/Ara.slnx -c Release`, or set ARA_DOTNET_ARTIFACTS)",
        )
    })?;
    let text = |p: &Path| p.to_string_lossy().into_owned();
    Ok(Some(ScriptScene {
        host: Default::default(),
        host_dir: text(&paths.host_dir),
        compile_server: paths.compile_server.as_deref().map(text),
        modules: modules.iter().map(|m| text(m)).collect(),
    }))
}

impl PlayService {
    /// The C# part of a play scene: `None` without modules; an error when the project
    /// has modules but the .NET host is not built.
    pub(crate) fn script_scene(&self) -> Result<Option<ScriptScene>, Vec<Diagnostic>> {
        script_scene(&self.root, self.config.scripts.as_ref())
    }

    /// `script.write`: an agent writes a C# source. Every running world's players
    /// claim the path as an agent edit first (coalesced, compiled behind human edits),
    /// then the file is written; the reload applies at a tick boundary.
    fn claim_source(&mut self, path: &Path, agent: bool) -> Result<usize, Vec<Diagnostic>> {
        let mut claimed = 0;
        if agent {
            let ids: Vec<String> = self.worlds.keys().cloned().collect();
            for id in ids {
                let result = self.call(
                    &id,
                    method::COMPILE,
                    json!({"paths": [path.to_string_lossy()]}),
                    true,
                )?;
                claimed += usize::from(result["claimed"].as_bool().unwrap_or(false));
            }
        }
        Ok(claimed)
    }

    /// Drains player events into each world's buffer (logging diagnostics and the
    /// authority's successful reloads, so the Console and `ara logs` show them) and
    /// returns the worlds whose players asked for an L4 restart.
    pub(crate) fn drain_events(&mut self, cx: &mut Cx<'_>) -> Vec<String> {
        let mut restarts = Vec::new();
        let mut reloaded = Vec::new();
        for (id, world) in &mut self.worlds {
            for (index, (_, player)) in world.players.iter_mut().enumerate() {
                for mut event in std::mem::take(&mut player.events) {
                    if let Event::Reload {
                        tick,
                        level,
                        cascade,
                        errors: 0,
                        ..
                    } = &event
                        && index == 0
                    {
                        cx.log(Diagnostic::info(
                            codes::PLAY,
                            format!(
                                "{id}: C# reloaded at tick {tick} (level {level}, {cascade} module{})",
                                if *cascade == 1 { "" } else { "s" }
                            ),
                        ));
                    }
                    match &mut event {
                        Event::Diagnostic { diagnostic } => {
                            if let Ok(d) = serde_json::from_value::<Diagnostic>(diagnostic.clone())
                            {
                                match self.script_diagnostics.scope(
                                    id,
                                    index,
                                    &player.incarnation,
                                    d,
                                ) {
                                    Ok(d) => {
                                        *diagnostic = serde_json::to_value(&d)
                                            .expect("diagnostics serialize to JSON");
                                        cx.log(d);
                                    }
                                    Err(d) => cx.log(d),
                                }
                            }
                        }
                        Event::Restart => {
                            if !restarts.contains(id) {
                                restarts.push(id.clone());
                            }
                        }
                        Event::Types { files } => {
                            for (path, doc) in files {
                                if let Err(d) = write_types(&self.root, Path::new(path), doc) {
                                    cx.log(d);
                                }
                            }
                        }
                        Event::Reload { .. } => {
                            if !reloaded.contains(id) {
                                reloaded.push(id.clone());
                            }
                        }
                    }
                    world
                        .events
                        .push(serde_json::to_value(&event).unwrap_or_default());
                }
            }
            let excess = world.events.len().saturating_sub(256);
            world.events.drain(..excess);
        }
        // Reloads may add, change or remove C# commands.
        for id in reloaded {
            self.publish_commands(cx, &id);
        }
        restarts
    }

    /// Publishes `world`s C# command catalog to the host (`schema.list` shows it next
    /// to the registry commands; `play.command` runs them). A player that faults
    /// answering is logged (the world is marked faulted), never dropped silently.
    pub(crate) fn publish_commands(&mut self, cx: &mut Cx<'_>, world: &str) {
        match self.call(world, method::COMMANDS, json!({}), false) {
            Ok(catalog) => {
                if let Some(world) = self.worlds.get_mut(world) {
                    world.commands = catalog.clone();
                }
                cx.publish_commands("play", &catalog);
            }
            Err(problems) => problems.into_iter().for_each(|d| cx.log(d)),
        }
    }

    /// Restarts a world from a portable snapshot of its current tick (C# L4).
    pub(crate) fn resume(&mut self, id: &str) -> Result<Value, Vec<Diagnostic>> {
        let snapshot = self.call(id, method::SNAPSHOT, json!({}), false)?;
        let bytes = snapshot["snapshot"]
            .as_str()
            .ok_or_else(|| fail(codes::PLAY, "player returned no snapshot"))?
            .to_owned();
        self.restart(id, Some(&bytes))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod discovery_tests {
    use super::*;
    #[test]
    fn a_root_module_keeps_its_generated_types_out_of_project_types() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-play-host/root-module-types")
            .join(std::process::id().to_string());
        let authored = root.join(ara_doc::AUTHORING_ROOT);
        std::fs::create_dir_all(authored.join("types")).unwrap();
        std::fs::write(
            root.join("ara.project.json"),
            ara_json::to_canonical_string(
                &json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION}),
            ),
        )
        .unwrap();
        std::fs::write(authored.join("root.module.json"), r#"{"name":"Root"}"#).unwrap();
        std::fs::write(
            authored.join("Root.types.json"),
            r#"{"types":{"Spinner":{}}}"#,
        )
        .unwrap();
        std::fs::write(
            authored.join("types/Project.types.json"),
            r#"{"types":{"Door":{}}}"#,
        )
        .unwrap();
        let mut found = Vec::new();
        crate::types_files(&root, &root, &mut found);
        assert_eq!(found, vec![serde_json::json!({"types":{"Door":{}}})]);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn root_and_nested_modules_keep_their_own_identity() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ara-play-host/module-inventory")
            .join(std::process::id().to_string());
        let authored = root.join(ara_doc::AUTHORING_ROOT);
        std::fs::create_dir_all(authored.join("Gameplay/Nested")).unwrap();
        std::fs::write(
            root.join("ara.project.json"),
            ara_json::to_canonical_string(
                &json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION}),
            ),
        )
        .unwrap();
        std::fs::write(authored.join("root.module.json"), r#"{"name":"Root"}"#).unwrap();
        std::fs::write(
            authored.join("Gameplay/gameplay.module.json"),
            r#"{"name":"Gameplay"}"#,
        )
        .unwrap();
        std::fs::write(
            authored.join("Gameplay/Nested/nested.module.json"),
            r#"{"name":"Nested"}"#,
        )
        .unwrap();
        let canonical = std::fs::canonicalize(&root).unwrap();
        assert_eq!(
            modules(&root).unwrap(),
            vec![
                canonical.join(".ara/scripts/Gameplay"),
                canonical.join(".ara/scripts/Nested"),
                canonical.join(".ara/scripts/Root")
            ]
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
