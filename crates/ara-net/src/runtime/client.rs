mod systems;
use super::*;
use lightyear::prediction::{
    diagnostics::PredictionMetrics, predicted_history::PredictionHistory, rollback::RollbackSystems,
};
use std::marker::PhantomData;
use systems::*;

/// Renderer-free client configuration, shared by players and headless bots.
pub struct ClientConfig {
    /// Authenticated input owner associated with the join token.
    pub player: PlayerId,
    /// Direct server or per-client conditioner endpoint.
    pub endpoint: SocketAddr,
    /// Fixed tick frequency matching the authority.
    pub hz: u32,
    /// Optional test boundary after which input/prediction stops.
    pub tick_limit: Option<u64>,
}
/// Inspectable prediction, interpolation, privacy and timeline counters.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClientStatus {
    /// Most recently received authoritative simulation tick.
    pub tick: u64,
    /// Most recently sampled native prediction tick, in game-relative ticks.
    pub predicted_tick: u64,
    /// Whether native timeline synchronization completed.
    pub synced: bool,
    /// Native rollback events.
    pub rollbacks: u32,
    /// Native resimulated ticks.
    pub replayed_ticks: u32,
    /// Same-tick nonzero corrections in feature units.
    pub corrections: Vec<f64>,
    /// Same-tick native history comparisons.
    pub checkpoints: u64,
    /// Number of predicted owner entities.
    pub predicted: usize,
    /// Number of interpolated remote entities.
    pub interpolated: usize,
    /// Private projections received and inspected.
    pub private_samples: u64,
    /// Foreign-owner private projections (always causes poll to fail).
    pub privacy_leaks: u64,
    /// References awaiting an out-of-order or interest-enter baseline.
    pub pending_references: usize,
}
#[derive(Resource)]
struct Owner {
    player: PlayerId,
    origin: Option<u32>,
    ready: bool,
    limit: Option<u64>,
    inputs: BTreeMap<(u64, String), Injection>,
    source: Option<fn(u64, PlayerId) -> Option<ActionValue>>,
    last_checked: BTreeMap<NetId, u32>,
    status: ClientStatus,
    fault: Option<NetError>,
    references: crate::DeferredReferences,
    visible: BTreeSet<NetId>,
    field_views: BTreeMap<NetId, FieldView>,
}
struct FieldView {
    previous: Projection,
    current: Projection,
}
#[derive(Resource)]
struct Perturb<F: SimulationFeature> {
    tick: u64,
    run: fn(&mut F::State),
    applied: bool,
}

/// Encrypted native client with selective owner rollback and remote interpolation.
/// Game/device/bot input goes through Ara actions, never backend types.
pub struct Client<F: SimulationFeature> {
    app: App,
    marker: PhantomData<F>,
    next_update: std::time::Instant,
    poll_interval: Duration,
}
impl<F: SimulationFeature> Client<F> {
    /// Connects with a trusted short-lived token and matching schema registry.
    pub fn new(policies: PolicyRegistry, config: ClientConfig, token: JoinToken) -> Result<Self> {
        let mut app = base::<F>(false, policies, config.hz)?;
        let input_delay = lightyear::prelude::client::InputDelayConfig::no_input_delay();
        let mut prediction = PredictionManager::default();
        // Retain every tick the input timeline is allowed to predict. Otherwise
        // delayed confirmations arrive after native rollback has pruned their history.
        prediction.rollback_policy.max_rollback_ticks = input_delay.maximum_predicted_ticks;
        app.insert_resource(prediction);
        app.insert_resource(InputTimelineConfig::new(
            SyncConfig {
                jitter_margin: 6.0,
                ..Default::default()
            },
            input_delay,
        ));
        app.insert_resource(Owner {
            player: config.player,
            origin: None,
            ready: false,
            limit: config.tick_limit,
            inputs: BTreeMap::new(),
            source: None,
            last_checked: BTreeMap::new(),
            status: ClientStatus::default(),
            fault: None,
            references: crate::DeferredReferences::new(4096),
            visible: BTreeSet::new(),
            field_views: BTreeMap::new(),
        });
        app.add_observer(controlled);
        app.add_systems(
            FixedPreUpdate,
            inputs::<F>.in_set(lightyear::input::client::InputSystems::WriteClientInputs),
        );
        app.add_systems(FixedUpdate, movement::<F>);
        app.add_systems(
            PreUpdate,
            corrections::<F>
                .after(ReplicationSystems::Receive)
                .before(RollbackSystems::Check),
        );
        app.add_systems(bevy_app::Update, observe);
        finish(&mut app);
        let token = lightyear::netcode::ConnectToken::try_from_bytes(&token.0).map_err(error)?;
        let link = app
            .world_mut()
            .spawn((
                lightyear::prelude::client::Client,
                ReplicationReceiver,
                LocalAddr(if config.endpoint.is_ipv6() {
                    SocketAddr::from(([0_u16; 8], 0))
                } else {
                    SocketAddr::from(([0, 0, 0, 0], 0))
                }),
                PeerAddr(config.endpoint),
                lightyear::prelude::client::NetcodeClient::new(
                    lightyear::netcode::auth::Authentication::Token(token),
                    lightyear::prelude::client::NetcodeConfig {
                        client_timeout_secs: 15,
                        ..Default::default()
                    },
                )
                .map_err(error)?,
                UdpIo::default(),
            ))
            .id();
        app.world_mut().trigger(Connect { entity: link });
        Ok(Self {
            app,
            marker: PhantomData,
            next_update: std::time::Instant::now(),
            poll_interval: Duration::from_secs_f64(1.0 / f64::from(config.hz)),
        })
    }
    /// Supplies immutable bot/action logic; only newly sampled inputs call this.
    /// Native rollback reads its input history rather than calling the source again.
    pub fn input_source(&mut self, source: fn(u64, PlayerId) -> Option<ActionValue>) {
        self.app.world_mut().resource_mut::<Owner>().source = Some(source);
    }
    /// Installs a shared multi-action codec before the client starts polling.
    pub fn action_codec(&mut self, codec: crate::ActionCodec<F::Input>) {
        self.app.insert_resource(codec);
    }
    /// Queues bounded headless/device action injection at an explicit game tick.
    pub fn inject(&mut self, tick: u64, input: Injection) -> Result<()> {
        let codec = self
            .app
            .world()
            .get_resource::<crate::ActionCodec<F::Input>>();
        if let Some(codec) = codec {
            if !codec.names.contains(&input.action.as_str()) {
                return Err(NetError("undeclared predicted action".into()));
            }
            (codec.sample)(&BTreeMap::from([(input.action.clone(), input.value)]))?;
        } else {
            if input.action != F::ACTION {
                return Err(NetError("undeclared predicted action".into()));
            }
            F::input(Some(input.value))?;
        }
        let mut owner = self.app.world_mut().resource_mut::<Owner>();
        if input.player != owner.player
            || input.hold_ticks == 0
            || tick.checked_add(u64::from(input.hold_ticks)).is_none()
            || owner.inputs.len() >= 2048
            || tick <= owner.status.predicted_tick
            || owner.limit.is_some_and(|limit| tick > limit)
        {
            return Err(NetError("invalid/bounded client action injection".into()));
        }
        owner.inputs.insert((tick, input.action.clone()), input);
        Ok(())
    }
    /// Sends only a typed intent. There is no editor-op/state-write message route.
    pub fn intent(&mut self, intent: Intent) -> Result<()> {
        if intent.payload.len() > 4096 || intent.target.len() > 256 || intent.command.len() > 128 {
            return Err(NetError("intent exceeds wire bounds".into()));
        }
        let world = self.app.world_mut();
        let mut q = world.query_filtered::<&mut MessageSender<ToServer>, With<Connected>>();
        let Some(mut sender) = q.iter_mut(world).next() else {
            return Err(NetError("client is not connected".into()));
        };
        sender.send::<Control>(ToServer::Intent(intent));
        Ok(())
    }
    /// Encodes a registered typed gameplay command as an owner intent.
    pub fn submit<C: ara_sim::SimCommand>(
        &mut self,
        tick: u64,
        sequence: u64,
        target: &str,
        value: &C,
    ) -> Result<()> {
        let player = self.app.world().resource::<Owner>().player;
        value.validate(&ara_sim::CommandContext {
            player,
            sequence,
            target: Some(target.into()),
        })?;
        self.intent(Intent {
            tick,
            sequence,
            command: C::NAME.into(),
            target: target.into(),
            payload: serde_json::to_vec(value)?,
        })
    }
    /// Polls transport/prediction and fails on any observed privacy leak.
    pub fn poll(&mut self) -> Result<()> {
        let now = std::time::Instant::now();
        if now < self.next_update {
            return Ok(());
        }
        self.next_update = now + self.poll_interval;
        self.app.update();
        if let Some(e) = &self.app.world().resource::<Owner>().fault {
            return Err(e.clone());
        }
        Ok(())
    }
    /// Advances transport and prediction with caller-supplied elapsed time.
    /// Drive every peer and its links from one clock; do not mix with [`Self::poll`].
    pub fn advance(&mut self, elapsed: Duration) -> Result<()> {
        super::advance(&mut self.app, elapsed);
        if let Some(fault) = &self.app.world().resource::<Owner>().fault {
            return Err(fault.clone());
        }
        Ok(())
    }
    /// Timeline/rollback/interpolation diagnostics without exposing backend types.
    pub fn status(&mut self) -> ClientStatus {
        let world = self.app.world_mut();
        let mut result = world.resource::<Owner>().status.clone();
        result.synced = world.resource::<LocalTimelineSync>().is_synced();
        let metrics = world.resource::<PredictionMetrics>();
        result.rollbacks = metrics.rollbacks;
        result.replayed_ticks = metrics.rollback_ticks;
        result.predicted = world
            .query_filtered::<Entity, With<Predicted>>()
            .iter(world)
            .count();
        result.interpolated = world
            .query_filtered::<Entity, With<Interpolated>>()
            .iter(world)
            .count();
        result
    }
    /// Received prefab/lifetime baselines, including late joins and interest transitions.
    pub fn spawns(&mut self) -> Vec<Spawn> {
        let world = self.app.world_mut();
        world.query::<&Spawn>().iter(world).cloned().collect()
    }
    /// Resolves a network reference only while its exact epoch has a received baseline.
    pub fn resolve_reference(&mut self, id: NetId) -> Option<Spawn> {
        if !self.app.world().resource::<Owner>().references.is_live(id) {
            return None;
        }
        self.spawns().into_iter().find(|s| s.id == id)
    }
    /// Authoritative same-tick feature state; prediction/rendering never repair this value.
    pub fn state(&mut self, id: NetId) -> Option<F::State> {
        let world = self.app.world_mut();
        world
            .query::<(
                &Spawn,
                Option<&ConfirmedHistory<F::State>>,
                Option<&F::State>,
            )>()
            .iter(world)
            .find(|(s, _, _)| s.id == id)
            .and_then(|(_, history, state)| {
                history
                    .and_then(ConfirmedHistory::newest_present)
                    .map(|(_, v)| v.clone())
                    .or_else(|| state.cloned())
            })
    }
    /// Current owner-predicted or remote-interpolated presentation state.
    pub fn view(&mut self, id: NetId) -> Option<F::State> {
        let world = self.app.world_mut();
        world
            .query::<(&Spawn, &F::State)>()
            .iter(world)
            .find(|(s, _)| s.id == id)
            .map(|(_, s)| s.clone())
    }
    /// Decodes only received fields. Private/authority-only fields stay absent.
    pub fn component(&mut self, id: NetId, component: &str) -> Result<Option<serde_json::Value>> {
        let world = self.app.world_mut();
        let fields = world
            .query::<(&Spawn, Option<&PublicFields>, Option<&OwnerFields>)>()
            .iter(world)
            .find(|(s, _, _)| s.id == id)
            .map(|(_, public, private)| {
                let mut p = public.map(|p| p.0.clone()).unwrap_or_default();
                if let Some(private) = private {
                    p.0.extend(private.0.0.clone());
                }
                p
            });
        fields
            .map(|p| world.resource::<Policies>().0.component(&p, component))
            .transpose()
    }
    /// Samples two received generic-field baselines using schema interpolation.
    /// Native feature views use `view`; this path handles other reflected components.
    pub fn sample_component(
        &self,
        id: NetId,
        component: &str,
        alpha: f32,
    ) -> Result<Option<serde_json::Value>> {
        let world = self.app.world();
        let owner = world.resource::<Owner>();
        let Some(view) = owner.field_views.get(&id) else {
            return Ok(None);
        };
        let policies = &world.resource::<Policies>().0;
        Ok(Some(policies.component(
            &policies.interpolate(&view.previous, &view.current, alpha),
            component,
        )?))
    }
    /// Forces a reproducible prediction-only error for reconciliation diagnostics.
    pub fn perturb_prediction(&mut self, tick: u64, run: fn(&mut F::State)) {
        self.app.insert_resource(Perturb::<F> {
            tick,
            run,
            applied: false,
        });
    }
}
