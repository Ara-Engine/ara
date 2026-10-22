//! Real UDP server and bot processes. Gameplay mutations occur only in Simulation::step.

use crate::{
    adapter::Snapshot,
    gameplay::{Frame, State},
    local,
};
#[cfg(feature = "renet-stack")]
use crate::{
    adapter::{self, Prediction, Registry, RemoteView},
    gameplay::{self, Command, MoveInput, Simulation},
};
#[cfg(feature = "renet-stack")]
use renet::{DefaultChannel, RenetClient, RenetServer};
#[cfg(feature = "renet-stack")]
use renet_netcode::{
    ClientAuthentication, ConnectToken, NetcodeClientTransport, NetcodeServerTransport,
    ServerAuthentication, ServerConfig, generate_random_bytes,
};
use serde::{Deserialize, Serialize};
#[cfg(feature = "renet-stack")]
use std::{
    collections::{BTreeMap, BTreeSet},
    net::UdpSocket,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use std::{fs, net::SocketAddr, path::Path};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[cfg(feature = "renet-stack")]
const PROTOCOL: u64 = 0xa4a0_0004;
pub const TICKS: u32 = 600;
#[cfg(feature = "renet-stack")]
const STEP: Duration = Duration::from_nanos(1_000_000_000 / 60);
#[cfg(feature = "renet-stack")]
const UNRELIABLE: u8 = DefaultChannel::Unreliable as u8;
#[cfg(feature = "renet-stack")]
const RELIABLE: u8 = DefaultChannel::ReliableOrdered as u8;

/// Backend choice is session configuration, never gameplay policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stack {
    #[default]
    Renet,
    Lightyear,
}

pub fn run_server(directory: &Path) -> Result<()> {
    match read_json::<Config>(directory.join("config.json"))?.stack {
        #[cfg(feature = "renet-stack")]
        Stack::Renet => server(directory),
        #[cfg(feature = "lightyear-stack")]
        Stack::Lightyear => crate::ly::server(directory),
        #[allow(unreachable_patterns)]
        _ => Err("requested stack is not compiled into this binary".into()),
    }
}
pub fn run_client(directory: &Path, player: usize) -> Result<()> {
    match read_json::<Config>(directory.join("config.json"))?.stack {
        #[cfg(feature = "renet-stack")]
        Stack::Renet => client(directory, player),
        #[cfg(feature = "lightyear-stack")]
        Stack::Lightyear => crate::ly::client(directory, player),
        #[allow(unreachable_patterns)]
        _ => Err("requested stack is not compiled into this binary".into()),
    }
}

#[derive(Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub stack: Stack,
    pub proxies: Vec<SocketAddr>,
    pub listen: bool,
    pub clients: usize,
    pub late: bool,
    pub seed: u64,
    pub impaired: bool,
    pub prediction_lead_ticks: u32,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[cfg(feature = "renet-stack")]
struct InputSample {
    tick: u32,
    input: MoveInput,
}

#[derive(Serialize, Deserialize)]
#[cfg(feature = "renet-stack")]
enum ClientMessage {
    Ready,
    Inputs(Vec<InputSample>),
    Intent { tick: u32, command: Command },
    FinalAck,
}

#[derive(Serialize, Deserialize)]
#[cfg(feature = "renet-stack")]
enum ServerMessage {
    Baseline(Snapshot),
    Snapshot(Snapshot),
    Final(Snapshot),
}

#[derive(Serialize, Deserialize)]
pub struct Replay {
    pub format: u32,
    pub schema: String,
    pub frames: Vec<Frame>,
    pub checkpoints: Vec<State>,
}

#[derive(Serialize, Deserialize)]
pub struct ServerReport {
    pub listen: bool,
    pub clients: usize,
    pub ticks: u32,
    pub entities_replicated: usize,
    pub state: State,
    pub state_hash: String,
    pub replay_hash: String,
    pub script_hash: String,
    pub exact_script_match: bool,
    pub replay_tick_checkpoints: usize,
    pub accepted_commands: u32,
    pub rejected_commands: u32,
    pub late_inputs: u32,
    pub late_commands_scheduled: u32,
    pub nonzero_input_ticks: u32,
    pub bytes_encoded: Option<u64>,
    pub host_direct_input_ticks: u32,
    pub host_projection: Option<Snapshot>,
    pub all_final_acks: bool,
    pub elapsed_seconds: f64,
}

#[derive(Serialize, Deserialize)]
pub struct NativeDiagnostics {
    pub rollback_events: u32,
    pub replayed_ticks: u32,
    pub faults_injected: u32,
    pub predicted_entities: usize,
    pub interpolated_entities: usize,
    pub prediction_histories: usize,
    pub private_inventory_components: usize,
    pub world_resources: usize,
    pub world_entities: usize,
}

#[derive(Serialize, Deserialize)]
pub struct ClientReport {
    pub player: usize,
    pub state: Snapshot,
    pub snapshots: u32,
    pub first_baseline_tick: u32,
    pub baseline_door_open: bool,
    pub baseline_collected_items: usize,
    pub foreign_inventory_values: u32,
    pub inventory_values_received: u32,
    pub checked_prediction_checkpoints: u32,
    pub corrections_mm: Vec<f64>,
    pub resimulated_ticks: u32,
    pub final_predicted_motor: crate::gameplay::Motor,
    pub interpolation_samples: u32,
    pub bytes_encoded: Option<u64>,
    #[serde(default)]
    pub native: Option<NativeDiagnostics>,
}

pub fn write_json(path: impl AsRef<Path>, value: &impl Serialize) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

pub fn read_json<T: for<'a> Deserialize<'a>>(path: impl AsRef<Path>) -> Result<T> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

#[cfg(feature = "renet-stack")]
fn now() -> Result<Duration> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?)
}

/// One authority world; listen's local endpoint submits the same typed action frames.
#[cfg(feature = "renet-stack")]
pub fn server(directory: &Path) -> Result<()> {
    let config: Config = read_json(directory.join("config.json"))?;
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    let address = socket.local_addr()?;
    let private_key = generate_random_bytes();
    let current_time = now()?;
    for (id, proxy) in config.proxies.iter().enumerate() {
        let token = ConnectToken::generate(
            current_time,
            PROTOCOL,
            120,
            id as u64,
            15,
            vec![*proxy],
            None,
            &private_key,
        )?;
        token.write(&mut fs::File::create(
            directory.join(format!("token-{id}.bin")),
        )?)?;
    }
    let mut transport = NetcodeServerTransport::new(
        ServerConfig {
            current_time,
            max_clients: 4,
            protocol_id: PROTOCOL,
            public_addresses: config.proxies,
            authentication: ServerAuthentication::Secure { private_key },
        },
        socket,
    )?;
    let mut network = RenetServer::new(Default::default());
    let mut registry = Registry::new();
    registry.validate()?;
    write_json(directory.join("ready.json"), &address)?;
    let mut simulation = Simulation::new();
    let mut ready = BTreeSet::new();
    let mut final_acks = BTreeSet::new();
    let mut inputs = BTreeMap::<(u32, usize), MoveInput>::new();
    let mut commands = BTreeMap::<u32, BTreeMap<(u8, u32), Command>>::new();
    let mut frames = Vec::new();
    let mut checkpoints = Vec::new();
    let mut accepted = 0;
    let mut rejected = 0;
    let mut late_inputs = 0;
    let mut late_commands_scheduled = 0;
    let mut final_sent = false;
    let boot = Instant::now();
    let mut last = boot;
    let mut started = None;
    let mut next_tick = boot;
    let mut next_flush = boot;
    loop {
        let instant = Instant::now();
        if instant.duration_since(boot) > Duration::from_secs(30) {
            return Err("server timed out waiting for clients/final acks".into());
        }
        let elapsed = instant.duration_since(last);
        last = instant;
        network.update(elapsed);
        transport.update(elapsed, &mut network)?;
        let mut clients = network.clients_id();
        clients.sort_unstable();
        for client in clients {
            let player = usize::try_from(client)?;
            if player >= config.clients || (config.listen && player == 0) {
                network.disconnect(client);
                continue;
            }
            for channel in [RELIABLE, UNRELIABLE] {
                while let Some(bytes) = network.receive_message(client, channel) {
                    let message: ClientMessage = adapter::decode(&bytes)?;
                    match message {
                        ClientMessage::Ready => {
                            if ready.insert(player) && started.is_some() {
                                let baseline = registry.project(&simulation.capture(), player);
                                let bytes = registry.encode(&ServerMessage::Baseline(baseline))?;
                                network.send_message(client, RELIABLE, bytes);
                            }
                        }
                        ClientMessage::Inputs(samples) => {
                            if samples.len() > 32 {
                                return Err("input batch exceeded bound".into());
                            }
                            for sample in samples {
                                if sample.tick <= simulation.tick() {
                                    late_inputs += 1;
                                    continue;
                                }
                                if sample.tick > simulation.tick() + 120
                                    || !(-1..=1).contains(&sample.input.x)
                                    || !(-1..=1).contains(&sample.input.z)
                                {
                                    continue;
                                }
                                inputs.insert((sample.tick, player), sample.input);
                            }
                        }
                        ClientMessage::Intent { tick, mut command } => {
                            // Never authorize the claimed caller in a payload.
                            command.caller = player as u8;
                            if tick >= simulation.tick().saturating_sub(60)
                                && tick <= simulation.tick() + 120
                                && simulation.tick() < TICKS
                            {
                                let assigned_tick = tick.max(simulation.tick() + 1);
                                late_commands_scheduled += u32::from(assigned_tick != tick);
                                let queue = commands.entry(assigned_tick).or_default();
                                if queue.len() < 64 {
                                    queue
                                        .entry((command.caller, command.sequence))
                                        .or_insert(command);
                                }
                            } else {
                                rejected += 1;
                            }
                        }
                        ClientMessage::FinalAck => {
                            final_acks.insert(player);
                        }
                    }
                }
            }
        }
        let initial_count = config.clients - usize::from(config.listen) - usize::from(config.late);
        if started.is_none() && ready.len() >= initial_count {
            started = Some(instant);
            next_tick = instant + STEP;
            // Start all initial owner clocks at the authority barrier, not during handshakes.
            for &player in &ready {
                let bytes = registry.encode(&ServerMessage::Baseline(
                    registry.project(&simulation.capture(), player),
                ))?;
                network.send_message(player as u64, RELIABLE, bytes);
            }
            write_json(directory.join("started.json"), &true)?;
        }
        if started.is_some() && !final_sent {
            while instant >= next_tick && simulation.tick() < TICKS {
                let tick = simulation.tick() + 1;
                let mut frame = Frame {
                    tick,
                    inputs: [MoveInput::default(); 4],
                    commands: Vec::new(),
                };
                for (player, input) in frame.inputs.iter_mut().enumerate().take(config.clients) {
                    *input = inputs.remove(&(tick, player)).unwrap_or_default();
                }
                if let Some(queued) = commands.remove(&tick) {
                    frame.commands.extend(queued.into_values());
                }
                if config.listen {
                    let host = gameplay::scripted_frame(tick, 1);
                    frame.inputs[0] = host.inputs[0];
                    frame.commands.extend(host.commands);
                    frame
                        .commands
                        .sort_by_key(|command| (command.caller, command.sequence));
                }
                let outcome = simulation.step(&frame);
                accepted += outcome.accepted;
                rejected += outcome.rejected;
                frames.push(frame);
                checkpoints.push(simulation.capture());
                if tick.is_multiple_of(3) {
                    let state = checkpoints
                        .last()
                        .expect("one checkpoint per applied frame");
                    for &player in &ready {
                        let snapshot = registry.project(state, player);
                        let bytes = registry.encode(&ServerMessage::Snapshot(snapshot))?;
                        network.send_message(player as u64, UNRELIABLE, bytes);
                    }
                }
                next_tick += STEP;
            }
            if simulation.tick() == TICKS {
                for &player in &ready {
                    let final_state = registry
                        .project(checkpoints.last().expect("final checkpoint exists"), player);
                    let bytes = registry.encode(&ServerMessage::Final(final_state))?;
                    network.send_message(player as u64, RELIABLE, bytes);
                }
                final_sent = true;
            }
        }
        if instant >= next_flush {
            transport.send_packets(&mut network);
            next_flush = instant + STEP;
        }
        if final_sent && final_acks.len() == config.clients - usize::from(config.listen) {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    let state = simulation.capture();
    let replay_hash = local::replay(&frames, &checkpoints)?;
    let mut scripted = Simulation::new();
    for tick in 1..=TICKS {
        scripted.step(&gameplay::scripted_frame(tick, config.clients));
    }
    let scripted = scripted.capture();
    let report = ServerReport {
        listen: config.listen,
        clients: config.clients,
        ticks: TICKS,
        entities_replicated: gameplay::ENTITIES,
        state_hash: format!("{:016x}", gameplay::hash(&state)),
        replay_hash: format!("{replay_hash:016x}"),
        script_hash: format!("{:016x}", gameplay::hash(&scripted)),
        exact_script_match: state == scripted,
        replay_tick_checkpoints: checkpoints.len(),
        accepted_commands: accepted,
        rejected_commands: rejected,
        late_inputs,
        late_commands_scheduled,
        nonzero_input_ticks: frames
            .iter()
            .flat_map(|frame| frame.inputs)
            .filter(|input| *input != MoveInput::default())
            .count() as u32,
        bytes_encoded: Some(registry.bytes_encoded),
        host_direct_input_ticks: if config.listen { TICKS } else { 0 },
        host_projection: config.listen.then(|| registry.project(&state, 0)),
        state,
        all_final_acks: true,
        elapsed_seconds: started.expect("simulation started").elapsed().as_secs_f64(),
    };
    write_json(
        directory.join("replay.json"),
        &Replay {
            format: 1,
            schema: "s4:Motor1/Door2/Inventory3/Item4:v1".into(),
            frames,
            checkpoints,
        },
    )?;
    write_json(directory.join("server.json"), &report)?;
    // Peers have already acknowledged the final checkpoint. Closing sockets ends the run.
    Ok(())
}

/// A headless owner endpoint with input history, resimulation and remote interpolation.
#[cfg(feature = "renet-stack")]
pub fn client(directory: &Path, player: usize) -> Result<()> {
    let config: Config = read_json(directory.join("config.json"))?;
    let lead = config.prediction_lead_ticks;
    let token = ConnectToken::read(&mut fs::File::open(
        directory.join(format!("token-{player}.bin")),
    )?)?;
    let mut transport = NetcodeClientTransport::new(
        now()?,
        ClientAuthentication::Secure {
            connect_token: token,
        },
        UdpSocket::bind("127.0.0.1:0")?,
    )?;
    let mut network = RenetClient::new(Default::default());
    let mut registry = Registry::new();
    registry.validate()?;
    let mut prediction = Prediction::new();
    let mut view = RemoteView::new();
    let mut sent_ready = false;
    let mut next_command = 1;
    let mut first_baseline = None;
    let mut snapshot_count = 0;
    let mut inventories_received = 0;
    let mut interpolation_samples = 0;
    let mut timeline = None;
    let mut confirmed_tick = None;
    let mut next_send = Instant::now();
    let mut next_flush = next_send;
    let boot = Instant::now();
    let mut last = boot;
    let mut final_state = None;
    let mut exit_at = None;
    loop {
        let instant = Instant::now();
        if instant.duration_since(boot) > Duration::from_secs(30) {
            return Err(format!("client {player} timed out").into());
        }
        let elapsed = instant.duration_since(last);
        last = instant;
        network.update(elapsed);
        transport.update(elapsed, &mut network)?;
        if network.is_connected() && !sent_ready {
            network.send_message(RELIABLE, registry.encode(&ClientMessage::Ready)?);
            sent_ready = true;
        }
        for channel in [RELIABLE, UNRELIABLE] {
            while let Some(bytes) = network.receive_message(channel) {
                let message: ServerMessage = adapter::decode(&bytes)?;
                let (snapshot, baseline, final_message) = match message {
                    ServerMessage::Baseline(snapshot) => (snapshot, true, false),
                    ServerMessage::Snapshot(snapshot) => (snapshot, false, false),
                    ServerMessage::Final(snapshot) => (snapshot, false, true),
                };
                if usize::from(snapshot.inventory.owner) != player {
                    return Err(format!(
                        "privacy breach: client {player} received owner {} at tick {}",
                        snapshot.inventory.owner, snapshot.tick
                    )
                    .into());
                }
                if snapshot.items.len() != 45 {
                    return Err("invalid baseline item count".into());
                }
                inventories_received += 1;
                if baseline {
                    next_command = snapshot.tick + 1;
                    first_baseline = Some((
                        snapshot.tick,
                        snapshot.door.open,
                        snapshot.items.iter().filter(|item| !item.available).count(),
                    ));
                    timeline = Some((instant, snapshot.tick + lead));
                }
                // Discard stale/reordered authoritative checkpoints before restoring history.
                if first_baseline.is_none() || final_state.is_some() {
                    continue;
                }
                if confirmed_tick.is_some_and(|tick| snapshot.tick <= tick) && !final_message {
                    continue;
                }
                confirmed_tick = Some(snapshot.tick);
                if let Some((origin, initial_tick)) = timeline {
                    let estimated =
                        initial_tick + (instant.duration_since(origin).as_secs_f64() * 60.0) as u32;
                    if snapshot.tick + lead > estimated {
                        timeline = Some((instant, snapshot.tick + lead));
                    }
                }
                prediction.reconcile(snapshot.tick, snapshot.motors[player]);
                snapshot_count += 1;
                if final_message {
                    final_state = Some(snapshot);
                    network.send_message(RELIABLE, registry.encode(&ClientMessage::FinalAck)?);
                    exit_at = Some(instant + Duration::from_millis(500));
                } else {
                    view.push(snapshot);
                }
            }
        }
        if let Some((origin, initial_tick)) = timeline {
            let target = (initial_tick
                + (instant.duration_since(origin).as_secs_f64() * 60.0) as u32)
                .min(TICKS);
            while prediction.tick < target {
                let tick = prediction.tick + 1;
                prediction.advance(tick, gameplay::scripted_frame(tick, 4).inputs[player]);
            }
            if instant >= next_send && final_state.is_none() {
                // Send sampled inputs only: no knowledge of the bot's future actions.
                let through = prediction.tick.min(TICKS);
                let from = through.saturating_sub(23).max(1);
                let samples = (from..=through)
                    .map(|tick| InputSample {
                        tick,
                        input: gameplay::scripted_frame(tick, 4).inputs[player],
                    })
                    .collect();
                network.send_message(
                    UNRELIABLE,
                    registry.encode(&ClientMessage::Inputs(samples))?,
                );
                while next_command <= through {
                    for command in gameplay::scripted_frame(next_command, 4).commands {
                        if usize::from(command.caller) == player {
                            network.send_message(
                                RELIABLE,
                                registry.encode(&ClientMessage::Intent {
                                    tick: next_command,
                                    command,
                                })?,
                            );
                        }
                    }
                    next_command += 1;
                }
                if view
                    .sample_x(
                        (player + 1) % 4,
                        f64::from(prediction.tick.saturating_sub(lead + 6)),
                    )
                    .is_some()
                {
                    interpolation_samples += 1;
                }
                next_send = instant + STEP;
            }
        }
        if instant >= next_flush {
            transport.send_packets(&mut network)?;
            next_flush = instant + STEP;
        }
        if exit_at.is_some_and(|deadline| instant >= deadline) {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    let (first_baseline_tick, baseline_door_open, baseline_collected_items) =
        first_baseline.ok_or("client never received baseline")?;
    let state = final_state.ok_or("client never received final checkpoint")?;
    let final_predicted_motor = prediction.motor();
    if prediction.motor() != state.motors[player] {
        return Err(format!(
            "tick {} owner {player} prediction did not flush: {:?} != {:?}",
            state.tick,
            prediction.motor(),
            state.motors[player]
        )
        .into());
    }
    write_json(
        directory.join(format!("client-{player}.json")),
        &ClientReport {
            player,
            state,
            snapshots: snapshot_count,
            first_baseline_tick,
            baseline_door_open,
            baseline_collected_items,
            foreign_inventory_values: 0,
            inventory_values_received: inventories_received,
            checked_prediction_checkpoints: prediction.checked,
            corrections_mm: prediction.corrections_mm,
            resimulated_ticks: prediction.resimulated_ticks,
            final_predicted_motor,
            interpolation_samples,
            bytes_encoded: Some(registry.bytes_encoded),
            native: None,
        },
    )?;
    Ok(())
}

pub fn replay_file(path: &Path) -> Result<String> {
    let replay: Replay = read_json(path)?;
    if replay.format != 1 || replay.schema != "s4:Motor1/Door2/Inventory3/Item4:v1" {
        return Err("replay format/schema mismatch".into());
    }
    Ok(format!(
        "{:016x}",
        local::replay(&replay.frames, &replay.checkpoints)?
    ))
}
