use super::*;

pub(super) fn refresh<F: SimulationFeature>(
    world: &mut World,
    state: &mut Authority,
) -> Result<()> {
    let policies = world
        .remove_resource::<Policies>()
        .expect("policies installed");
    let result = refresh_inner::<F>(world, state, &policies.0);
    world.insert_resource(policies);
    result
}
fn refresh_inner<F: SimulationFeature>(
    world: &mut World,
    state: &mut Authority,
    policies: &PolicyRegistry,
) -> Result<()> {
    let feature = F::State::describe().name;
    let components: Vec<_> = policies
        .schema()
        .components()
        .map(|c| c.name.clone())
        .collect();
    let live: BTreeMap<_, _> = state
        .session
        .session()
        .world()
        .iter_entities()
        .filter_map(|e| e.get::<ara_sim::Identity>().map(|i| (i.0.clone(), e.id())))
        .collect();
    let gone: Vec<_> = state
        .actors
        .keys()
        .filter(|id| !live.contains_key(*id))
        .cloned()
        .collect();
    for identity in gone {
        if let Some(actor) = state.actors.remove(&identity) {
            for &link in state.ready.values() {
                send(world, link, ToClient::Retired { id: actor.spawn.id });
            }
            world.despawn(actor.entity);
            state.interest.remove(actor.spawn.id);
            state.ids.despawn(&identity);
        }
    }
    for identity in live.keys() {
        state.ids.spawn(identity)?;
    }
    for (identity, source) in live {
        if !state.actors.contains_key(&identity) {
            let id = state.ids.spawn(&identity)?;
            let spawn = Spawn {
                id,
                identity: identity.clone(),
                prefab: identity.clone(),
                version: policies.hash(),
                owner: None,
                team: None,
            };
            let entity = world
                .spawn((
                    spawn.clone(),
                    Replicate::to_clients(NetworkTarget::All),
                    PrivateTo(None),
                ))
                .id();
            state.interest.update(&spawn, [0.0; 2])?;
            state.actors.insert(
                identity.clone(),
                Actor {
                    spawn,
                    entity,
                    position: [0.0; 2],
                    public: Projection::default(),
                    private: Projection::default(),
                    native: Projection::default(),
                },
            );
        }
        let actor = state
            .actors
            .get_mut(&identity)
            .expect("actor lifetime inserted");
        let mut public = Projection::default();
        let mut private = Projection::default();
        for name in &components {
            if name == &feature {
                continue;
            }
            if let Some(value) = state.session.component_at(source, name)? {
                public
                    .0
                    .extend(policies.project(name, &value, Audience::Relevant)?.0);
                private
                    .0
                    .extend(policies.project(name, &value, Audience::Owner)?.0);
            }
        }
        let resolve = |text: &str| {
            state
                .ids
                .get(text)
                .or_else(|| text.rsplit_once('#').and_then(|(_, id)| state.ids.get(id)))
        };
        policies.bind_references(&mut public, resolve)?;
        policies.bind_references(&mut private, resolve)?;
        policies.account(&actor.public, &public, &mut state.profile)?;
        policies.account(&actor.private, &private, &mut state.profile)?;
        let mut e = world.entity_mut(actor.entity);
        if actor.public != public {
            e.insert(PublicFields(public.clone()));
            actor.public = public;
        }
        if actor.private != private {
            e.insert(OwnerFields(private.clone()));
            actor.private = private;
        }
        if let Some(native) = state.session.session().world().get::<F::State>(source) {
            if let Some(position) = F::position(native) {
                state.interest.update(&actor.spawn, position)?;
                actor.position = position;
            }
            if e.get::<F::State>() != Some(native) {
                let projected = policies.project(
                    &feature,
                    &serde_json::to_value(native)?,
                    Audience::Relevant,
                )?;
                policies.account(&actor.native, &projected, &mut state.profile)?;
                let wire_native = if policies.has_quantization(&feature) {
                    serde_json::from_value(policies.component(&projected, &feature)?)?
                } else {
                    native.clone()
                };
                actor.native = projected;
                if e.get::<F::State>() != Some(&wire_native) {
                    e.insert(wire_native);
                }
            }
        } else {
            e.remove::<F::State>();
        }
    }
    if let Some(actor) = state.actors.values().next() {
        let mut e = world.entity_mut(actor.entity);
        if let Some(mut tick) = e.get_mut::<ReplicaTick>() {
            tick.set_if_neq(ReplicaTick(state.session.session().tick()));
        } else {
            e.insert(ReplicaTick(state.session.session().tick()));
        }
    }
    let mut recipients: BTreeMap<NetId, BTreeSet<u64>> = BTreeMap::new();
    for player in &state.issued {
        let fallback = ConnectionView {
            player: *player,
            position: [0.0; 2],
            radius: 1_000_000.0,
            team: None,
            team_only: false,
            bytes_per_second: state.config.bytes_per_second,
        };
        for id in state
            .interest
            .visible(state.views.get(player).unwrap_or(&fallback))?
        {
            recipients.entry(id).or_default().insert(player.0);
        }
    }
    for actor in state.actors.values() {
        let target = recipients.remove(&actor.spawn.id).unwrap_or_default();
        if world
            .get::<RelevantTo>(actor.entity)
            .is_none_or(|r| r.0 != target)
        {
            world.entity_mut(actor.entity).insert(RelevantTo(target));
        }
    }
    Ok(())
}
