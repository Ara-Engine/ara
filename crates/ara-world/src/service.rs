//! Engine Protocol adapter for [`WorldTools`] (MCP tool `ara_world`): `world.cells.list`,
//! `world.query`, `world.entity.move_to_cell` and `world.layer.set`. Edits commit
//! through `Cx::transact`, so multi-document moves are attributed, journaled and
//! reviewed exactly like `cmd.apply`. The text index is rebuilt on a worker thread
//! when the store revision moves and published once ready; a query waits briefly for
//! the index of the current revision rather than answering from a stale one.
use crate::{
    CellsList, Manifest, MoveToCell, SetLayer, SpatialQuery, TextIndex, WorldCommand, WorldTools,
};
use ara_diag::{Diagnostic, codes};
use ara_doc::AssetId;
use ara_host::{Cx, Host, Method, Service, parse};
use ara_proto::Scope;
use ara_schema::{Schema, SchemaDb};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    sync::mpsc::{Receiver, RecvTimeoutError},
    time::Duration,
};

/// Registers the world tools on a host (CLI daemon, editor).
pub fn register(host: &mut Host) -> Result<(), Diagnostic> {
    host.register(Box::new(WorldService::default()))
}

/// `world.cells.list` params.
#[derive(Schema, Deserialize)]
#[ara(params, name = "WorldCellsParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CellsParams {
    /// Session handle from `session.begin`.
    #[expect(dead_code, reason = "the host validates the session")]
    session: String,
    /// World document ID or path; default: the project's only world.
    world: Option<String>,
    /// Restrict results to this layer; absent lists every partition.
    layer: Option<String>,
}

/// `world.query` params.
#[derive(Schema, Deserialize)]
#[ara(params, name = "WorldQueryParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QueryParams {
    /// Session handle from `session.begin`.
    #[expect(dead_code, reason = "the host validates the session")]
    session: String,
    /// World document ID or path; default: the project's only world.
    world: Option<String>,
    /// Inclusive world-space minimum.
    #[ara(unit = Meters)]
    min: [f32; 3],
    /// Inclusive world-space maximum.
    #[ara(unit = Meters)]
    max: [f32; 3],
    /// Optional data layer filter.
    layer: Option<String>,
    /// Maximum results (at most 10,000; default 100).
    limit: Option<u32>,
}

/// `world.entity.move_to_cell` params.
#[derive(Schema, Deserialize)]
#[ara(params, name = "WorldMoveParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MoveParams {
    /// Session handle from `session.begin`.
    #[expect(dead_code, reason = "the host validates the session")]
    session: String,
    /// World document ID or path; default: the project's only world.
    world: Option<String>,
    /// Stable entity ID (an `@path#id` address works too).
    entity: String,
    /// Destination manifest cell ID.
    cell: String,
    /// Why the move is made (required from agents, shown in review).
    why: Option<String>,
}

/// `world.layer.set` params.
#[derive(Schema, Deserialize)]
#[ara(params, name = "WorldLayerParams")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LayerParams {
    /// Session handle from `session.begin`.
    #[expect(dead_code, reason = "the host validates the session")]
    session: String,
    /// World document ID or path; default: the project's only world.
    world: Option<String>,
    /// Stable layer record ID.
    layer: String,
    /// Desired runtime/editor loading state.
    loaded: bool,
    /// Desired presentation state.
    visible: bool,
    /// Why the change is made (required from agents, shown in review).
    why: Option<String>,
}

/// An index being built on a worker for one world at one store revision.
struct Build {
    world: AssetId,
    revision: u64,
    result: Receiver<crate::Result<TextIndex>>,
}

/// The world tools service: the published index per world and the build in flight.
#[derive(Default)]
pub struct WorldService {
    tools: Option<(AssetId, u64, WorldTools)>,
    build: Option<Build>,
}

fn fail(message: impl Into<String>) -> Vec<Diagnostic> {
    vec![Diagnostic::error(codes::INVALID_PARAMS, message.into())]
}

fn world_error(e: crate::Error) -> Vec<Diagnostic> {
    e.diagnostics()
}

/// The world document named by ID or path, or the only one.
fn world_of(
    workspace: &ara_doc::Workspace,
    named: Option<&str>,
) -> Result<AssetId, Vec<Diagnostic>> {
    if let Some(named) = named {
        return workspace
            .find(named)
            .and_then(|id| workspace.doc(id.as_str()))
            .filter(|document| document.kind == "world")
            .map(|document| document.id.clone())
            .ok_or_else(|| fail(format!("no world document `{named}`")));
    }
    let worlds: Vec<(&str, &ara_doc::Document)> = workspace
        .docs()
        .filter(|(_, doc)| doc.kind == "world")
        .collect();
    match worlds.as_slice() {
        [(_, doc)] => Ok(doc.id.clone()),
        [] => Err(fail(match named {
            Some(n) => format!("no world document `{n}`"),
            None => "the project has no world document".into(),
        })),
        many => Err(fail(format!(
            "{} world documents; name one with `world`",
            many.len()
        ))),
    }
}

impl WorldService {
    /// Starts an index build on a worker for `world` at the current revision.
    fn start_build(&mut self, cx: &Cx<'_>, world: &AssetId) -> Result<(), Vec<Diagnostic>> {
        let store = cx.store();
        let revision = store.revision();
        if self
            .build
            .as_ref()
            .is_some_and(|b| b.world == *world && b.revision == revision)
        {
            return Ok(());
        }
        let document = store
            .workspace()
            .doc(world.as_str())
            .ok_or_else(|| fail("world is not loaded"))?;
        let manifest = Manifest::read(document).map_err(world_error)?;
        let workspace = store.workspace().clone();
        let schema = store.schema().clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(TextIndex::build(&manifest, &workspace, &schema));
        });
        self.build = Some(Build {
            world: world.clone(),
            revision,
            result: rx,
        });
        Ok(())
    }

    /// Installs a finished build; `wait` blocks up to that long for it.
    fn finish_build(&mut self, wait: Duration) -> Result<(), Vec<Diagnostic>> {
        let Some(build) = &self.build else {
            return Ok(());
        };
        let index = match build.result.recv_timeout(wait) {
            Ok(index) => index,
            Err(RecvTimeoutError::Timeout) => return Ok(()),
            Err(RecvTimeoutError::Disconnected) => Err(crate::invalid("index worker stopped")),
        };
        let build = self
            .build
            .take()
            .ok_or_else(|| fail("index build vanished"))?;
        let index = index.map_err(world_error)?;
        self.tools = Some((
            build.world.clone(),
            build.revision,
            WorldTools::new(build.world, index, build.revision),
        ));
        Ok(())
    }

    /// Tools for `world` whose index matches the current revision (built if needed).
    fn current(&mut self, cx: &Cx<'_>, world: &AssetId) -> Result<&WorldTools, Vec<Diagnostic>> {
        let revision = cx.store().revision();
        let fresh = |tools: &Option<(AssetId, u64, WorldTools)>| {
            tools
                .as_ref()
                .is_some_and(|(w, r, _)| w == world && *r == revision)
        };
        if !fresh(&self.tools) {
            self.start_build(cx, world)?;
            self.finish_build(Duration::from_secs(10))?;
        }
        match &self.tools {
            Some((w, r, tools)) if w == world && *r == revision => Ok(tools),
            _ => Err(fail("the world text index is still building; retry")),
        }
    }
}

impl Service for WorldService {
    fn name(&self) -> &'static str {
        "world"
    }

    fn methods(&self, db: &mut SchemaDb) -> Vec<Method> {
        vec![
            Method::new::<CellsParams>(
                db,
                "world.cells.list",
                "List world cells and data-layer states.",
            )
            .read_only()
            .action("ara_world", "cells"),
            Method::new::<QueryParams>(
                db,
                "world.query",
                "Query entity points across loaded and unloaded text-indexed cells.",
            )
            .read_only()
            .action("ara_world", "query"),
            Method::new::<MoveParams>(
                db,
                "world.entity.move_to_cell",
                "Atomically move an entity subtree between cells (one multi-document transaction), retaining stable IDs.",
            )
            .scope(Scope::EditScene)
            .action("ara_world", "move"),
            Method::new::<LayerParams>(
                db,
                "world.layer.set",
                "Set layer loading and visibility through attributed undoable ops.",
            )
            .scope(Scope::EditScene)
            .action("ara_world", "layer"),
        ]
    }

    fn call(
        &mut self,
        cx: &mut Cx<'_>,
        method: &str,
        params: Value,
    ) -> Result<Value, Vec<Diagnostic>> {
        // Play residency is per player process; authoring tools report text only.
        let resident = |_: &str| false;
        match method {
            "world.cells.list" | "world.query" => {
                let (world, args) = if method == "world.query" {
                    let p: QueryParams = parse(params)?;
                    let query = SpatialQuery {
                        min: p.min,
                        max: p.max,
                        layer: p.layer,
                        limit: p.limit.unwrap_or(100),
                    };
                    (p.world, serde_json::to_value(query))
                } else {
                    let p: CellsParams = parse(params)?;
                    (p.world, serde_json::to_value(CellsList { layer: p.layer }))
                };
                let args = args.map_err(|e| fail(e.to_string()))?;
                let world = world_of(cx.store().workspace(), world.as_deref())?;
                let tools = self.current(cx, &world)?;
                // Reads: the tools take the store mutably but commit nothing.
                cx.transact(None, |store, meta| {
                    tools.call(method, args, store, meta, resident)
                })?
                .map_err(world_error)
            }
            "world.entity.move_to_cell" | "world.layer.set" => {
                let (world, why, command) = if method == "world.layer.set" {
                    let p: LayerParams = parse(params)?;
                    let command = WorldCommand::SetLayer(SetLayer {
                        layer: p.layer,
                        loaded: p.loaded,
                        visible: p.visible,
                    });
                    (p.world, p.why, command)
                } else {
                    let p: MoveParams = parse(params)?;
                    let command = WorldCommand::MoveToCell(MoveToCell {
                        entity: p.entity,
                        cell: p.cell,
                    });
                    (p.world, p.why, command)
                };
                let world = world_of(cx.store().workspace(), world.as_deref())?;
                // One attributed multi-document transaction; the host journals it.
                cx.transact(why, |store, meta| {
                    command
                        .apply(store, &world, meta)
                        .map(|commit| json!({"txn": commit.txn()}))
                })?
                .map_err(world_error)
            }
            other => Err(fail(format!("unknown world method `{other}`"))),
        }
    }

    fn tick(&mut self, _: &mut Cx<'_>) {
        // Publishes a finished background index without blocking the host tick.
        let _ = self.finish_build(Duration::ZERO);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_doc::{Document, Workspace};

    #[test]
    fn named_world_uses_authored_references_and_still_requires_world_kind() {
        let mut workspace = Workspace::default();
        workspace.set_reference_root(ara_doc::AUTHORING_ROOT);
        for (path, kind, id) in [
            ("worlds/first.world", "world", "wd_first"),
            ("worlds/second.world", "world", "wd_second"),
            ("scenes/main.scene", "scene", "sc_main"),
        ] {
            workspace.insert(
                workspace.storage_path(path),
                Document::new(kind, AssetId::parse(id).expect("fixture ID")),
            );
        }
        for named in [
            "worlds/second.world".to_owned(),
            workspace.storage_path("worlds/second.world"),
            "@worlds/second.world#wd_second".to_owned(),
            "wd_second".to_owned(),
        ] {
            assert_eq!(
                world_of(&workspace, Some(&named))
                    .expect("named world")
                    .as_str(),
                "wd_second"
            );
        }
        assert!(world_of(&workspace, Some("scenes/main.scene")).is_err());
        assert!(world_of(&workspace, Some("@scenes/main.scene#sc_main")).is_err());
        assert!(
            world_of(&workspace, None).is_err(),
            "multiple worlds require a selector"
        );
        workspace.remove("wd_first").expect("remove one world");
        assert_eq!(
            world_of(&workspace, None).expect("only world").as_str(),
            "wd_second"
        );
    }
}
