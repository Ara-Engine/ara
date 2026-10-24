//! Play and test services for the edit-world owner (`ara` daemon, editor): MCP
//! `ara_play` and `ara_test`. Play runs in pooled `ara-player` child processes (S5);
//! the owner snapshots its in-memory documents into shared memory, routes `play.*`
//! calls to the world's players over the live link, watches them for crashes and
//! hangs, and turns "Keep changes" into one typed-command transaction.
#![warn(missing_docs)]

pub mod assets;
pub mod embed;
mod keep;
mod lifecycle;
mod package;
pub use package::{package_scene, package_scene_for_export, package_scene_with_progress};
pub mod params;
pub mod pool;
mod published;
mod rendering;
pub use published::{PublishedAssets, PublishedGeneration, PublishedSnapshot};
mod scripts;
mod spec;

pub use scripts::ScriptPaths;

use ara_diag::{Code, Diagnostic, FixAction};
use ara_host::{Cx, Method, Service, parse};
use ara_play::{
    Joined, LinkConditions, NetStart, PlayScene, Role, SnapshotRef, Start, method, shm::Section,
};
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
    /// Info: a sidecar recorded an older importer version; it was upgraded and reimported.
    pub const ASSET_UPGRADED: Code = Code(7306);
    /// An embedded player's shared surface could not be created, shared or resized.
    pub const SURFACE: Code = Code(7307);
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
    /// The .NET host for C# modules; `None` when not built (projects with modules
    /// then refuse Play with a setup diagnostic).
    pub scripts: Option<ScriptPaths>,
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
            scripts: std::env::current_exe()
                .ok()
                .and_then(|exe| ScriptPaths::locate(&exe)),
        }
    }
}

/// Players taken and started for a world, their `play.start` results and (embedded
/// worlds) their shared surfaces, index-aligned with the players.
type Launched = (
    Vec<(Role, Player)>,
    Vec<Value>,
    Vec<Option<ara_gpu::shared::SharedSurface>>,
);

/// A running world: its players (authority first), snapshot section and fault.
struct World {
    players: Vec<(Role, Player)>,
    scene: String,
    snapshot: Vec<u8>,
    section: Option<Section>,
    window: bool,
    /// Embedded worlds: the surface size every rendering player started with.
    embed: Option<embed::EmbedParams>,
    /// Embedded players' current surfaces (handles owned by this process), by player.
    surfaces: Vec<Option<ara_gpu::shared::SharedSurface>>,
    start_paused: bool,
    fault: Option<Value>,
    hot_reload: bool,
    /// Player events (C# reloads, diagnostics) not yet read with `play.events`.
    events: Vec<Value>,
    /// Simulated link conditions per client (networked worlds).
    conditions: Vec<Option<LinkConditions>>,
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
    retiring: Vec<lifecycle::Retiring>,
    next: u32,
    /// The in-process editor's [`embed::Surfaces`] handle, once taken.
    channel: Option<embed::Channel>,
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
            retiring: Vec::new(),
            next: 0,
            channel: None,
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
            let scripts = self.script_scene().ok().flatten();
            let pool = Pool::with_scripts(self.config.pool.clone(), self.token.clone(), scripts)
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

    /// Logs pending asset notes (upgrades or placeholders) and adds them to a successful
    /// result as `notes`.
    fn with_notes(
        &mut self,
        cx: &mut Cx<'_>,
        mut result: Result<Value, Vec<Diagnostic>>,
    ) -> Result<Value, Vec<Diagnostic>> {
        let notes = self
            .assets
            .as_mut()
            .map(assets::Assets::take_notes)
            .unwrap_or_default();
        if notes.is_empty() {
            return result;
        }
        if let Ok(value) = &mut result {
            value["notes"] = json!(notes);
        }
        for note in notes {
            cx.log(note);
        }
        result
    }

    /// The most recently started world.
    pub(crate) fn latest(&self) -> Option<String> {
        self.worlds
            .keys()
            .max_by_key(|id| id.trim_start_matches("play:").parse::<u32>().unwrap_or(0))
            .cloned()
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
        self.call_on(id, method, params, (!broadcast).then_some(0))
    }

    /// Calls one player (`Some(index)`: 0 is the authority, then clients in launch
    /// order) or every player (`None`, answered by the authority).
    fn call_on(
        &mut self,
        id: &str,
        method: &str,
        params: Value,
        target: Option<usize>,
    ) -> Result<Value, Vec<Diagnostic>> {
        let world = self.world(id)?;
        if target.is_some_and(|t| t >= world.players.len()) {
            return Err(fail(
                codes::REQUEST,
                format!("world `{id}` has {} players", world.players.len()),
            ));
        }
        let answer = target.unwrap_or(0);
        let mut first = None;
        for (index, (_, player)) in world.players.iter_mut().enumerate() {
            if target.is_some_and(|t| t != index) {
                continue;
            }
            match player.call(method, &params, Duration::from_secs(120)) {
                Ok(Ok(value)) => {
                    if index == answer {
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
        let wanted = scene
            .or_else(|| cx.main_scene())
            .map(str::to_owned)
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
        self.assets()?
            .synchronize(workspace, store.revision())
            .map_err(|e| fail(codes::ASSETS, e))?;
        let pins = self
            .assets()?
            .pins(&referenced, Duration::from_secs(180))
            .map_err(|e| fail(codes::ASSETS, e))?;
        let cache = self.assets()?.cache.to_string_lossy().into_owned();
        let mut types = Vec::new();
        types_files(&self.root, &self.root, &mut types);
        let scene = PlayScene {
            rendering: rendering::read(workspace).map_err(|error| fail(codes::REQUEST, error))?,
            format: 1,
            scene: scene_id.clone(),
            documents: PlayScene::documents(workspace)
                .into_iter()
                .filter(|d| !matches!(d.kind.as_str(), "project" | "import"))
                .collect(),
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
            scripts: self.script_scene()?,
            imports: workspace
                .index()
                .imports()
                .map(|(id, path)| (id.to_owned(), path.to_owned()))
                .collect(),
        };
        Ok((scene_id, scene.encode().map_err(|e| fail(codes::PLAY, e))?))
    }

    /// Assigns players and starts them. Networked worlds start the authority first;
    /// each client then receives its join (issued by the authority over the trusted
    /// player link, never returned to callers) and connects through its own proxy.
    /// Embedded worlds give every rendering player its shared surface first.
    #[allow(clippy::too_many_arguments)]
    fn launch(
        &mut self,
        roles: &[Role],
        window: bool,
        embed: Option<&embed::EmbedParams>,
        snapshot: &SnapshotRef,
        start_paused: bool,
        resume: Option<&str>,
        conditions: &[Option<LinkConditions>],
    ) -> Result<Launched, Vec<Diagnostic>> {
        let clients = roles.iter().filter(|r| **r == Role::Client).count();
        let net = NetStart {
            players: clients as u64 + 1,
            clients: (0..clients)
                .map(|i| conditions.get(i).copied().flatten())
                .collect(),
            join: None,
        };
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
        let mut players: Vec<(Role, Player)> = Vec::new();
        let mut started = Vec::new();
        let mut surfaces = Vec::new();
        for &role in roles {
            let mut player = if window && role != Role::Dedicated {
                windows.pop()
            } else {
                heads.pop()
            }
            .ok_or_else(|| fail(codes::PLAY, "pool returned too few players"))?;
            let surface = match embed {
                Some(params) if role != Role::Dedicated => {
                    Some(embed::embed_player(&mut player, params)?)
                }
                _ => None,
            };
            let net = match role {
                Role::Local => None,
                Role::Host | Role::Dedicated => Some(net.clone()),
                Role::Client => {
                    let index = players.iter().filter(|(r, _)| *r == Role::Client).count();
                    let (_, authority) = players
                        .first_mut()
                        .ok_or_else(|| fail(codes::PLAY, "clients need an authority"))?;
                    let joined = match authority.call(
                        method::JOIN,
                        &json!({"index": index}),
                        Duration::from_secs(30),
                    ) {
                        Ok(Ok(value)) => serde_json::from_value::<Joined>(value)
                            .map_err(|e| fail(codes::PLAY, format!("play.join: {e}")))?,
                        Ok(Err(error)) => {
                            return Err(fail(
                                codes::PLAY,
                                format!("play.join failed: {}", error["message"]),
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
                    };
                    Some(NetStart {
                        players: net.players,
                        clients: Vec::new(),
                        join: Some(joined),
                    })
                }
            };
            let start = Start {
                snapshot: snapshot.clone(),
                role,
                net,
                start_paused,
                resume: resume.map(str::to_owned),
            };
            match player.call(
                method::START,
                &serde_json::to_value(&start).unwrap_or_default(),
                Duration::from_secs(120),
            ) {
                Ok(Ok(result)) => started.push(json!({
                    "role": role,
                    "pid": player.hello.pid,
                    "started": result,
                    "surface": surface.as_ref().map(ara_gpu::shared::SharedSurface::desc),
                })),
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
            players.push((role, player));
            surfaces.push(surface);
        }
        Ok((players, started, surfaces))
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
        if p.embed.is_some() && p.window == Some(true) {
            return Err(fail(
                codes::REQUEST,
                "`embed` shows players inside the editor; leave `window` unset",
            ));
        }
        let conditions = p
            .conditions
            .unwrap_or_default()
            .into_iter()
            .map(|c| {
                (!c.is_null())
                    .then(|| serde_json::from_value::<LinkConditions>(c))
                    .transpose()
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| fail(codes::REQUEST, format!("conditions: {e}")))?;
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
        let (players, started, surfaces) = self.launch(
            &roles,
            window,
            p.embed.as_ref(),
            &snapshot,
            start_paused,
            None,
            &conditions,
        )?;
        let boot_ms = launch.elapsed().as_secs_f64() * 1000.0;
        self.worlds.insert(
            id.clone(),
            World {
                players,
                scene: scene.clone(),
                snapshot: bytes.clone(),
                section,
                window,
                embed: p.embed,
                surfaces,
                start_paused,
                fault: None,
                hot_reload: p.hot_reload.unwrap_or(false),
                events: Vec::new(),
                conditions,
            },
        );
        self.announce(&id);
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

    /// Restarts a world in fresh players from its start snapshot, or from `resume`
    /// (a portable snapshot of a later tick: C# L4 restarts keep the game state).
    fn restart(&mut self, id: &str, resume: Option<&str>) -> Result<Value, Vec<Diagnostic>> {
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
            embed,
            start_paused,
            hot_reload,
            conditions,
            ..
        } = world;
        drop(players);
        self.close(id);
        let (players, started, surfaces) = self.launch(
            &roles,
            window,
            embed.as_ref(),
            &snapshot,
            start_paused,
            resume,
            &conditions,
        )?;
        let boot_ms = began.elapsed().as_secs_f64() * 1000.0;
        self.worlds.insert(
            id.to_owned(),
            World {
                players,
                scene,
                snapshot: bytes,
                section,
                window,
                embed,
                surfaces,
                start_paused,
                fault: None,
                hot_reload,
                events: Vec::new(),
                conditions,
            },
        );
        self.announce(id);
        Ok(json!({"world": id, "bootMs": boot_ms, "players": started}))
    }

    fn stop(&mut self, id: &str) -> Result<Value, Vec<Diagnostic>> {
        ara_gpu::cpu_scope!("Play.stop_service");
        if !self.worlds.contains_key(id) {
            return Err(fail(codes::UNKNOWN_WORLD, format!("no play world `{id}`")));
        }
        let (task, sender) = lifecycle::Retiring::new(id)?;
        let world = self
            .worlds
            .remove(id)
            .expect("validated world remains owned by this service call");
        if let Err(error) = sender.send(world) {
            self.worlds.insert(id.to_owned(), error.0);
            return Err(fail(
                codes::PLAY,
                "shutdown worker stopped before accepting the world",
            ));
        }
        self.retiring.push(task);
        self.close(id);
        Ok(json!({"world": id, "status":"stopping"}))
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
                "players": w.players.iter().zip(&w.surfaces).map(|((role, p), surface)| json!({
                    "role": role,
                    "pid": p.hello.pid,
                    "status": p.status,
                    "surface": surface.as_ref().map(|s| {
                        let d = s.desc();
                        json!({"generation": d.generation, "width": d.width, "height": d.height})
                    }),
                })).collect::<Vec<_>>(),
            }))
            .collect();
        json!({"pool": pool, "worlds": worlds, "stopping": self.retiring.iter().map(|task|
            json!({"world":task.id,"elapsedMs":task.began.elapsed().as_secs_f64()*1000.})
        ).collect::<Vec<_>>()})
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
                    embed: None,
                    start_paused: Some(true),
                    seed: p.seed,
                    tick_hz: p.tick_hz,
                    hot_reload: None,
                    conditions: None,
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
            // C# module folders hold generated types the player's host registers itself.
            if !(name.starts_with('.') || name == "target" || name == "node_modules")
                && !(path != root && path.join("ara.project.json").exists())
                && !path.join("ara.module.toml").exists()
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
            play(Method::new::<PlayDeviceParams>(db, "play.device", "Send raw keyboard/mouse events to one embedded player (the focused Game view); mapped to actions exactly like a windowed player's input."), "device"),
            play(Method::new::<PlayResizeParams>(db, "play.resize", "Resize one embedded player's shared surface; returns the new surface generation (new handles)."), "resize"),
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
            play(Method::new::<PlayRestartParams>(db, "play.restart", "Restart a (faulted) world in fresh players from the same snapshot, or from the current tick with keepState."), "restart"),
            play(Method::new::<PlayCrashParams>(db, "play.debug.crash", "Test fault isolation: make the player panic, raise an access violation or hang.").destructive(), "crash"),
            play(Method::new::<PlayPoolParams>(db, "play.pool", "Set how many headless/windowed players stay warm."), "pool"),
            play(Method::new::<PlayCommandParams>(db, "play.command", "Run a C# [Command] (game.UseMedkit) in the running world between ticks; returns its effects."), "command"),
            play(Method::new::<WorldParams>(db, "play.commands", "The running world's C# command catalog (params schemas, examples; scope play).").read_only(), "commands"),
            play(Method::new::<WorldParams>(db, "play.events", "C# reload reports and module diagnostics since the last call.").read_only(), "events"),
            play(Method::new::<PlayCaptureAudioParams>(db, "play.captureAudio", "Render the world's sound offline (sources at their poses, the listener at the camera) to a WAV with loudness, peak and spectrum analysis.").read_only(), "captureAudio"),
            Method::new::<ScriptWriteParams>(db, "script.write", "Write a C# module source; agent writes are claimed by running players first and reload at a tick boundary (D20 policy).")
                .scope(Scope::EditCode)
                .action("ara_play", "write"),
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
        let latest = self.latest().unwrap_or_default();
        let world_of = |params: &Value| {
            params["world"]
                .as_str()
                .map_or_else(|| latest.clone(), str::to_owned)
        };
        match method {
            "play.start" => {
                let p: PlayStartParams = parse(params)?;
                let result = self.start(cx, p);
                if let Ok(started) = &result
                    && let Some(id) = started["world"].as_str()
                {
                    self.publish_commands(cx, id);
                }
                self.with_notes(cx, result)
            }
            "play.stop" => {
                let result = self.stop(&world_of(&params));
                if let Some(id) = self.latest() {
                    self.publish_commands(cx, &id);
                } else {
                    cx.publish_commands("play", &Value::Null);
                }
                result
            }
            "play.restart" => {
                let p: PlayRestartParams = parse(params)?;
                let id = p.world.unwrap_or_else(|| latest.clone());
                if p.keep_state.unwrap_or(false) {
                    self.resume(&id)
                } else {
                    self.restart(&id, None)
                }
            }
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
            "script.write" => {
                let p: ScriptWriteParams = parse(params)?;
                self.script_write(cx, p)
            }
            "play.events" => {
                let world = self.world(&world_of(&params))?;
                Ok(json!({"events": std::mem::take(&mut world.events)}))
            }
            "play.device" => {
                let p: PlayDeviceParams = parse(params)?;
                let id = p.world.unwrap_or_else(|| latest.clone());
                self.device(&id, p.player as usize, p.events)
            }
            "play.resize" => {
                let p: PlayResizeParams = parse(params)?;
                let id = p.world.unwrap_or_else(|| latest.clone());
                self.resize(&id, p.player as usize, [p.width, p.height])
            }
            "test.run" => {
                let p: TestRunParams = parse(params)?;
                let result = self.test(cx, p);
                self.with_notes(cx, result)
            }
            _ => {
                let id = world_of(&params);
                let mut forward = params;
                let mut only = None;
                if let Some(map) = forward.as_object_mut() {
                    map.remove("session");
                    map.remove("world");
                    only = map
                        .remove("player")
                        .and_then(|p| p.as_u64())
                        .map(|p| p as usize);
                }
                let (target, broadcast) = match method {
                    "play.pause" | "play.resume" | "play.timescale" | "play.input"
                    | "play.patch" | "play.step" => (method, true),
                    "play.inspect" | "play.read" | "play.capture" | "play.watch" | "play.until"
                    | "play.record" | "play.recording" | "play.replay" | "play.diff"
                    | "play.debug.crash" | "play.commands" | "play.captureAudio" => (method, false),
                    // C# commands mutate the world: every player of it runs them.
                    "play.command" => (method, true),
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
                    for key in ["width", "height", "marks", "presented"] {
                        if map.get(key).is_some_and(Value::is_null) {
                            map.remove(key);
                        }
                    }
                }
                match only {
                    Some(index) => self.call_on(&id, target, forward, Some(index)),
                    None => self.call(&id, target, forward, broadcast),
                }
            }
        }
    }

    fn tick(&mut self, cx: &mut Cx<'_>) {
        for problem in self.collect_stopped() {
            cx.log(problem);
        }
        if let Ok(pool) = self.pool() {
            for problem in pool.poll() {
                cx.log(Diagnostic::warning(codes::PLAY, problem));
            }
            pool.refill();
        }
        let mut faults = Vec::new();
        let mut closed = Vec::new();
        for (id, world) in &mut self.worlds {
            if world.fault.is_some() {
                continue;
            }
            for (_, player) in &mut world.players {
                if let Some(fault) = player.check() {
                    if fault["code"] == "player_exited" && fault["exitCode"] == "0x00000000" {
                        closed.push(id.clone());
                        break;
                    }
                    faults.push((id.clone(), fault.clone()));
                    world.fault = Some(fault);
                    break;
                }
            }
        }
        for id in closed {
            let _ = self.stop(&id);
            cx.log(Diagnostic::info(
                codes::PLAY,
                format!("{id}: player window closed"),
            ));
            if self.worlds.is_empty() {
                cx.publish_commands("play", &Value::Null);
            }
        }
        for problem in self.pump_embed() {
            cx.log(problem);
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
        for id in self.drain_events(cx) {
            match self.resume(&id) {
                Ok(_) => cx.log(Diagnostic::info(
                    codes::PLAY,
                    format!("{id}: engine bindings changed; restarted from the current tick"),
                )),
                Err(problems) => problems.into_iter().for_each(|d| cx.log(d)),
            }
        }
        let playing = !self.worlds.is_empty();
        let allow = self.worlds.values().any(|w| w.hot_reload);
        if let Ok(assets) = self.assets() {
            if let Err(error) = assets.synchronize(cx.store().workspace(), cx.store().revision()) {
                cx.log(Diagnostic::warning(codes::ASSETS, error));
            }
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

/// Opens a project with the engine schema world and authoring commands registered
/// first (`ara_runtime::engine`): engine document kinds (`.animation`, `.terrain`,
/// `.world`…) load, project types build on engine types, and the host (CLI, daemon,
/// editor) validates documents exactly as its players do. Clashes become diagnostics.
pub fn open_project(root: &Path) -> Result<ara_host::Project, Diagnostic> {
    ara_host::Project::open_with(root, |db, registry| {
        let mut problems = ara_runtime::engine::register_schemas(db);
        ara_runtime::engine::register_commands(registry, db);
        if let Err(d) = ara_importers::commands::register(registry, db) {
            problems.push(d);
        }
        problems
    })
}
