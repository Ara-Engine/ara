//! A networked client's presentation world.
use super::*;
use ara_input::Injection;
use ara_net::{Client, ClientConfig, JoinToken, NetId, PolicyRegistry};
use ara_physics::Velocity;
use ara_runtime::components::FollowCamera;
use ara_schema::EntityRef;
use ara_sim::Identity;
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    time::{Duration, Instant},
};

/// Client-side numbers: join time, prediction error and replication counters.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MirrorStats {
    /// This client's player.
    pub player: u64,
    /// Client creation until the timeline synced and the first authoritative tick
    /// arrived, in ms (connect, handshake, schema check, baseline).
    pub join_ms: Option<f64>,
    /// Latest authoritative tick received.
    pub tick: u64,
    /// Latest predicted tick.
    pub predicted_tick: u64,
    /// Lightyear rollbacks and the ticks they resimulated.
    pub rollbacks: u32,
    /// Ticks resimulated by those rollbacks.
    pub replayed_ticks: u32,
    /// Same-tick prediction checks against authoritative state.
    pub checkpoints: u64,
    /// Checks whose prediction differed.
    pub corrections: usize,
    /// Mean nonzero prediction error, in millimetres.
    pub correction_mean_mm: f64,
    /// Largest prediction error, in millimetres.
    pub correction_max_mm: f64,
    /// Predicted (owned) and interpolated (remote) feature entities.
    pub predicted: usize,
    /// Remote feature entities shown through interpolation.
    pub interpolated: usize,
    /// Replicated entities written into the mirror world.
    pub replicated: usize,
}

/// One generic replicated transform: the newest baseline and when it arrived (the
/// interpolation clock between the previous and current baselines).
struct Baseline {
    current: Value,
    arrived: Instant,
}

/// A client's presentation world: a [`Runtime`] built from the same snapshot that never
/// steps. Each predicted tick, [`NetMirror::frame`] writes the owner's predicted
/// motor, other players' interpolated motors and every other entity's interpolated
/// transform and velocity into it, then runs animation, world poses and cameras.
pub struct NetMirror {
    client: Client<Locomotion>,
    player: PlayerId,
    hz: u32,
    entities: BTreeMap<NetId, Entity>,
    baselines: BTreeMap<NetId, Baseline>,
    motors: BTreeMap<NetId, NetMotor>,
    presented: u64,
    interval: Duration,
    created: Instant,
    join_ms: Option<f64>,
}

impl NetMirror {
    /// Connects `runtime` (built from the same snapshot after [`avatars`], with
    /// [`NetPlugin`]) as `player`'s view: its follow cameras track that player's avatar.
    pub fn connect(
        runtime: &mut Runtime,
        player: u64,
        endpoint: SocketAddr,
        token: JoinToken,
    ) -> Result<Self> {
        let hz = runtime.hz();
        let world = runtime.play_mut().session_mut().world_mut();
        let avatar = world
            .query::<(&Identity, &PlayerControl)>()
            .iter(world)
            .find(|(_, c)| c.player == player)
            .map(|(id, _)| id.0.clone())
            .ok_or_else(|| NetError(format!("the scene has no avatar for player {player}")))?;
        for mut camera in world.query::<&mut FollowCamera>().iter_mut(world) {
            camera.target = Some(EntityRef(avatar.clone()));
            camera.player = player;
        }
        let policies = PolicyRegistry::new(runtime.play().session().schema().clone())?;
        let client = Client::<Locomotion>::new(
            policies,
            ClientConfig {
                player: PlayerId(player),
                endpoint,
                hz,
                tick_limit: None,
            },
            token,
        )?;
        Ok(Self {
            client,
            player: PlayerId(player),
            hz,
            entities: BTreeMap::new(),
            baselines: BTreeMap::new(),
            motors: BTreeMap::new(),
            presented: 0,
            interval: Duration::from_secs_f64(1.0 / f64::from(hz.min(30))),
            created: Instant::now(),
            join_ms: None,
        })
    }

    /// The owner's input, `after` ticks past its next predicted tick. Only `Move` (the
    /// predicted action) is networked; other actions are dropped.
    pub fn input(&mut self, after: u64, inputs: Vec<Injection>) -> Result<()> {
        let tick = self.client.status().predicted_tick + 1 + after;
        for mut input in inputs.into_iter().filter(|i| i.action == MOVE) {
            input.player = self.player;
            self.client.inject(tick, input)?;
        }
        Ok(())
    }

    /// This client's player.
    pub fn player(&self) -> PlayerId {
        self.player
    }

    /// Whether the timeline synced with the authority (the join is complete).
    pub fn synced(&mut self) -> bool {
        self.client.status().synced
    }

    /// Bot input: `source(tick, player)` is sampled for each new predicted tick (never
    /// during rollback replays).
    pub fn input_source(&mut self, source: fn(u64, PlayerId) -> Option<ActionValue>) {
        self.client.input_source(source);
    }

    /// One frame: polls the client; for each newly predicted tick, writes replicated
    /// state into `runtime` and advances its presentation (at most four per frame).
    pub fn frame(&mut self, runtime: &mut Runtime) -> Result<()> {
        self.client.poll()?;
        let status = self.client.status();
        if self.join_ms.is_none() && status.synced && status.tick > 0 {
            self.join_ms = Some(self.created.elapsed().as_secs_f64() * 1000.0);
        }
        let ticks = status.predicted_tick.saturating_sub(self.presented);
        if ticks == 0 {
            return Ok(());
        }
        self.presented = status.predicted_tick;
        self.apply(runtime, ticks)?;
        for _ in 0..ticks.min(4) {
            runtime
                .present_mirror(status.tick)
                .map_err(|e| NetError(e.0))?;
        }
        Ok(())
    }

    /// The latest authoritative tick received.
    pub fn tick(&mut self) -> u64 {
        self.client.status().tick
    }

    /// Join, prediction and replication numbers so far.
    pub fn stats(&mut self) -> MirrorStats {
        let status = self.client.status();
        let errors = &status.corrections;
        MirrorStats {
            player: self.player.0,
            join_ms: self.join_ms,
            tick: status.tick,
            predicted_tick: status.predicted_tick,
            rollbacks: status.rollbacks,
            replayed_ticks: status.replayed_ticks,
            checkpoints: status.checkpoints,
            corrections: errors.len(),
            correction_mean_mm: if errors.is_empty() {
                0.0
            } else {
                errors.iter().sum::<f64>() / errors.len() as f64
            },
            correction_max_mm: errors.iter().copied().fold(0.0, f64::max),
            predicted: status.predicted,
            interpolated: status.interpolated,
            replicated: self.entities.len(),
        }
    }

    fn apply(&mut self, runtime: &mut Runtime, ticks: u64) -> Result<()> {
        let spawns = self.client.spawns();
        let world = runtime.play_mut().session_mut().world_mut();
        if spawns.iter().any(|s| !self.entities.contains_key(&s.id)) {
            let by_identity: BTreeMap<String, Entity> = world
                .query::<(Entity, &Identity)>()
                .iter(world)
                .map(|(e, id)| (id.0.clone(), e))
                .collect();
            self.entities = spawns
                .iter()
                .filter_map(|s| Some((s.id, *by_identity.get(&s.identity)?)))
                .collect();
        }
        let now = Instant::now();
        for spawn in &spawns {
            let Some(&entity) = self.entities.get(&spawn.id) else {
                continue;
            };
            if let Some(current) = self.client.component(spawn.id, "Transform")? {
                let baseline = self.baselines.entry(spawn.id).or_insert(Baseline {
                    current: current.clone(),
                    arrived: now - self.interval,
                });
                if baseline.current != current {
                    baseline.current = current;
                    baseline.arrived = now;
                }
                let alpha =
                    ((now - baseline.arrived).as_secs_f32() / self.interval.as_secs_f32()).min(1.0);
                if let Some(sample) = self.client.sample_component(spawn.id, "Transform", alpha)?
                    && let Some(mut transform) = world.get_mut::<Transform>(entity)
                {
                    write(&mut transform, &sample);
                }
            }
            if let Some(velocity) = self.client.component(spawn.id, "Velocity")?
                && let Ok(linear) = serde_json::from_value::<[f32; 3]>(velocity["linear"].clone())
            {
                set_velocity(world, entity, linear);
            }
            if let Some(motor) = self.client.view(spawn.id) {
                let previous = self.motors.insert(spawn.id, motor).unwrap_or(motor);
                let rate = self.hz as f32 / ticks as f32 / 1000.0;
                set_velocity(
                    world,
                    entity,
                    [
                        (motor.x - previous.x) as f32 * rate,
                        0.0,
                        (motor.z - previous.z) as f32 * rate,
                    ],
                );
                let [x, z] = motor.ground();
                if let Some(mut transform) = world.get_mut::<Transform>(entity) {
                    transform.position[0] = x;
                    transform.position[2] = z;
                    transform.rotation = [0.0, motor.yaw(), 0.0];
                }
                if let Some(mut control) = world.get_mut::<PlayerControl>(entity) {
                    control.yaw = motor.yaw();
                }
            }
        }
        Ok(())
    }
}

/// Writes a sampled (possibly partial) `Transform` projection: absent fields keep
/// their authored values (scale is not replicated).
fn write(transform: &mut Transform, sample: &Value) {
    if let Ok(position) = serde_json::from_value::<[f32; 3]>(sample["position"].clone()) {
        transform.position = position;
    }
    if let Ok(rotation) = serde_json::from_value::<[f32; 3]>(sample["rotation"].clone()) {
        transform.rotation = rotation;
    }
}

/// Sets an entity's presented velocity (animation's ground speed), inserting it.
fn set_velocity(world: &mut World, entity: Entity, linear: [f32; 3]) {
    match world.get_mut::<Velocity>(entity) {
        Some(mut velocity) => velocity.linear = linear,
        None => {
            world.entity_mut(entity).insert(Velocity {
                linear,
                ..Default::default()
            });
        }
    }
}
