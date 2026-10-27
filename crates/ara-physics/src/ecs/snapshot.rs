use super::*;

#[derive(Clone, Copy)]
struct Inputs {
    id: u64,
    pose: Option<PhysicsPose>,
    velocity: Option<Velocity>,
    motion: Option<CharacterMotion>,
    telemetry: Option<VehicleTelemetry>,
    character: Option<CharacterInput>,
    vehicle: Option<VehicleInput>,
}

/// Trusted companion checkpoint for ara-sim's ECS/RNG snapshot at the same completed tick.
/// Backend handles are retained locally; they never enter an asset or network payload.
#[derive(Clone)]
pub struct EcsPhysicsSnapshot<S> {
    physics: PhysicsSnapshot<S>,
    bindings: BTreeMap<u64, Binding>,
    inputs: Vec<Inputs>,
    commands: PhysicsCommands,
}
impl<S> EcsPhysicsSnapshot<S> {
    /// Completed tick shared with the simulation checkpoint.
    pub fn tick(&self) -> u64 {
        self.physics.tick()
    }
    /// Underlying native checkpoint and configuration identity.
    pub fn physics(&self) -> &PhysicsSnapshot<S> {
        &self.physics
    }
}

/// Captures native state, boundary intent and the stable-ID handle cache.
/// The host captures its ECS/RNG state at the same tick without advancing the schedule.
pub fn snapshot_ecs<B: PhysicsBackend>(
    world: &mut World,
) -> Result<EcsPhysicsSnapshot<B::Snapshot>> {
    prepare_ecs::<B>(world)?;
    let physics = world.resource_scope(|_, mut p: Mut<PhysicsWorld<B>>| p.snapshot())?;
    let runtime = world.resource::<Runtime>();
    let inputs = runtime
        .rows
        .iter()
        .map(|&(id, e)| Inputs {
            id,
            pose: world.get::<PhysicsPose>(e).copied(),
            velocity: world.get::<Velocity>(e).copied(),
            motion: world.get::<CharacterMotion>(e).copied(),
            telemetry: world.get::<VehicleTelemetry>(e).copied(),
            character: world.get::<CharacterInput>(e).copied(),
            vehicle: world.get::<VehicleInput>(e).copied(),
        })
        .collect();
    Ok(EcsPhysicsSnapshot {
        physics,
        bindings: runtime.bindings.clone(),
        inputs,
        commands: world.resource::<PhysicsCommands>().clone(),
    })
}

/// Restores a matching local checkpoint, rebinds current Bevy entities by PhysicsId,
/// and restores exact optional ECS mirrors. Rejects topology changes before mutation.
/// Restore the simulation's other registered components/RNG before calling this.
pub fn restore_ecs<B: PhysicsBackend>(
    world: &mut World,
    snapshot: &EcsPhysicsSnapshot<B::Snapshot>,
) -> Result<()> {
    world.resource_scope(|world,mut physics:Mut<PhysicsWorld<B>>| {
        world.resource_scope(|world,mut runtime:Mut<Runtime>|->Result<()> {
            runtime.rows.clear();
            let rows:Vec<_>=runtime.query.iter(world).map(|(e,id)|(id.value,e)).collect();
            runtime.rows.extend(rows);runtime.rows.sort_unstable_by_key(|r|r.0);
            if runtime.rows.len()!=snapshot.bindings.len() || runtime.rows.iter().any(|&(id,e)|snapshot.bindings.get(&id).is_none_or(|b|!b.matches(world,e) || b.joint_settings!=world.get::<Joint>(e).copied())) {
                return Err(PhysicsError("ECS snapshot topology differs; restore/reconstruct its authoring structure first".into()));
            }
            physics.restore(&snapshot.physics)?;
            runtime.bindings.clone_from(&snapshot.bindings);
            runtime.bodies.clear();
            let bodies:Vec<_>=runtime.rows.iter().filter_map(|(id,_)|runtime.bindings.get(id).and_then(Binding::body)).collect();
            runtime.bodies.extend(bodies);
            writeback(world,&mut physics,&mut runtime)?;
            for input in &snapshot.inputs {
                if let Ok(index)=runtime.rows.binary_search_by_key(&input.id,|r|r.0) {
                    let e=runtime.rows[index].1;
                    // Native extraction may round shape-center transforms and report
                    // uninitialized tick-zero support. Gameplay sees the captured ECS
                    // mirrors until the next physics boundary, including their absence.
                    restore_mirror(world,e,input.pose);
                    restore_mirror(world,e,input.velocity);
                    restore_mirror(world,e,input.motion);
                    restore_mirror(world,e,input.telemetry);
                    if let Some(c)=input.character {world.entity_mut(e).insert(c);} else {world.entity_mut(e).remove::<CharacterInput>();}
                    if let Some(v)=input.vehicle {world.entity_mut(e).insert(v);} else {world.entity_mut(e).remove::<VehicleInput>();}
                }
            }
            world.resource_mut::<PhysicsCommands>().clone_from(&snapshot.commands);
            world.resource_mut::<PhysicsStatus>().error=None;Ok(())
        })
    })
}

fn restore_mirror<T: Component<Mutability = Mutable>>(
    world: &mut World,
    entity: Entity,
    value: Option<T>,
) {
    match value {
        Some(value) => {
            world.entity_mut(entity).insert(value);
        }
        None => {
            world.entity_mut(entity).remove::<T>();
        }
    }
}
