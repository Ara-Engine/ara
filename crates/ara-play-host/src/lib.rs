//! Play and test services for the edit-world owner (`ara` daemon, editor): MCP
//! `ara_play` and `ara_test`. Play runs in pooled `ara-player` child processes (S5);
//! the owner snapshots its in-memory documents into shared memory, routes `play.*`
//! calls to the world's players over the live link, watches them for crashes and
//! hangs, and turns "Keep changes" into one typed-command transaction.
#![warn(missing_docs)]

pub mod assets;
pub mod params;
pub mod pool;
mod spec;

use ara_diag::{Code, Diagnostic, FixAction};
use ara_host::{Cx, Method, Service, params::ApplyParams, params::CallSpec, parse};
use ara_play::{PlayScene, Role, SnapshotRef, Start, method, shm::Section};
use ara_proto::Scope;
use ara_schema::SchemaDb;
use params::*;
use pool::{Player, Pool, PoolConfig};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Stable diagnostic codes of the play service (ARA73xx).
pub mod codes {
    use ara_diag::Code;
    /// A player call or link failed.
    pub const PLAY: Code = Code(7300);
    /// The world's player crashed, panicked, hung or exited.
    pub const PLAYER_FAULT: Code = Code(7301);
    /// Unknown world ID.
    pub const UNKNOWN_WORLD: Code = Code(7302);
    /// Assets are not imported (yet) or failed to import.
    pub const ASSETS: Code = Code(7303);
    /// A playtest assertion failed.
    pub const TEST_FAILED: Code = Code(7304);
    /// Invalid play request (scene, mode, spec).
    pub const REQUEST: Code = Code(7305);
}

fn fail(code: Code, message: impl Into<String>) -> Vec<Diagnostic> {
    vec![Diagnostic::error(code, message)]
}

/// Service configuration.
#[derive(Debug, Clone)]
pub struct PlayConfig {
    /// Player processes and pool sizes.
    pub pool: PoolConfig,
    /// The import worker executable (`ara-import-worker`).
    pub import_worker: PathBuf,
    /// Default fixed tick rate.
    pub tick_hz: u32,
}

impl PlayConfig {
    /// Defaults for a project: executables next to the current executable (or
    /// `ARA_PLAYER` / `ARA_IMPORT_WORKER`), two warm headless players, no warm window.
    pub fn for_project(root: &Path) -> Self {
        let sibling = |name: &str, var: &str| {
            std::env::var_os(var).map(PathBuf::from).unwrap_or_else(|| {
                let exe = std::env::current_exe().unwrap_or_default();
                let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
                // Test binaries live in target/<profile>/deps; executables one level up.
                let dir = if dir.ends_with("deps") {
                    dir.parent().map(Path::to_path_buf).unwrap_or(dir)
                } else {
                    dir
                };
                dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
            })
        };
        Self {
            pool: PoolConfig {
                executable: sibling("ara-player", "ARA_PLAYER"),
                logs: root.join(".ara/play"),
                gpu_cache: root.join(".ara/gpu"),
                headless: 2,
                windowed: 0,
                size: [1280, 720],
            },
            import_worker: sibling("ara-import-worker", "ARA_IMPORT_WORKER"),
            tick_hz: 60,
        }
    }
}

/// A running world: its players (authority first), snapshot section and fault.
struct World {
    players: Vec<(Role, Player)>,
    scene: String,
    snapshot: Vec<u8>,
    section: Option<Section>,
    window: bool,
    start_paused: bool,
    fault: Option<Value>,
    hot_reload: bool,
}

/// The `play` and `test` service. Construction is cheap; the pool and the asset
/// pipeline warm up on the first host tick (or the first call).
pub struct PlayService {
    root: PathBuf,
    config: PlayConfig,
    token: String,
    pool: Option<Pool>,
    assets: Option<assets::Assets>,
    worlds: BTreeMap<String, World>,
    next: u32,
}

impl PlayService {
    /// A service for the project at `root`.
    pub fn new(root: &Path, config: PlayConfig) -> Self {
        // 128 bits from the OS-seeded std hasher state; token secrecy is per session.
        let token = (0..2)
            .map(|i| {
                use std::hash::{BuildHasher, Hasher};
                let mut h = std::collections::hash_map::RandomState::new().build_hasher();
                h.write_u64(i ^ u64::from(std::process::id()));
                format!("{:016x}", h.finish())
            })
            .collect();
        Self {
            root: root.to_path_buf(),
            config,
            token,
            pool: None,
            assets: None,
            worlds: BTreeMap::new(),
            next: 0,
        }
    }

    fn pool(&mut self) -> Result<&mut Pool, Vec<Diagnostic>> {
        if self.pool.is_none() {
            if !self.config.pool.executable.exists() {
                return Err(vec![
                    Diagnostic::error(
                        codes::PLAY,
                        format!(
                            "player executable `{}` not found",
                            self.config.pool.executable.display()
                        ),
                    )
                    .with_fix(
                        "Build it, or set ARA_PLAYER",
                        FixAction::Cli {
                            args: vec!["(cargo build -p ara-player)".into()],
                        },
                    ),
                ]);
            }
            let pool = Pool::new(self.config.pool.clone(), self.token.clone())
                .map_err(|e| fail(codes::PLAY, format!("cannot start the player pool: {e}")))?;
            self.pool = Some(pool);
        }
        self.pool
            .as_mut()
            .ok_or_else(|| fail(codes::PLAY, "pool unavailable"))
    }

    fn assets(&mut self) -> Result<&mut assets::Assets, Vec<Diagnostic>> {
        if self.assets.is_none() {
            let assets = assets::Assets::new(&self.root, &self.config.import_worker)
                .map_err(|e| fail(codes::ASSETS, e))?;
            self.assets = Some(assets);
        }
        self.assets
            .as_mut()
            .ok_or_else(|| fail(codes::ASSETS, "asset pipeline unavailable"))
    }

    fn world(&mut self, id: &str) -> Result<&mut World, Vec<Diagnostic>> {
        let world = self.worlds.get_mut(id).ok_or_else(|| {
            fail(
                codes::UNKNOWN_WORLD,
                format!("no play world `{id}`; see `play.status`"),
            )
        })?;
        if let Some(fault) = &world.fault {
            return Err(vec![
                Diagnostic::error(
                    codes::PLAYER_FAULT,
                    format!(
                        "{}; exit {}; last tick {}",
                        fault["message"].as_str().unwrap_or("player fault"),
                        fault["exitCode"],
                        fault["lastTick"]
                    ),
                )
                .with_fix(
                    "Restart play from the same snapshot",
                    FixAction::Call {
                        method: "play.restart".into(),
                        params: json!({"world": id}),
                    },
                ),
            ]);
        }
        Ok(world)
    }

    /// Calls the authority (or every player for `broadcast`) and returns the
    /// authority's result. Faults mark the world and are reported structurally.
    fn call(
        &mut self,
        id: &str,
        method: &str,
        params: Value,
        broadcast: bool,
    ) -> Result<Value, Vec<Diagnostic>> {
        let world = self.world(id)?;
        let mut first = None;
        for (index, (_, player)) in world.players.iter_mut().enumerate() {
            if index > 0 && !broadcast {
                break;
            }
            match player.call(method, &params, Duration::from_secs(120)) {
                Ok(Ok(value)) => {
                    if index == 0 {
                        first = Some(value);
                    }
                }
                Ok(Err(error)) => {
                    return Err(fail(
                        codes::PLAY,
                        error["message"]
                            .as_str()
                            .unwrap_or("player error")
                            .to_owned(),
                    ));
                }
                Err(fault) => {
                    let message = fault["message"]
                        .as_str()
                        .unwrap_or("player fault")
                        .to_owned();
                    world.fault = Some(fault);
                    return Err(vec![
                        Diagnostic::error(codes::PLAYER_FAULT, message).with_fix(
                            "Restart play from the same snapshot",
                            FixAction::Call {
                                method: "play.restart".into(),
                                params: json!({"world": id}),
                            },
                        ),
                    ]);
                }
            }
        }
        first.ok_or_else(|| fail(codes::PLAY, "world has no players"))
    }

    fn scene_snapshot(
        &mut self,
        cx: &Cx<'_>,
        scene: Option<&str>,
        seed: u64,
        tick_hz: u32,
    ) -> Result<(String, Vec<u8>), Vec<Diagnostic>> {
        let store = cx.store();
        let workspace = store.workspace();
        let main = std::fs::read_to_string(self.root.join("ara.project.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v["main"].as_str().map(str::to_owned));
        let wanted = scene
            .map(str::to_owned)
            .or(main)
            .ok_or_else(|| fail(codes::REQUEST, "no scene given and no project main scene"))?;
        let (_, doc) = workspace
            .docs()
            .find(|(path, doc)| *path == wanted || doc.id.as_str() == wanted)
            .ok_or_else(|| {
                fail(
                    codes::REQUEST,
                    format!("scene `{wanted}` is not a loaded document"),
                )
            })?;
        let scene_id = doc.id.to_string();
        // Asset IDs referenced anywhere in the documents (`@path#as_…`).
        let mut referenced = BTreeSet::new();
        for (_, doc) in workspace.docs() {
            for record in doc.records().values() {
                collect_refs(&Value::Object(record.clone()), &mut referenced);
            }
        }
        let pins = self
            .assets()?
            .pins(&referenced, Duration::from_secs(180))
            .map_err(|e| fail(codes::ASSETS, e))?;
        let cache = self.assets()?.cache.to_string_lossy().into_owned();
        let mut types = Vec::new();
        types_files(&self.root, &self.root, &mut types);
        let scene = PlayScene {
            format: 1,
            scene: scene_id.clone(),
            documents: PlayScene::documents(workspace),
            types,
            seed,
            tick_hz,
            assets: pins,
            cache,
            nav_cache: Some(
                self.root
                    .join(".ara/cache/nav")
                    .to_string_lossy()
                    .into_owned(),
            ),
        };
        Ok((scene_id, scene.encode().map_err(|e| fail(codes::PLAY, e))?))
    }

    /// Assigns players and starts them; clients handshake with the authority.
    fn launch(
        &mut self,
        roles: &[Role],
        window: bool,
        snapshot: &SnapshotRef,
        start_paused: bool,
    ) -> Result<(Vec<(Role, Player)>, Vec<Value>), Vec<Diagnostic>> {
        let windowed = roles
            .iter()
            .filter(|r| window && **r != Role::Dedicated)
            .count();
        let headless = roles.len() - windowed;
        let pool = self.pool()?;
        let mut windows = pool
            .take(windowed, true, Duration::from_secs(30))
            .map_err(|e| fail(codes::PLAY, e))?;
        let mut heads = pool
            .take(headless, false, Duration::from_secs(30))
            .map_err(|e| fail(codes::PLAY, e))?;
        let mut players = Vec::new();
        let mut started = Vec::new();
        let mut server = None;
        for &role in roles {
            let mut player = if window && role != Role::Dedicated {
                windows.pop()
            } else {
                heads.pop()
            }
            .ok_or_else(|| fail(codes::PLAY, "pool returned too few players"))?;
            let start = Start {
                snapshot: snapshot.clone(),
                role,
                connect_to: if role == Role::Client {
                    server.clone()
                } else {
                    None
                },
                start_paused,
            };
            match player.call(
                method::START,
                &serde_json::to_value(&start).unwrap_or_default(),
                Duration::from_secs(120),
            ) {
                Ok(Ok(result)) => {
                    started.push(json!({"role": role, "pid": player.hello.pid, "started": result}))
                }
                Ok(Err(error)) => {
                    return Err(fail(
                        codes::PLAY,
                        format!("play.start failed: {}", error["message"]),
                    ));
                }
                Err(fault) => {
                    return Err(fail(
                        codes::PLAYER_FAULT,
                        fault["message"]
                            .as_str()
                            .unwrap_or("player fault")
                            .to_owned(),
                    ));
                }
            }
            if matches!(role, Role::Host | Role::Dedicated) {
                server = player.hello.net_addr.clone();
            }
            players.push((role, player));
        }
        Ok((players, started))
    }

    fn start(&mut self, cx: &Cx<'_>, p: PlayStartParams) -> Result<Value, Vec<Diagnostic>> {
        let began = Instant::now();
        let mode = p.mode.as_deref().unwrap_or("local");
        let clients = p.clients.unwrap_or(0) as usize;
        let roles: Vec<Role> = match mode {
            "local" if clients == 0 => vec![Role::Local],
            "host" => std::iter::once(Role::Host)
                .chain(std::iter::repeat_n(Role::Client, clients))
                .collect(),
            "dedicated" => std::iter::once(Role::Dedicated)
                .chain(std::iter::repeat_n(Role::Client, clients))
                .collect(),
            _ => {
                return Err(fail(
                    codes::REQUEST,
                    "mode is `local` (no clients), `host` or `dedicated`",
                ));
            }
        };
        if roles.len() > 17 {
            return Err(fail(codes::REQUEST, "at most 16 clients"));
        }
        let (scene, bytes) = self.scene_snapshot(
            cx,
            p.scene.as_deref(),
            p.seed.unwrap_or(0),
            p.tick_hz.unwrap_or(self.config.tick_hz),
        )?;
        let prepared = Instant::now();
        self.next += 1;
        let id = format!("play:{}", self.next);
        let name = format!("Local\\ara-play-{}-{}", &self.token[..12], self.next);
        let (section, snapshot) = match Section::create(&name, &bytes) {
            Ok(section) => (
                Some(section),
                SnapshotRef::Shm {
                    section: name,
                    len: bytes.len() as u64,
                },
            ),
            Err(_) => (
                None,
                SnapshotRef::Inline {
                    bytes: bytes.clone(),
                },
            ),
        };
        let encode_ms = prepared.duration_since(began).as_secs_f64() * 1000.0;
        let window = p.window.unwrap_or(false);
        let start_paused = p.start_paused.unwrap_or(false);
        let launch = Instant::now();
        let (players, started) = self.launch(&roles, window, &snapshot, start_paused)?;
        let boot_ms = launch.elapsed().as_secs_f64() * 1000.0;
        self.worlds.insert(
            id.clone(),
            World {
                players,
                scene: scene.clone(),
                snapshot: bytes.clone(),
                section,
                window,
                start_paused,
                fault: None,
                hot_reload: p.hot_reload.unwrap_or(false),
            },
        );
        Ok(json!({
            "world": id,
            "scene": scene,
            "snapshotBytes": bytes.len(),
            "transport": if matches!(snapshot, SnapshotRef::Shm { .. }) { "shm" } else { "inline" },
            "prepareMs": encode_ms,
            "bootMs": boot_ms,
            "players": started,
        }))
    }

    fn restart(&mut self, id: &str) -> Result<Value, Vec<Diagnostic>> {
        let world = self
            .worlds
            .remove(id)
            .ok_or_else(|| fail(codes::UNKNOWN_WORLD, format!("no play world `{id}`")))?;
        let roles: Vec<Role> = world.players.iter().map(|(r, _)| *r).collect();
        let snapshot = match &world.section {
            Some(_) => SnapshotRef::Shm {
                section: format!(
                    "Local\\ara-play-{}-{}",
                    &self.token[..12],
                    id.trim_start_matches("play:")
                ),
                len: world.snapshot.len() as u64,
            },
            None => SnapshotRef::Inline {
                bytes: world.snapshot.clone(),
            },
        };
        let began = Instant::now();
        let World {
            players,
            scene,
            snapshot: bytes,
            section,
            window,
            start_paused,
            hot_reload,
            ..
        } = world;
        drop(players);
        let (players, started) = self.launch(&roles, window, &snapshot, start_paused)?;
        let boot_ms = began.elapsed().as_secs_f64() * 1000.0;
        self.worlds.insert(
            id.to_owned(),
            World {
                players,
                scene,
                snapshot: bytes,
                section,
                window,
                start_paused,
                fault: None,
                hot_reload,
            },
        );
        Ok(json!({"world": id, "bootMs": boot_ms, "players": started}))
    }

    fn stop(&mut self, id: &str) -> Result<Value, Vec<Diagnostic>> {
        let mut world = self
            .worlds
            .remove(id)
            .ok_or_else(|| fail(codes::UNKNOWN_WORLD, format!("no play world `{id}`")))?;
        let mut tick = Value::Null;
        for (_, player) in &mut world.players {
            if let Ok(Ok(result)) = player.call(method::STOP, &json!({}), Duration::from_secs(2)) {
                tick = result["tick"].clone();
            }
            let start = Instant::now();
            while start.elapsed() < Duration::from_millis(500)
                && !matches!(player.child.try_wait(), Ok(Some(_)))
            {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        Ok(json!({"world": id, "tick": tick}))
    }

    /// "Keep changes": diff → filter → `prev` conflict check → one transaction.
    fn keep(&mut self, cx: &mut Cx<'_>, p: PlayKeepParams) -> Result<Value, Vec<Diagnostic>> {
        let diff = self.call(&p.world, method::DIFF, json!({}), false)?;
        let scene = self.world(&p.world)?.scene.clone();
        let changes: Vec<ara_runtime::live::Change> =
            serde_json::from_value(diff["changes"].clone())
                .map_err(|e| fail(codes::PLAY, e.to_string()))?;
        let include_driven = p.include_driven.unwrap_or(false);
        let doc = cx
            .store()
            .workspace()
            .docs()
            .find(|(_, d)| d.id.as_str() == scene)
            .map(|(_, d)| d.clone())
            .ok_or_else(|| {
                fail(
                    codes::REQUEST,
                    format!("scene `{scene}` is no longer loaded"),
                )
            })?;
        let schema = cx.store().schema().clone();
        let mut calls = Vec::new();
        let mut kept = Vec::new();
        let mut conflicts = Vec::new();
        let mut skipped = Vec::new();
        for change in changes {
            let wanted = (include_driven || !change.driven)
                && p.entities
                    .as_ref()
                    .is_none_or(|e| e.contains(&change.entity))
                && p.paths
                    .as_ref()
                    .is_none_or(|paths| paths.iter().any(|q| change.path.starts_with(q.as_str())));
            if !wanted {
                continue;
            }
            let Some(value) = change.value.clone() else {
                skipped.push(json!({"entity": change.entity, "path": change.path, "reason": "removed in play"}));
                continue;
            };
            if change.path.is_empty() {
                skipped.push(json!({"entity": change.entity, "reason": "deleted in play"}));
                continue;
            }
            // The edit world may have changed since Play: `prev` must still hold.
            let current = current_value(&doc, &schema, &change.entity, &change.path);
            if !same(&current, &change.prev) {
                conflicts.push(json!({"entity": change.entity, "path": change.path, "edit": current, "playStart": change.prev, "play": value}));
                continue;
            }
            calls.push(CallSpec {
                cmd: "property.set".into(),
                args: Some(json!({"entity": change.entity, "path": change.path, "value": value})),
                symbol: None,
            });
            kept.push(json!({"entity": change.entity, "path": change.path, "value": value}));
        }
        if calls.is_empty() {
            return Ok(
                json!({"kept": kept, "conflicts": conflicts, "skipped": skipped, "transaction": null}),
            );
        }
        let session = cx
            .session()
            .map(|s| s.id.clone())
            .ok_or_else(|| fail(codes::REQUEST, "keep needs a session"))?;
        let result = cx.apply(ApplyParams {
            commands: calls,
            doc: Some(scene),
            session,
            why: Some(
                p.why
                    .unwrap_or_else(|| format!("Keep changes from {}", p.world)),
            ),
            dry_run: None,
            base: None,
            idempotency_key: None,
        })?;
        Ok(json!({"kept": kept, "conflicts": conflicts, "skipped": skipped, "transaction": result}))
    }

    fn status(&mut self) -> Value {
        let pool = self.pool.as_ref().map(|p| {
            json!({
                "headless": p.ready(false),
                "windowed": p.ready(true),
                "targets": {"headless": p.config().headless, "windowed": p.config().windowed},
            })
        });
        let worlds: Vec<Value> = self
            .worlds
            .iter()
            .map(|(id, w)| json!({
                "world": id,
                "scene": w.scene,
                "window": w.window,
                "hotReload": w.hot_reload,
                "fault": w.fault,
                "players": w.players.iter().map(|(role, p)| json!({"role": role, "pid": p.hello.pid, "status": p.status})).collect::<Vec<_>>(),
            }))
            .collect();
        json!({"pool": pool, "worlds": worlds})
    }

    fn test(&mut self, cx: &Cx<'_>, p: TestRunParams) -> Result<Value, Vec<Diagnostic>> {
        let steps = spec::parse(&p.steps).map_err(|e| fail(codes::REQUEST, e))?;
        let runs = if p.determinism.unwrap_or(false) { 2 } else { 1 };
        let mut reports = Vec::new();
        for _ in 0..runs {
            let world = self.start(
                cx,
                PlayStartParams {
                    session: String::new(),
                    scene: p.scene.clone(),
                    mode: None,
                    clients: None,
                    window: Some(false),
                    start_paused: Some(true),
                    seed: p.seed,
                    tick_hz: p.tick_hz,
                    hot_reload: None,
                },
            )?;
            let id = world["world"].as_str().unwrap_or_default().to_owned();
            let report = spec::run(&steps, |method, params| {
                self.call(&id, method, params, false)
            });
            let _ = self.stop(&id);
            reports.push(report?);
        }
        let passed = reports.iter().all(|r| r["passed"] == json!(true));
        let deterministic = (runs == 2).then(|| reports[0]["hash"] == reports[1]["hash"]);
        let ok = passed && deterministic.unwrap_or(true);
        let result = json!({"passed": ok, "deterministic": deterministic, "runs": reports});
        if ok {
            Ok(result)
        } else {
            Err(fail(
                codes::TEST_FAILED,
                format!("playtest failed: {}", spec::summary(&result)),
            ))
        }
    }
}

/// The stored value at `Component.field…` of `entity` (canonical, defaults omitted).
fn current_value(
    doc: &ara_doc::Document,
    schema: &SchemaDb,
    entity: &str,
    path: &str,
) -> Option<Value> {
    let record = doc.record(entity)?;
    let mut parts = path.split('.');
    let component = parts.next()?;
    let ty = ara_schema::ValueType::Named(component.into());
    let mut value = schema.canonical_value(&ty, record.get(component)?);
    for part in parts {
        value = value.get(part)?.clone();
    }
    Some(value)
}

/// Equality at f32 precision for numbers (play values are f32 widened to JSON).
fn same(a: &Option<Value>, b: &Option<Value>) -> bool {
    fn eq(a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Number(x), Value::Number(y)) => {
                x.as_f64().map(|v| v as f32) == y.as_f64().map(|v| v as f32)
            }
            (Value::Array(x), Value::Array(y)) => {
                x.len() == y.len() && x.iter().zip(y).all(|(a, b)| eq(a, b))
            }
            _ => a == b,
        }
    }
    match (a, b) {
        (Some(a), Some(b)) => eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

fn collect_refs(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::String(s) if s.starts_with('@') => {
            if let Ok(parts) = ara_schema::grammar::parse_ref(s)
                && let Some(id) = parts.id
            {
                out.insert(id.to_owned());
            }
        }
        Value::Array(items) => items.iter().for_each(|v| collect_refs(v, out)),
        Value::Object(map) => map.values().for_each(|v| collect_refs(v, out)),
        _ => {}
    }
}

fn types_files(root: &Path, dir: &Path, out: &mut Vec<Value>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if !(name.starts_with('.') || name == "target" || name == "node_modules")
                && !(path != root && path.join("ara.project.json").exists())
            {
                types_files(root, &path, out);
            }
        } else if name.ends_with(".types.json")
            && let Some(value) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
        {
            out.push(value);
        }
    }
}

impl Service for PlayService {
    fn name(&self) -> &'static str {
        "play"
    }

    fn methods(&self, db: &mut SchemaDb) -> Vec<Method> {
        let play = |m: Method, action: &str| m.scope(Scope::Play).action("ara_play", action);
        vec![
            play(Method::new::<PlayStartParams>(db, "play.start", "Start the scene in pooled player processes (local, host+N or dedicated+N; windowed or headless)."), "start"),
            play(Method::new::<WorldParams>(db, "play.stop", "Stop a play world; its single-use players exit."), "stop"),
            play(Method::new::<WorldParams>(db, "play.pause", "Freeze host-time ticking (explicit steps still work)."), "pause"),
            play(Method::new::<WorldParams>(db, "play.resume", "Resume ticking at the fixed rate."), "resume"),
            play(Method::new::<PlayStepParams>(db, "play.step", "Run exact fixed ticks with action injections; returns the state hash and watch samples."), "step"),
            play(Method::new::<PlayUntilParams>(db, "play.until", "Tick until a watched value satisfies a comparison, within a tick budget."), "until"),
            play(Method::new::<PlayInputParams>(db, "play.input", "Inject input actions (Move, Look, Jump, Sprint) at the next tick."), "input"),
            play(Method::new::<PlayTimescaleParams>(db, "play.timescale", "Set host-time speed; the fixed dt never changes."), "timescale"),
            play(Method::new::<PlayInspectParams>(db, "play.inspect", "Read an entity's runtime components.").read_only(), "inspect"),
            play(Method::new::<PlayReadParams>(db, "play.read", "Read entity.Component.field values at the current tick.").read_only(), "read"),
            play(Method::new::<PlayPatchParams>(db, "play.patch", "Live-edit authored values; applied at the next tick boundary and acknowledged after it."), "patch"),
            play(Method::new::<PlayCaptureParams>(db, "play.capture", "State hash and, with a path, a labelled capture (numbered marks + legend)."), "capture"),
            play(Method::new::<PlayWatchParams>(db, "play.watch", "Sample entity.Component.field paths during step/until."), "watch"),
            play(Method::new::<WorldParams>(db, "play.record", "Start recording a replay (initial checkpoint + per-tick hashes)."), "record"),
            play(Method::new::<PlayFileParams>(db, "play.recording", "Finish the recording and write the replay file."), "saveReplay"),
            play(Method::new::<PlayFileParams>(db, "play.replay", "Verify a replay file tick by tick in this world."), "replay"),
            play(Method::new::<WorldParams>(db, "play.diff", "Field-level differences between play and the start documents.").read_only(), "diff"),
            play(Method::new::<PlayKeepParams>(db, "play.keep", "Keep play changes: one typed-command transaction on the scene (driven values skipped unless asked)."), "keep"),
            play(Method::new::<WorldParams>(db, "play.restart", "Restart a (faulted) world from the same snapshot in fresh players."), "restart"),
            play(Method::new::<PlayCrashParams>(db, "play.debug.crash", "Test fault isolation: make the player panic, raise an access violation or hang.").destructive(), "crash"),
            play(Method::new::<PlayPoolParams>(db, "play.pool", "Set how many headless/windowed players stay warm."), "pool"),
            Method::new::<PlayStatusParams>(db, "play.status", "Pool readiness, worlds, player heartbeats and faults.").read_only().action("ara_play", "status"),
            Method::new::<TestRunParams>(db, "test.run", "Run a playtest (inputs, steps, tick-level assertions) in a fresh headless player; optionally twice for determinism.")
                .scope(Scope::Play)
                .tool("ara_test"),
        ]
    }

    fn call(
        &mut self,
        cx: &mut Cx<'_>,
        method: &str,
        params: Value,
    ) -> Result<Value, Vec<Diagnostic>> {
        let world_of = |params: &Value| params["world"].as_str().unwrap_or_default().to_owned();
        match method {
            "play.start" => {
                let p: PlayStartParams = parse(params)?;
                self.start(cx, p)
            }
            "play.stop" => self.stop(&world_of(&params)),
            "play.restart" => self.restart(&world_of(&params)),
            "play.status" => Ok(self.status()),
            "play.pool" => {
                let p: PlayPoolParams = parse(params)?;
                let pool = self.pool()?;
                let headless = p.headless.map_or(pool.config().headless, |v| v as usize);
                let windowed = p.windowed.map_or(pool.config().windowed, |v| v as usize);
                pool.resize(headless, windowed);
                Ok(json!({"headless": headless, "windowed": windowed}))
            }
            "play.keep" => {
                let p: PlayKeepParams = parse(params)?;
                self.keep(cx, p)
            }
            "test.run" => {
                let p: TestRunParams = parse(params)?;
                self.test(cx, p)
            }
            _ => {
                let id = world_of(&params);
                let mut forward = params;
                if let Some(map) = forward.as_object_mut() {
                    map.remove("session");
                    map.remove("world");
                }
                let (target, broadcast) = match method {
                    "play.pause" | "play.resume" | "play.timescale" | "play.input"
                    | "play.patch" | "play.step" => (method, true),
                    "play.inspect" | "play.read" | "play.capture" | "play.watch" | "play.until"
                    | "play.record" | "play.recording" | "play.replay" | "play.diff"
                    | "play.debug.crash" => (method, false),
                    other => {
                        return Err(fail(
                            codes::REQUEST,
                            format!("unknown play method `{other}`"),
                        ));
                    }
                };
                if target == "play.capture"
                    && let Some(map) = forward.as_object_mut()
                {
                    for key in ["width", "height", "marks"] {
                        if map.get(key).is_some_and(Value::is_null) {
                            map.remove(key);
                        }
                    }
                }
                self.call(&id, target, forward, broadcast)
            }
        }
    }

    fn tick(&mut self, cx: &mut Cx<'_>) {
        if let Ok(pool) = self.pool() {
            for problem in pool.poll() {
                cx.log(Diagnostic::warning(codes::PLAY, problem));
            }
            pool.refill();
        }
        let mut faults = Vec::new();
        for (id, world) in &mut self.worlds {
            if world.fault.is_some() {
                continue;
            }
            for (_, player) in &mut world.players {
                if let Some(fault) = player.check() {
                    faults.push((id.clone(), fault.clone()));
                    world.fault = Some(fault);
                    break;
                }
            }
        }
        for (id, fault) in faults {
            cx.log(
                Diagnostic::error(
                    codes::PLAYER_FAULT,
                    format!(
                        "{id}: {}",
                        fault["message"].as_str().unwrap_or("player fault")
                    ),
                )
                .with_fix(
                    "Restart play from the same snapshot",
                    FixAction::Call {
                        method: "play.restart".into(),
                        params: json!({"world": id}),
                    },
                ),
            );
        }
        let playing = !self.worlds.is_empty();
        let allow = self.worlds.values().any(|w| w.hot_reload);
        if let Ok(assets) = self.assets() {
            let changed = assets.poll(playing, allow);
            if !changed.is_empty() {
                let pins: BTreeMap<String, String> = changed
                    .iter()
                    .map(|c| (c.asset.clone(), ara_play::hex(&c.current.0)))
                    .collect();
                let ids: Vec<String> = self
                    .worlds
                    .iter()
                    .filter(|(_, w)| w.hot_reload)
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in ids {
                    if let Err(d) = self.call(&id, method::ASSETS, json!({"assets": pins}), true) {
                        for d in d {
                            cx.log(d);
                        }
                    }
                }
            }
        }
    }
}

/// Every engine component the runtime understands, for the host's Schema DB.
pub fn register_engine_schemas(db: &mut SchemaDb) {
    ara_runtime::components::register_schemas(db);
}
