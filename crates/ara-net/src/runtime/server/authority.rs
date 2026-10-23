use super::*;

pub(super) fn new_link(
    event: On<Add<RemoteId>>,
    remote: Query<&RemoteId, With<LinkOf>>,
    state: Res<Authority>,
    mut commands: Commands,
) {
    let Ok(remote) = remote.get(event.entity) else {
        return;
    };
    let PeerId::Netcode(player) = remote.0 else {
        return;
    };
    let budget = state
        .views
        .get(&PlayerId(player))
        .map_or(state.config.bytes_per_second, |v| v.bytes_per_second);
    commands.entity(event.entity).insert((
        ReplicationSender,
        Transport::new(PriorityConfig::new(budget)),
        ByteBudget(budget),
    ));
}
pub(super) fn receive(world: &mut World) {
    let messages: Vec<_> = world
        .query::<(Entity, &RemoteId, &mut MessageReceiver<ToServer>)>()
        .iter_mut(world)
        .flat_map(|(e, r, mut rx)| {
            rx.receive()
                .take(64)
                .map(|m| (e, r.0, m))
                .collect::<Vec<_>>()
        })
        .collect();
    let hash = world.resource::<Policies>().0.hash();
    let feature_hash = world.resource::<Compatibility>().0;
    let mut state = world
        .remove_resource::<Authority>()
        .expect("authority resource installed");
    let connected: BTreeSet<_> = world
        .query_filtered::<&RemoteId, With<Connected>>()
        .iter(world)
        .filter_map(|r| {
            if let PeerId::Netcode(p) = r.0 {
                Some(PlayerId(p))
            } else {
                None
            }
        })
        .collect();
    let gone: Vec<_> = state
        .ready
        .keys()
        .filter(|p| !connected.contains(p))
        .copied()
        .collect();
    for player in gone {
        state.ready.remove(&player);
        state.adapter.gate.disconnect(player);
    }
    for (link, peer, message) in messages {
        let PeerId::Netcode(p) = peer else {
            continue;
        };
        let player = PlayerId(p);
        if !state.issued.contains(&player) {
            continue;
        }
        match message {
            ToServer::Ready { schema, feature } if schema == hash && feature == feature_hash => {
                if state.ready.insert(player, link) != Some(link) {
                    state.adapter.gate.authenticate(player);
                    for actor in state.actors.values() {
                        if actor.spawn.owner == Some(player) {
                            let mut entity = world.entity_mut(actor.entity);
                            entity.insert((
                                ActionState::<Input>::default(),
                                lightyear::input::input_buffer::InputBuffer::<
                                    ActionState<Input>,
                                    Input,
                                >::default(),
                            ));
                            entity.insert(ControlledBy {
                                owner: link,
                                lifetime: Lifetime::Persistent,
                            });
                        }
                    }
                    if let Some(origin) = state.origin {
                        send(world, link, ToClient::Begin { origin });
                    }
                }
            }
            ToServer::Intent(intent)
                if state.ready.contains_key(&player)
                    && state.pending_intents.values().map(Vec::len).sum::<usize>()
                        < state.config.max_clients * 16
                    && state
                        .pending_intents
                        .values()
                        .flatten()
                        .filter(|(p, _)| *p == player)
                        .count()
                        < 16
                    && intent.tick >= state.session.session().tick().saturating_sub(60)
                    && intent.tick <= state.session.session().tick().saturating_add(120)
                    && intent.payload.len() <= 4096
                    && intent.target.len() <= 256
                    && intent.command.len() <= 128 =>
            {
                let assigned = intent.tick.max(state.session.session().tick() + 1);
                state
                    .pending_intents
                    .entry(assigned)
                    .or_default()
                    .push((player, intent));
            }
            _ => send(world, link, ToClient::Rejected),
        }
    }
    if state.origin.is_none() && state.ready.len() >= state.config.start_clients {
        let origin = world.resource::<LocalTimeline>().tick().0 + 120;
        state.origin = Some(origin);
        for &link in state.ready.values() {
            send(world, link, ToClient::Begin { origin });
        }
    }
    world.insert_resource(state);
}
pub(super) fn tick<F: SimulationFeature>(world: &mut World) {
    let mut state = world
        .remove_resource::<Authority>()
        .expect("authority resource installed");
    let result = tick_inner::<F>(world, &mut state);
    if let Err(e) = result {
        state.fault = Some(e);
    }
    world.insert_resource(state);
}
fn tick_inner<F: SimulationFeature>(world: &mut World, state: &mut Authority) -> Result<()> {
    if state.fault.is_some()
        || state
            .config
            .tick_limit
            .is_some_and(|limit| state.session.session().tick() >= limit)
    {
        return Ok(());
    }
    let Some(origin) = state.origin else {
        return Ok(());
    };
    let tick = world.resource::<LocalTimeline>().tick().0;
    if tick <= origin {
        return Ok(());
    }
    if u64::from(tick - origin) != state.session.session().tick() + 1 {
        return Err(NetError(
            "authority timeline skipped a simulation boundary".into(),
        ));
    }
    for actor in state.actors.values() {
        if let Some(player) = actor.spawn.owner
            && state.ready.contains_key(&player)
            && let Some(input) = world.get::<ActionState<Input>>(actor.entity)
        {
            // Invalid/unavailable input is neutral; clients never submit state or edits.
            let codec = world.get_resource::<crate::ActionCodec<F::Input>>();
            let typed = input.0.typed_with::<F>(codec).unwrap_or_default();
            let actions = match codec {
                Some(codec) => (codec.expand)(&typed)?,
                None => vec![(F::ACTION.into(), F::action(&typed)?)],
            };
            state
                .adapter
                .inputs
                .extend(actions.into_iter().map(|(action, value)| Injection {
                    player,
                    action,
                    value,
                    hold_ticks: 1,
                }));
        }
    }
    if let Some(intents) = state
        .pending_intents
        .remove(&(state.session.session().tick() + 1))
    {
        for (caller, intent) in intents {
            let owner = state.actors.get(&intent.target).and_then(|a| a.spawn.owner);
            state.adapter.intents.push((caller, owner, intent));
        }
    }
    let record = state.session.step(&mut state.adapter)?;
    if let Some(trace) = &mut state.trace {
        trace.push(state.session.session(), record.clone())?;
    }
    if state.record {
        state.records.push(record);
    }
    refresh::<F>(world, state)
}
