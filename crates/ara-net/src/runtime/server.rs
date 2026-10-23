mod authority;
mod replication;
use super::*;
use crate::{ConnectionView, Interest, WorldIds};
use authority::{new_link, receive, tick};
use replication::refresh;
use std::marker::PhantomData;

/// Listen/dedicated authority configuration. No renderer or editor is required.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Listen or Dedicated; Local uses Session directly.
    pub mode: SessionMode,
    /// Direct listen-host input owner (ignored by Dedicated).
    pub local_owner: PlayerId,
    /// UDP bind address; port zero allocates an ephemeral port.
    pub bind: SocketAddr,
    /// Maximum authenticated connections.
    pub max_clients: usize,
    /// Number of ready peers before the initial tick barrier opens.
    pub start_clients: usize,
    /// Fixed simulation frequency, matching Session's tick configuration.
    pub hz: u32,
    /// Snapshot rate in Hz, independent of simulation/presentation rate.
    pub snapshot_hz: u32,
    /// Optional deterministic stopping boundary for tests.
    pub tick_limit: Option<u64>,
    /// Per-connection application byte budget per second.
    pub bytes_per_second: u32,
    /// Optional proxy addresses accepted by netcode authentication.
    pub expected_addresses: Vec<SocketAddr>,
}
impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            mode: SessionMode::Dedicated,
            local_owner: PlayerId(0),
            bind: SocketAddr::from(([127, 0, 0, 1], 0)),
            max_clients: 16,
            start_clients: 1,
            hz: 60,
            snapshot_hz: 20,
            tick_limit: None,
            bytes_per_second: 64000,
            expected_addresses: Vec::new(),
        }
    }
}

/// Optional authority adapter driven through ara-sim's Session seam.
struct Adapter {
    mode: SessionMode,
    gate: IntentGate,
    inputs: Vec<Injection>,
    intents: Vec<(PlayerId, Option<PlayerId>, Intent)>,
    completed_tick: u64,
}
impl SessionAdapter for Adapter {
    fn mode(&self) -> SessionMode {
        self.mode
    }
    fn before_tick(&mut self, session: &mut Session) -> std::result::Result<(), ara_sim::SimError> {
        for (caller, owner, intent) in self.intents.drain(..) {
            let _ = self.gate.submit(session, caller, owner, intent);
        }
        session.inject(session.tick() + 1, std::mem::take(&mut self.inputs))
    }
    fn after_tick(
        &mut self,
        _: &World,
        record: &TickRecord,
    ) -> std::result::Result<(), ara_sim::SimError> {
        self.completed_tick = record.tick;
        Ok(())
    }
}
struct Actor {
    spawn: Spawn,
    entity: Entity,
    position: [f32; 2],
    public: Projection,
    private: Projection,
    native: Projection,
}
#[derive(Component)]
struct ByteBudget(u32);
#[derive(Resource)]
struct Authority {
    session: Session,
    adapter: Adapter,
    config: ServerConfig,
    actors: BTreeMap<String, Actor>,
    ids: WorldIds,
    views: BTreeMap<PlayerId, ConnectionView>,
    ready: BTreeMap<PlayerId, Entity>,
    issued: BTreeSet<PlayerId>,
    interest: Interest,
    origin: Option<u32>,
    profile: BTreeMap<String, FieldBytes>,
    fault: Option<NetError>,
    records: Vec<TickRecord>,
    record: bool,
    trace: Option<crate::Trace>,
    pending_intents: BTreeMap<u64, Vec<(PlayerId, Intent)>>,
}

/// Encrypted server transport and schema projections around an existing Session.
/// Polling runs bounded fixed ticks; gameplay state remains in Session's ECS.
pub struct Server<F: SimulationFeature> {
    app: App,
    listener: Entity,
    key: lightyear::netcode::Key,
    marker: PhantomData<F>,
    next_update: std::time::Instant,
    poll_interval: Duration,
}
impl<F: SimulationFeature> Server<F> {
    /// Starts a Listen/Dedicated server from the same world/systems used offline.
    pub fn new(session: Session, policies: PolicyRegistry, config: ServerConfig) -> Result<Self> {
        if PolicyRegistry::new(session.schema().clone())?.hash() != policies.hash() {
            return Err(NetError("simulation/network schema mismatch".into()));
        }
        if config.mode == SessionMode::Local
            || config.snapshot_hz == 0
            || config.snapshot_hz > config.hz
            || config.bytes_per_second < 1200
            || config.max_clients == 0
            || config.max_clients > 256
            || config.start_clients > config.max_clients
            || (session.context().dt() - 1.0 / f64::from(config.hz)).abs() > f64::EPSILON
        {
            return Err(NetError("invalid authority configuration".into()));
        }
        let mut app = base::<F>(true, policies.clone(), config.hz)?;
        app.insert_resource(ReplicationMetadata::new(Duration::from_secs_f64(
            1.0 / f64::from(config.snapshot_hz),
        )));
        app.insert_resource(Authority {
            session,
            adapter: Adapter {
                mode: config.mode,
                gate: IntentGate::new(policies.schema().clone()),
                inputs: Vec::new(),
                intents: Vec::new(),
                completed_tick: 0,
            },
            config: config.clone(),
            actors: BTreeMap::new(),
            ids: WorldIds::default(),
            views: BTreeMap::new(),
            ready: BTreeMap::new(),
            issued: BTreeSet::new(),
            interest: Interest::new(16.0)?,
            origin: None,
            profile: BTreeMap::new(),
            fault: None,
            records: Vec::new(),
            record: false,
            trace: None,
            pending_intents: BTreeMap::new(),
        });
        app.add_observer(new_link);
        app.add_systems(bevy_app::Update, receive);
        app.add_systems(FixedUpdate, tick::<F>);
        finish(&mut app);
        let key = lightyear::netcode::generate_key();
        let listener = app
            .world_mut()
            .spawn((
                lightyear::prelude::server::NetcodeServer::new(
                    lightyear::prelude::server::NetcodeConfig {
                        protocol_id: PROTOCOL,
                        private_key: key,
                        max_clients: config.max_clients,
                        client_timeout_secs: 15,
                        additional_expected_addresses: config.expected_addresses,
                        ..Default::default()
                    },
                ),
                LocalAddr(config.bind),
                lightyear::prelude::server::ServerUdpIo,
            ))
            .id();
        app.world_mut()
            .trigger(lightyear::prelude::server::Start { entity: listener });
        app.update();
        let mut this = Self {
            app,
            listener,
            key,
            marker: PhantomData,
            next_update: std::time::Instant::now(),
            poll_interval: Duration::from_secs_f64(1.0 / f64::from(config.hz)),
        };
        this.refresh()?;
        Ok(this)
    }
    /// Bound server address, available after construction.
    pub fn address(&self) -> Result<SocketAddr> {
        self.app
            .world()
            .get::<LocalAddr>(self.listener)
            .map(|a| a.0)
            .ok_or_else(|| NetError("server has no UDP address".into()))
    }
    /// Issues a unique player token for a direct address or an impairment proxy.
    pub fn token(&mut self, player: PlayerId, endpoint: SocketAddr) -> Result<JoinToken> {
        let mut state = self.app.world_mut().resource_mut::<Authority>();
        if state.ready.contains_key(&player)
            || (!state.issued.contains(&player) && state.issued.len() >= state.config.max_clients)
        {
            return Err(NetError("player is connected or server is full".into()));
        }
        if state.config.mode == SessionMode::Listen && player == state.config.local_owner {
            return Err(NetError(
                "listen host uses the direct local endpoint".into(),
            ));
        }
        let token = lightyear::netcode::ConnectToken::build(endpoint, PROTOCOL, player.0, self.key)
            .timeout_seconds(15)
            .expire_seconds(120)
            .generate()
            .map_err(error)?;
        state.issued.insert(player);
        Ok(JoinToken(token.try_into_bytes().map_err(error)?.to_vec()))
    }
    /// Registers an owner/prefab baseline for an existing simulation entity.
    pub fn spawn(
        &mut self,
        identity: &str,
        prefab: &str,
        version: [u8; 32],
        owner: Option<PlayerId>,
        team: Option<u32>,
    ) -> Result<NetId> {
        if identity.len() > 256
            || prefab.len() > 256
            || !prefab.starts_with('@')
            || !prefab.contains('#')
        {
            return Err(NetError("spawn requires a bounded prefab reference".into()));
        }
        self.refresh()?;
        let world = self.app.world_mut();
        let mut state = world
            .remove_resource::<Authority>()
            .expect("authority resource installed");
        let controller = owner.and_then(|owner| state.ready.get(&owner).copied());
        let result = if let Some(actor) = state.actors.get_mut(identity) {
            let ownership_changed = actor.spawn.owner != owner;
            actor.spawn.prefab = prefab.into();
            actor.spawn.version = version;
            actor.spawn.owner = owner;
            actor.spawn.team = team;
            let id = actor.spawn.id;
            let mut entity = world.entity_mut(actor.entity);
            entity.insert((actor.spawn.clone(), PrivateTo(owner)));
            if ownership_changed {
                entity.remove::<ControlledBy>();
                entity.insert((ActionState::<Input>::default(),lightyear::input::input_buffer::InputBuffer::<ActionState<Input>,Input>::default()));
                if let Some(link) = controller {
                    entity.insert(ControlledBy {
                        owner: link,
                        lifetime: Lifetime::Persistent,
                    });
                }
            }
            if let Some(owner) = owner {
                let peer = PeerId::Netcode(owner.0);
                entity.insert((
                    PredictionTarget::to_clients(NetworkTarget::Single(peer)),
                    InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(peer)),
                ));
            } else {
                entity.remove::<PredictionTarget>();
                entity.insert(InterpolationTarget::to_clients(NetworkTarget::All));
            }
            state.interest.update(&actor.spawn, actor.position)?;
            Ok(id)
        } else {
            Err(NetError(format!("unknown simulation identity {identity}")))
        };
        world.insert_resource(state);
        result
    }
    /// Supplies authoritative spatial positions used by the coarse interest grid.
    pub fn position(&mut self, identity: &str, position: [f32; 2]) -> Result<()> {
        let mut state = self.app.world_mut().resource_mut::<Authority>();
        let Authority {
            actors, interest, ..
        } = &mut *state;
        let actor = actors
            .get_mut(identity)
            .ok_or_else(|| NetError("unknown actor".into()))?;
        interest.update(&actor.spawn, position)?;
        actor.position = position;
        Ok(())
    }
    /// Sets authority-owned interest/team/budget policy; budget is fixed at connection creation.
    pub fn view(&mut self, view: ConnectionView) -> Result<()> {
        let applied = self.budgets().get(&view.player).copied();
        let mut state = self.app.world_mut().resource_mut::<Authority>();
        state.interest.visible(&view)?;
        if applied.is_some_and(|budget| budget != view.bytes_per_second) {
            return Err(NetError("change byte budgets before connecting".into()));
        }
        state.views.insert(view.player, view);
        Ok(())
    }
    /// Effective native packet-admission budgets of authenticated connections.
    pub fn budgets(&self) -> BTreeMap<PlayerId, u32> {
        self.app
            .world()
            .iter_entities()
            .filter_map(|e| {
                let PeerId::Netcode(p) = e.get::<RemoteId>()?.0 else {
                    return None;
                };
                Some((PlayerId(p), e.get::<ByteBudget>()?.0))
            })
            .collect()
    }
    /// Exposes a typed authority intent with owner authorization and rate limits.
    pub fn expose(&mut self, expose: crate::Expose) -> Result<()> {
        self.app
            .world_mut()
            .resource_mut::<Authority>()
            .adapter
            .gate
            .expose(expose)
    }
    /// Enables optional recording/hash sampling for deterministic tests and replay.
    pub fn record(&mut self, build: ara_sim::StateHash, link_seeds: Vec<u64>) -> Result<()> {
        let mut state = self.app.world_mut().resource_mut::<Authority>();
        let trace = crate::Trace::begin(&state.session, build, link_seeds)?;
        state.trace = Some(trace);
        state.record = true;
        state.session.set_hash_sampling(true);
        Ok(())
    }
    /// Full arbitrary-initial-state replay with first-field divergence diagnostics.
    pub fn trace(&self) -> Option<&crate::Trace> {
        self.app.world().resource::<Authority>().trace.as_ref()
    }
    /// Recorded applied boundaries, never a source of replica repair.
    pub fn records(&self) -> &[TickRecord] {
        &self.app.world().resource::<Authority>().records
    }
    /// Read-only authority simulation for editor inspection and convergence assertions.
    pub fn session(&self) -> &Session {
        &self.app.world().resource::<Authority>().session
    }
    /// Direct listen-host input. This is injected once, with no network round trip.
    pub fn inject_host(&mut self, tick: u64, input: Injection) -> Result<()> {
        let mut state = self.app.world_mut().resource_mut::<Authority>();
        if state.config.mode != SessionMode::Listen || input.player != state.config.local_owner {
            return Err(NetError("host input requires Listen mode".into()));
        }
        F::input(Some(input.value))?;
        state.session.inject(tick, vec![input])?;
        Ok(())
    }
    /// Submits a listen owner's intent through the same exposure/ownership/rate gate.
    pub fn intent_host(&mut self, intent: Intent) -> Result<()> {
        let mut state = self.app.world_mut().resource_mut::<Authority>();
        if state.config.mode != SessionMode::Listen
            || intent.tick < state.session.tick().saturating_sub(60)
            || intent.tick > state.session.tick().saturating_add(120)
            || intent.payload.len() > 4096
            || intent.target.len() > 256
            || intent.command.len() > 128
            || state.pending_intents.values().map(Vec::len).sum::<usize>() >= 256
        {
            return Err(NetError("invalid listen-host intent".into()));
        }
        let player = state.config.local_owner;
        state.adapter.gate.authenticate(player);
        let assigned = intent.tick.max(state.session.tick() + 1);
        state
            .pending_intents
            .entry(assigned)
            .or_default()
            .push((player, intent));
        Ok(())
    }
    /// Counts ready remote peers; local listen owners are not network peers.
    pub fn connections(&self) -> usize {
        self.app.world().resource::<Authority>().ready.len()
    }
    /// Per-field replication payload accounting (separate from Link's encrypted datagrams).
    pub fn profile(&self) -> &BTreeMap<String, FieldBytes> {
        &self.app.world().resource::<Authority>().profile
    }
    /// Polls nonblocking transport and fixed schedules; errors stop further simulation.
    pub fn poll(&mut self) -> Result<()> {
        let now = std::time::Instant::now();
        if now < self.next_update {
            return Ok(());
        }
        self.next_update = now + self.poll_interval;
        self.app.update();
        if let Some(fault) = &self.app.world().resource::<Authority>().fault {
            return Err(fault.clone());
        }
        Ok(())
    }
    fn refresh(&mut self) -> Result<()> {
        let world = self.app.world_mut();
        let mut state = world
            .remove_resource::<Authority>()
            .expect("authority resource installed");
        let result = refresh::<F>(world, &mut state);
        world.insert_resource(state);
        result
    }
}
