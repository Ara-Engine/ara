use super::*;

pub(super) fn controlled(event: On<Add<Controlled>>, mut commands: Commands) {
    commands
        .entity(event.entity)
        .insert(InputMarker::<Input>::default());
}
pub(super) fn inputs<F: SimulationFeature>(
    timeline: SyncedLocalTimeline,
    sync: Res<LocalTimelineSync>,
    mut owner: ResMut<Owner>,
    mut actions: Query<&mut ActionState<Input>, With<InputMarker<Input>>>,
) {
    let Some(origin) = owner.origin else {
        return;
    };
    let target = timeline.tick().0 + u32::from(sync.input_delay());
    let tick = u64::from(target.saturating_sub(origin));
    owner.status.predicted_tick = tick;
    let input = if target > origin && !owner.limit.is_some_and(|l| tick > l) {
        let injected = owner
            .inputs
            .range(..=tick)
            .next_back()
            .and_then(|(start, i)| (tick < start + u64::from(i.hold_ticks)).then_some(i.value));
        F::input(injected.or_else(|| owner.source.and_then(|f| f(tick, owner.player))))
    } else {
        Ok(F::Input::default())
    };
    match input
        .and_then(|v| postcard::to_allocvec(&v).map_err(|e| ara_sim::SimError(e.to_string())))
    {
        Ok(data) if data.len() <= 256 => {
            for mut action in &mut actions {
                action.0.data.clone_from(&data);
            }
        }
        Ok(_) => owner.fault = Some(NetError("feature input exceeds wire limit".into())),
        Err(e) => owner.fault = Some(e.into()),
    }
    owner
        .inputs
        .retain(|start, i| tick < start.saturating_add(u64::from(i.hold_ticks)));
}
pub(super) fn movement<F: SimulationFeature>(
    timeline: SyncedLocalTimeline,
    owner: Res<Owner>,
    mut motors: Query<(&mut F::State, &ActionState<Input>), With<Predicted>>,
    perturb: Option<ResMut<Perturb<F>>>,
) {
    let Some(origin) = owner.origin else {
        return;
    };
    let tick = u64::from(timeline.tick().0.saturating_sub(origin));
    if timeline.tick().0 <= origin || owner.limit.is_some_and(|l| tick > l) {
        return;
    }
    let mut perturb = perturb;
    for (mut state, input) in &mut motors {
        if let Ok(input) = input.0.typed::<F>() {
            F::step(&mut state, &input);
        }
        if let Some(p) = &mut perturb
            && tick == p.tick
            && !p.applied
        {
            (p.run)(&mut state);
            p.applied = true;
        }
    }
}
type Histories<'w, 's, S> = Query<
    'w,
    's,
    (
        &'static Spawn,
        &'static ConfirmedHistory<S>,
        &'static PredictionHistory<S>,
    ),
    With<Predicted>,
>;
pub(super) fn corrections<F: SimulationFeature>(
    mut owner: ResMut<Owner>,
    histories: Histories<F::State>,
) {
    for (spawn, confirmed, predicted) in &histories {
        let Some((tick, authority)) = confirmed.newest_present() else {
            continue;
        };
        if owner
            .last_checked
            .get(&spawn.id)
            .is_some_and(|last| tick.0 <= *last)
        {
            continue;
        }
        let Some(prediction) = predicted.get(tick) else {
            continue;
        };
        owner.last_checked.insert(spawn.id, tick.0);
        owner.status.checkpoints += 1;
        let magnitude = F::correction(authority, prediction);
        if magnitude > 0.0 && owner.status.corrections.len() < 4096 {
            owner.status.corrections.push(magnitude);
        }
    }
}
pub(super) fn observe(world: &mut World) {
    let link = world
        .query_filtered::<Entity, With<Connected>>()
        .iter(world)
        .next();
    let Some(link) = link else {
        return;
    };
    if !world.resource::<Owner>().ready {
        let hash = world.resource::<Policies>().0.hash();
        let feature = world.resource::<Compatibility>().0;
        if let Some(mut sender) = world.get_mut::<MessageSender<ToServer>>(link) {
            sender.send::<Control>(ToServer::Ready {
                schema: hash,
                feature,
            });
            world.resource_mut::<Owner>().ready = true;
        }
    }
    let messages: Vec<_> = world
        .get_mut::<MessageReceiver<ToClient>>(link)
        .map(|mut rx| rx.receive().take(64).collect())
        .unwrap_or_default();
    for message in messages {
        match message {
            ToClient::Begin { origin } => world.resource_mut::<Owner>().origin = Some(origin),
            ToClient::Retired { id } => world.resource_mut::<Owner>().references.despawned(id),
            ToClient::Rejected => {
                world.resource_mut::<Owner>().fault =
                    Some(NetError("authority rejected protocol/schema".into()))
            }
        }
    }
    let player = world.resource::<Owner>().player;
    let changed_fields: Vec<_> = world
        .query::<(
            Ref<Spawn>,
            Option<Ref<PublicFields>>,
            Option<Ref<OwnerFields>>,
        )>()
        .iter(world)
        .filter(|(s, a, b)| {
            s.is_changed()
                || a.as_ref().is_some_and(|a| a.is_changed())
                || b.as_ref().is_some_and(|b| b.is_changed())
        })
        .map(|(s, a, b)| {
            let mut fields = a.map(|p| p.0.clone()).unwrap_or_default();
            if s.owner == Some(player)
                && let Some(p) = b
            {
                fields.0.extend(p.0.0.clone());
            }
            (s.id, fields)
        })
        .collect();
    let visible: BTreeSet<_> = world.query::<&Spawn>().iter(world).map(|s| s.id).collect();
    let refs: Vec<_> = world
        .query::<(&Spawn, Option<&PublicFields>, Option<&OwnerFields>)>()
        .iter(world)
        .flat_map(|(s, a, b)| {
            a.into_iter()
                .flat_map(|a| a.0.references())
                .chain(b.into_iter().flat_map(|b| b.0.references()))
                .map(|target| (s.id, target))
                .collect::<Vec<_>>()
        })
        .collect();
    {
        let mut owner = world.resource_mut::<Owner>();
        owner.field_views.retain(|id, _| visible.contains(id));
        for (id, fields) in changed_fields {
            if let Some(view) = owner.field_views.get_mut(&id) {
                if view.current != fields {
                    view.previous = std::mem::replace(&mut view.current, fields);
                }
            } else {
                owner.field_views.insert(
                    id,
                    FieldView {
                        previous: fields.clone(),
                        current: fields,
                    },
                );
            }
        }
        let gone: Vec<_> = owner.visible.difference(&visible).copied().collect();
        for id in gone {
            owner.references.hidden(id);
        }
        for &id in &visible {
            owner.references.clear_source(id);
            if !owner.references.is_retired(id)
                && let Err(e) = owner.references.spawned(id)
            {
                owner.fault = Some(e);
            }
        }
        for (source, target) in refs {
            if !owner.references.is_retired(target)
                && let Err(e) = owner.references.resolve(source, target)
            {
                owner.fault = Some(e);
            }
        }
        owner.visible = visible;
        owner.status.pending_references = owner.references.pending();
    }
    let mut count = 0;
    let mut leaks = 0;
    for (spawn, _) in world.query::<(&Spawn, &OwnerFields)>().iter(world) {
        count += 1;
        if spawn.owner != Some(player) {
            leaks += 1;
        }
    }
    let tick = world
        .query::<&ReplicaTick>()
        .iter(world)
        .map(|t| t.0)
        .max()
        .unwrap_or_default();
    let mut owner = world.resource_mut::<Owner>();
    owner.status.tick = tick;
    owner.status.private_samples += count;
    owner.status.privacy_leaks += leaks;
    if leaks > 0 {
        owner.fault = Some(NetError("foreign private fields received".into()));
    }
}
