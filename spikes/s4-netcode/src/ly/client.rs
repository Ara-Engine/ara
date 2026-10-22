use super::*;
use lightyear::prediction::{
    diagnostics::PredictionMetrics, predicted_history::PredictionHistory, rollback::RollbackSystems,
};

#[derive(Resource)]
struct Owner {
    player: usize,
    origin: Option<u32>,
    ready: bool,
    next_command: u32,
    baseline: Option<(u32, bool, usize)>,
    final_digest: Option<u64>,
    completed: Option<Snapshot>,
    exit_at: Option<Instant>,
    snapshots: u32,
    inventory_values: u32,
    last_scene_tick: Option<u32>,
    checked: u32,
    corrections: Vec<f64>,
    last_checked: u32,
    interpolation_samples: u32,
    fault_enabled: bool,
    fault_injected: bool,
    error: Option<String>,
}

fn controlled(event: On<Add<Controlled>>, mut commands: Commands) {
    commands
        .entity(event.entity)
        .insert(InputMarker::<MoveInput>::default());
}

/// Sample only new inputs; the upstream plugin owns buffering and rollback restoration.
fn inputs(
    timeline: SyncedLocalTimeline,
    sync: Res<LocalTimelineSync>,
    owner: Res<Owner>,
    mut actions: Query<&mut ActionState<MoveInput>, With<InputMarker<MoveInput>>>,
) {
    let Some(origin) = owner.origin else {
        return;
    };
    let target = timeline.tick().0 + u32::from(sync.input_delay());
    let tick = target.saturating_sub(origin);
    let input = if target > origin && tick <= TICKS {
        gameplay::scripted_frame(tick, 4).inputs[owner.player]
    } else {
        MoveInput::default()
    };
    for mut action in &mut actions {
        action.0 = input;
    }
}

/// This is the same gameplay function as Local and server; native rollback reruns this system.
fn movement(
    timeline: SyncedLocalTimeline,
    mut owner: ResMut<Owner>,
    mut motors: Query<(&mut Motor, &ActionState<MoveInput>), With<Predicted>>,
) {
    let Some(origin) = owner.origin else {
        return;
    };
    let tick = timeline.tick().0.saturating_sub(origin);
    if timeline.tick().0 <= origin || tick > TICKS {
        return;
    }
    for (mut motor, input) in &mut motors {
        gameplay::step_motor(&mut motor, input.0);
        // A reproducible fault tests native mismatch detection/restore/replay even on a quiet link.
        // It is a test perturbation of prediction only; authority and gameplay are unchanged.
        if owner.fault_enabled && owner.player == 0 && tick == 330 && !owner.fault_injected {
            motor.x += 250;
            owner.fault_injected = true;
        }
    }
}

/// Observe same-tick native histories before the library checks, restores or prunes them.
fn corrections(
    mut owner: ResMut<Owner>,
    histories: Query<(&ConfirmedHistory<Motor>, &PredictionHistory<Motor>), With<Predicted>>,
) {
    for (confirmed, predicted) in &histories {
        let Some((tick, authority)) = confirmed.newest_present() else {
            continue;
        };
        if tick.0 <= owner.last_checked {
            continue;
        }
        let Some(prediction) = predicted.get(tick) else {
            continue;
        };
        owner.last_checked = tick.0;
        owner.checked += 1;
        let magnitude =
            f64::from(authority.x - prediction.x).hypot(f64::from(authority.z - prediction.z));
        if magnitude > 0.0 {
            owner.corrections.push(magnitude);
        }
    }
}

fn replica(world: &mut World, player: usize) -> std::result::Result<Option<Snapshot>, String> {
    let mut motors = [None; 4];
    let mut items = [None; 45];
    let mut door = None;
    let mut tick = None;
    let mut inventory = None;
    let mut query = world.query::<(
        &ReplicaId,
        Option<&Motor>,
        Option<&ConfirmedHistory<Motor>>,
        Option<&Door>,
        Option<&Item>,
        Option<&Inventory>,
        Option<&ReplicaTick>,
    )>();
    for (id, motor, confirmed, received_door, item, private, scene_tick) in query.iter(world) {
        let id = usize::from(id.0);
        if let Some(private) = private {
            if usize::from(private.owner) != player {
                return Err(format!(
                    "privacy violation: client {player} received owner {} at slot {id}",
                    private.owner
                ));
            }
            inventory = Some(*private);
        }
        if id < 4 {
            motors[id] = confirmed
                .and_then(ConfirmedHistory::newest_present)
                .map(|(_, value)| *value)
                .or(motor.copied());
        } else if id == 4 {
            door = received_door.copied();
            tick = scene_tick.map(|value| value.0);
        } else if id < 50 {
            items[id - 5] = item.copied();
        } else {
            return Err(format!("unexpected replica slot {id}"));
        }
    }
    let (Some(tick), Some(door), Some(inventory)) = (tick, door, inventory) else {
        return Ok(None);
    };
    if motors.iter().any(Option::is_none) || items.iter().any(Option::is_none) {
        return Ok(None);
    }
    Ok(Some(Snapshot {
        tick,
        door,
        inventory,
        motors: motors.map(|value| value.expect("replica completeness checked")),
        items: items
            .into_iter()
            .map(|value| value.expect("replica completeness checked"))
            .collect(),
    }))
}

fn update(world: &mut World) {
    let links: Vec<_> = world
        .query_filtered::<Entity, With<Connected>>()
        .iter(world)
        .collect();
    let Some(&link) = links.first() else {
        return;
    };
    if !world.resource::<Owner>().ready {
        if let Some(mut sender) = world.get_mut::<MessageSender<ToServer>>(link) {
            sender.send::<ControlChannel>(ToServer::Ready);
        }
        world.resource_mut::<Owner>().ready = true;
    }
    let messages: Vec<_> = world
        .get_mut::<MessageReceiver<ToClient>>(link)
        .map(|mut receiver| receiver.receive().collect())
        .unwrap_or_default();
    for message in messages {
        match message {
            ToClient::Begin { origin } => {
                world.resource_mut::<Owner>().origin = Some(origin);
            }
            ToClient::Final { digest } => {
                world.resource_mut::<Owner>().final_digest = Some(digest);
            }
        }
    }
    let player = world.resource::<Owner>().player;
    let snapshot = match replica(world, player) {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => return,
        Err(error) => {
            world.resource_mut::<Owner>().error = Some(error);
            return;
        }
    };
    {
        let mut owner = world.resource_mut::<Owner>();
        if owner.origin.is_some() && owner.baseline.is_none() {
            owner.baseline = Some((
                snapshot.tick,
                snapshot.door.open,
                snapshot.items.iter().filter(|item| !item.available).count(),
            ));
            owner.next_command = snapshot.tick + 1;
        }
        if owner.last_scene_tick != Some(snapshot.tick) {
            owner.snapshots += 1;
            owner.inventory_values += 1;
            owner.last_scene_tick = Some(snapshot.tick);
        }
    }
    let through = (snapshot.tick + 12).min(TICKS);
    let commands: Vec<_> = {
        let mut owner = world.resource_mut::<Owner>();
        if owner.baseline.is_none() {
            return;
        }
        let from = owner.next_command;
        owner.next_command = owner.next_command.max(through + 1);
        (from..=through)
            .flat_map(|tick| {
                gameplay::scripted_frame(tick, 4)
                    .commands
                    .into_iter()
                    .filter(move |command| usize::from(command.caller) == player)
                    .map(move |command| ToServer::Intent { tick, command })
            })
            .collect()
    };
    for command in commands {
        world
            .get_mut::<MessageSender<ToServer>>(link)
            .expect("typed message sender registered")
            .send::<ControlChannel>(command);
    }
    let interpolated_count = world
        .query_filtered::<&Motor, With<Interpolated>>()
        .iter(world)
        .count();
    if interpolated_count > 0 {
        world.resource_mut::<Owner>().interpolation_samples += 1;
    }
    let final_digest = world.resource::<Owner>().final_digest;
    if snapshot.tick == TICKS
        && final_digest == Some(digest(&snapshot))
        && world.resource::<Owner>().completed.is_none()
    {
        let predicted = world
            .query_filtered::<&Motor, With<Predicted>>()
            .iter(world)
            .next()
            .copied();
        if predicted != Some(snapshot.motors[player]) {
            return;
        }
        world.resource_mut::<Owner>().completed = Some(snapshot);
        world.resource_mut::<Owner>().exit_at = Some(Instant::now() + Duration::from_millis(500));
        world
            .get_mut::<MessageSender<ToServer>>(link)
            .expect("typed message sender registered")
            .send::<ControlChannel>(ToServer::FinalAck);
    }
}

pub fn run(directory: &Path, player: usize) -> Result<()> {
    let config: Config = session::read_json(directory.join("config.json"))?;
    let mut app = base(false)?;
    app.insert_resource(PredictionManager::default());
    app.insert_resource(InputTimelineConfig::new(
        SyncConfig {
            jitter_margin: if config.prediction_lead_ticks == 6 {
                1.0
            } else {
                6.0
            },
            ..Default::default()
        },
        lightyear::prelude::client::InputDelayConfig::no_input_delay(),
    ));
    app.insert_resource(Owner {
        player,
        origin: None,
        ready: false,
        next_command: 1,
        baseline: None,
        final_digest: None,
        completed: None,
        exit_at: None,
        snapshots: 0,
        inventory_values: 0,
        last_scene_tick: None,
        checked: 0,
        corrections: Vec::new(),
        last_checked: 0,
        interpolation_samples: 0,
        fault_enabled: config.prediction_lead_ticks == 6,
        fault_injected: false,
        error: None,
    });
    app.add_observer(controlled);
    app.add_systems(
        FixedPreUpdate,
        inputs.in_set(lightyear::input::client::InputSystems::WriteClientInputs),
    );
    app.add_systems(FixedUpdate, movement);
    app.add_systems(
        PreUpdate,
        corrections
            .after(ReplicationSystems::Receive)
            .before(RollbackSystems::Check),
    );
    app.add_systems(bevy_app::Update, update);
    let token = lightyear::netcode::ConnectToken::try_from_bytes(&fs::read(
        directory.join(format!("token-{player}.bin")),
    )?)?;
    let link = app
        .world_mut()
        .spawn((
            lightyear::prelude::client::Client,
            ReplicationReceiver,
            LocalAddr("127.0.0.1:0".parse::<SocketAddr>()?),
            PeerAddr(config.proxies[player]),
            lightyear::prelude::client::NetcodeClient::new(
                lightyear::netcode::auth::Authentication::Token(token),
                lightyear::prelude::client::NetcodeConfig {
                    client_timeout_secs: 15,
                    ..Default::default()
                },
            )?,
            UdpIo::default(),
        ))
        .id();
    app.world_mut().trigger(Connect { entity: link });
    let boot = Instant::now();
    loop {
        app.update();
        if let Some(error) = &app.world().resource::<Owner>().error {
            return Err(error.to_owned().into());
        }
        if app
            .world()
            .resource::<Owner>()
            .exit_at
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            break;
        }
        if boot.elapsed() > Duration::from_secs(30) {
            let snapshot = replica(app.world_mut(), player)?;
            if let Some(observed) = &snapshot {
                session::write_json(directory.join(format!("observed-{player}.json")), observed)?;
            }
            let predicted = app
                .world_mut()
                .query_filtered::<&Motor, With<Predicted>>()
                .iter(app.world())
                .next()
                .copied();
            return Err(format!("Lightyear owner {player} timed out: final digest {:?}, replica tick {:?}, synced {}, predicted {:?}",
                app.world().resource::<Owner>().final_digest, snapshot.as_ref().map(|state| state.tick),
                app.world().resource::<LocalTimelineSync>().is_synced(),
                predicted).into());
        }
        thread::sleep(STEP);
    }
    let metrics = app.world().resource::<PredictionMetrics>();
    let rollbacks = metrics.rollbacks;
    let replayed = metrics.rollback_ticks;
    let predicted_ids: Vec<_> = app
        .world_mut()
        .query_filtered::<&ReplicaId, With<Predicted>>()
        .iter(app.world())
        .map(|id| usize::from(id.0))
        .collect();
    let interpolated = app
        .world_mut()
        .query_filtered::<&Motor, With<Interpolated>>()
        .iter(app.world())
        .count();
    let histories = app
        .world_mut()
        .query::<&PredictionHistory<Motor>>()
        .iter(app.world())
        .count();
    let inventories = app
        .world_mut()
        .query::<&Inventory>()
        .iter(app.world())
        .count();
    if predicted_ids != [player] || interpolated != 3 || histories != 1 || inventories != 1 {
        return Err(format!("invalid selective prediction/privacy scope: predicted {predicted_ids:?}, interpolated {interpolated}, histories {histories}, inventories {inventories}").into());
    }
    let diagnostics = session::NativeDiagnostics {
        rollback_events: rollbacks,
        replayed_ticks: replayed,
        faults_injected: u32::from(app.world().resource::<Owner>().fault_injected),
        predicted_entities: predicted_ids.len(),
        interpolated_entities: interpolated,
        prediction_histories: histories,
        private_inventory_components: inventories,
        world_resources: app.world().iter_resources().count(),
        world_entities: app.world().entities().len() as usize,
    };
    let predicted = app
        .world_mut()
        .query_filtered::<&Motor, With<Predicted>>()
        .iter(app.world())
        .next()
        .copied()
        .ok_or("owner never acquired native predicted motor")?;
    let owner = app
        .world_mut()
        .remove_resource::<Owner>()
        .expect("owner coordinator exists");
    let (baseline_tick, baseline_door, baseline_items) =
        owner.baseline.ok_or("no native baseline")?;
    let state = owner.completed.ok_or("native replicas did not converge")?;
    session::write_json(
        directory.join(format!("native-{player}.json")),
        &serde_json::json!({
            "native_rollbacks": rollbacks, "replayed_ticks": replayed, "fault_injected": owner.fault_injected,
            "registered_prediction_histories": histories, "final_scene_tick": state.tick,
            "final_world_resources": app.world().iter_resources().count(), "final_world_entities": app.world().entities().len(),
        }),
    )?;
    if owner.fault_injected && rollbacks == 0 {
        return Err("injected prediction error did not trigger native rollback".into());
    }
    session::write_json(
        directory.join(format!("client-{player}.json")),
        &ClientReport {
            player,
            state,
            snapshots: owner.snapshots,
            first_baseline_tick: baseline_tick,
            baseline_door_open: baseline_door,
            baseline_collected_items: baseline_items,
            foreign_inventory_values: 0,
            inventory_values_received: owner.inventory_values,
            checked_prediction_checkpoints: owner.checked,
            corrections_mm: owner.corrections,
            resimulated_ticks: replayed,
            final_predicted_motor: predicted,
            interpolation_samples: owner.interpolation_samples,
            bytes_encoded: None,
            native: Some(diagnostics),
        },
    )?;
    Ok(())
}
