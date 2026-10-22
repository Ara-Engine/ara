use super::*;

/// Scheduling and observations only. Mutable gameplay remains in scene components.
#[derive(Resource)]
struct Authority {
    directory: std::path::PathBuf,
    clients: usize,
    listen: bool,
    late: bool,
    ready: BTreeMap<usize, Entity>,
    acks: BTreeSet<usize>,
    origin: Option<u32>,
    started: Option<Instant>,
    commands: BTreeMap<u32, BTreeMap<(u8, u32), Command>>,
    frames: Vec<Frame>,
    checkpoints: Vec<State>,
    accepted: u32,
    rejected: u32,
    late_commands: u32,
    final_sent: bool,
}

fn new_link(event: On<Add<LinkOf>>, mut commands: Commands) {
    commands.entity(event.entity).insert(ReplicationSender);
}

fn receive(world: &mut World) {
    let messages: Vec<_> = world
        .query::<(Entity, &RemoteId, &mut MessageReceiver<ToServer>)>()
        .iter_mut(world)
        .flat_map(|(entity, remote, mut receiver)| {
            let PeerId::Netcode(player) = remote.0 else {
                return Vec::new().into_iter();
            };
            receiver
                .receive()
                .map(|message| (entity, player as usize, message))
                .collect::<Vec<_>>()
                .into_iter()
        })
        .collect();
    for (link, player, message) in messages {
        let scene = *world.resource::<Scene>();
        let current = scene.tick(world);
        match message {
            ToServer::Ready => {
                let (clients, listen) = {
                    let run = world.resource::<Authority>();
                    (run.clients, run.listen)
                };
                if player >= clients || (listen && player == 0) {
                    continue;
                }
                world.entity_mut(scene.entity(player)).insert(ControlledBy {
                    owner: link,
                    lifetime: Lifetime::Persistent,
                });
                let mut run = world.resource_mut::<Authority>();
                run.ready.insert(player, link);
                if let Some(origin) = run.origin {
                    send(world, link, ToClient::Begin { origin });
                }
            }
            ToServer::Intent { tick, mut command } => {
                command.caller = player as u8;
                let mut run = world.resource_mut::<Authority>();
                if player >= run.clients || !run.ready.contains_key(&player) {
                    continue;
                }
                if tick >= current.saturating_sub(60) && tick <= current + 120 && current < TICKS {
                    let assigned = tick.max(current + 1);
                    run.late_commands += u32::from(assigned != tick);
                    let queue = run.commands.entry(assigned).or_default();
                    if queue.len() < 64 {
                        queue
                            .entry((command.caller, command.sequence))
                            .or_insert(command);
                    }
                } else {
                    run.rejected += 1;
                }
            }
            ToServer::FinalAck => {
                world.resource_mut::<Authority>().acks.insert(player);
            }
        }
    }
    let start = {
        let run = world.resource::<Authority>();
        run.origin.is_none()
            && run.ready.len() >= run.clients - usize::from(run.listen) - usize::from(run.late)
    };
    if start {
        let origin = world.resource::<LocalTimeline>().tick().0 + 120;
        let links: Vec<_> = world
            .resource::<Authority>()
            .ready
            .values()
            .copied()
            .collect();
        world.resource_mut::<Authority>().origin = Some(origin);
        for link in links {
            send(world, link, ToClient::Begin { origin });
        }
    }
}

fn tick(world: &mut World) {
    let scene = *world.resource::<Scene>();
    let Some(origin) = world.resource::<Authority>().origin else {
        return;
    };
    let net_tick = world.resource::<LocalTimeline>().tick().0;
    if net_tick <= origin || scene.tick(world) >= TICKS {
        return;
    }
    let game_tick = net_tick - origin;
    assert_eq!(
        game_tick,
        scene.tick(world) + 1,
        "authority fixed ticks cannot skip"
    );
    if game_tick == 1 {
        let directory = world.resource::<Authority>().directory.clone();
        session::write_json(directory.join("started.json"), &true)
            .expect("runtime artifact directory is writable");
        world.resource_mut::<Authority>().started = Some(Instant::now());
    }
    let (clients, listen) = {
        let run = world.resource::<Authority>();
        (run.clients, run.listen)
    };
    let mut frame = Frame {
        tick: game_tick,
        inputs: [MoveInput::default(); 4],
        commands: Vec::new(),
    };
    for (player, input) in frame.inputs.iter_mut().enumerate().take(clients) {
        *input = world
            .get::<ActionState<MoveInput>>(scene.entity(player))
            .map(|state| state.0)
            .unwrap_or_default();
    }
    if let Some(queued) = world
        .resource_mut::<Authority>()
        .commands
        .remove(&game_tick)
    {
        frame.commands.extend(queued.into_values());
    }
    if listen {
        let host = gameplay::scripted_frame(game_tick, 1);
        frame.inputs[0] = host.inputs[0];
        frame.commands.extend(host.commands);
        frame
            .commands
            .sort_by_key(|command| (command.caller, command.sequence));
    }
    let result = scene.step(world, &frame);
    world
        .get_mut::<ReplicaTick>(scene.entity(4))
        .expect("door carries scene checkpoint tick")
        .0 = game_tick;
    let state = scene.capture(world);
    let mut run = world.resource_mut::<Authority>();
    run.accepted += result.accepted;
    run.rejected += result.rejected;
    run.frames.push(frame);
    run.checkpoints.push(state);
}

fn finalize(world: &mut World) {
    let scene = *world.resource::<Scene>();
    if scene.tick(world) != TICKS || world.resource::<Authority>().final_sent {
        return;
    }
    let state = scene.capture(world);
    let registry = Registry::new();
    let peers: Vec<_> = world
        .resource::<Authority>()
        .ready
        .iter()
        .map(|(&player, &link)| (player, link))
        .collect();
    for (player, link) in peers {
        let projection = registry.project(&state, player);
        session::write_json(
            world
                .resource::<Authority>()
                .directory
                .join(format!("expected-{player}.json")),
            &projection,
        )
        .expect("runtime artifact directory remains writable");
        send(
            world,
            link,
            ToClient::Final {
                digest: digest(&projection),
            },
        );
    }
    world.resource_mut::<Authority>().final_sent = true;
}

pub fn run(directory: &Path) -> Result<()> {
    let config: Config = session::read_json(directory.join("config.json"))?;
    let mut app = base(true)?;
    app.insert_resource(ReplicationMetadata::new(Duration::from_millis(50)));
    app.add_observer(new_link);
    let scene = Scene::spawn(app.world_mut());
    for id in 0..50 {
        let mut entity = app.world_mut().entity_mut(scene.entity(id));
        entity.insert((
            ReplicaId(id as u16),
            Replicate::to_clients(NetworkTarget::All),
        ));
        if id < 4 {
            let peer = PeerId::Netcode(id as u64);
            entity.insert((
                PrivateTo(peer),
                PredictionTarget::to_clients(NetworkTarget::Single(peer)),
                InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(peer)),
            ));
        }
    }
    app.world_mut()
        .entity_mut(scene.entity(4))
        .insert(ReplicaTick(0));
    app.insert_resource(scene);
    app.insert_resource(Authority {
        directory: directory.to_owned(),
        clients: config.clients,
        listen: config.listen,
        late: config.late,
        ready: BTreeMap::new(),
        acks: BTreeSet::new(),
        origin: None,
        started: None,
        commands: BTreeMap::new(),
        frames: Vec::new(),
        checkpoints: Vec::new(),
        accepted: 0,
        rejected: 0,
        late_commands: 0,
        final_sent: false,
    });
    app.add_systems(bevy_app::Update, (receive, finalize).chain());
    app.add_systems(FixedUpdate, tick);
    let key = lightyear::netcode::generate_key();
    for (player, proxy) in config.proxies.iter().enumerate() {
        let token = lightyear::netcode::ConnectToken::build(*proxy, PROTOCOL, player as u64, key)
            .timeout_seconds(15)
            .expire_seconds(120)
            .generate()?;
        fs::write(
            directory.join(format!("token-{player}.bin")),
            token.try_into_bytes()?,
        )?;
    }
    let server = app
        .world_mut()
        .spawn((
            lightyear::prelude::server::NetcodeServer::new(
                lightyear::prelude::server::NetcodeConfig {
                    protocol_id: PROTOCOL,
                    private_key: key,
                    max_clients: 4,
                    client_timeout_secs: 15,
                    additional_expected_addresses: config.proxies,
                    ..Default::default()
                },
            ),
            LocalAddr("127.0.0.1:0".parse::<SocketAddr>()?),
            lightyear::prelude::server::ServerUdpIo,
        ))
        .id();
    app.world_mut()
        .trigger(lightyear::prelude::server::Start { entity: server });
    app.update();
    let address = app
        .world()
        .get::<LocalAddr>(server)
        .ok_or("missing server address")?
        .0;
    session::write_json(directory.join("ready.json"), &address)?;
    let boot = Instant::now();
    loop {
        if boot.elapsed() > Duration::from_secs(32) {
            return Err(format!(
                "Lightyear server timed out: tick {}, {} ready, {} acks",
                scene.tick(app.world()),
                app.world().resource::<Authority>().ready.len(),
                app.world().resource::<Authority>().acks.len()
            )
            .into());
        }
        app.update();
        let run = app.world().resource::<Authority>();
        if run.final_sent && run.acks.len() == config.clients - usize::from(config.listen) {
            break;
        }
        thread::sleep(STEP);
    }
    let state = scene.capture(app.world());
    let mut ideal = gameplay::Simulation::new();
    for tick in 1..=TICKS {
        ideal.step(&gameplay::scripted_frame(tick, config.clients));
    }
    let run = app
        .world_mut()
        .remove_resource::<Authority>()
        .expect("authority coordinator exists");
    let replay_hash = local::replay(&run.frames, &run.checkpoints)?;
    session::write_json(
        directory.join("server.json"),
        &ServerReport {
            listen: config.listen,
            clients: config.clients,
            ticks: TICKS,
            entities_replicated: 50,
            state_hash: format!("{:016x}", gameplay::hash(&state)),
            replay_hash: format!("{replay_hash:016x}"),
            script_hash: format!("{:016x}", gameplay::hash(&ideal.capture())),
            exact_script_match: state == ideal.capture(),
            replay_tick_checkpoints: run.checkpoints.len(),
            accepted_commands: run.accepted,
            rejected_commands: run.rejected,
            late_inputs: 0,
            late_commands_scheduled: run.late_commands,
            nonzero_input_ticks: run
                .frames
                .iter()
                .flat_map(|frame| frame.inputs)
                .filter(|input| *input != MoveInput::default())
                .count() as u32,
            bytes_encoded: None,
            host_direct_input_ticks: if config.listen { TICKS } else { 0 },
            host_projection: config.listen.then(|| Registry::new().project(&state, 0)),
            state,
            all_final_acks: true,
            elapsed_seconds: run
                .started
                .ok_or("simulation never started")?
                .elapsed()
                .as_secs_f64(),
        },
    )?;
    session::write_json(
        directory.join("replay.json"),
        &Replay {
            format: 1,
            schema: "s4:Motor1/Door2/Inventory3/Item4:v1".into(),
            frames: run.frames,
            checkpoints: run.checkpoints,
        },
    )?;
    Ok(())
}
