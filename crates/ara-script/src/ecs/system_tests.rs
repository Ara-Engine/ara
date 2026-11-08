use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering};

use ara_physics::{PhysicsId, PhysicsPose, Velocity};
use bevy_ecs::prelude::{Component, Resource};
use bevy_ecs::system::SystemState;
use bevy_tasks::TaskPoolBuilder;

use super::*;
use crate::abi::{BehaviourCall, CommandCall, RUN_DISABLED, RUN_FAULTED, RUN_OK};

#[derive(Component)]
struct DenseA;
#[derive(Component)]
struct DenseB;
#[derive(Component)]
#[component(storage = "SparseSet")]
struct Sparse;
#[derive(Resource)]
struct Missing;
#[derive(Component, Default)]
#[component(summary_tick)]
struct Summary;

#[derive(Default)]
struct Check {
    ticks: BTreeMap<u64, (*const BevyTick, *const BevyTick)>,
    expected: AtomicU32,
    rows: AtomicUsize,
    calls: AtomicUsize,
    wrong_tick: AtomicBool,
    result: AtomicI32,
}

impl Check {
    fn refresh(&mut self, world: &World) {
        self.ticks.clear();
        let velocity = world.component_id::<Velocity>().expect("velocity");
        let pose = world.component_id::<PhysicsPose>().expect("pose");
        for table in world.storages().tables.iter() {
            let (Some(velocity), Some(pose)) = (table.get_column(velocity), table.get_column(pose))
            else {
                continue;
            };
            let len = table.entity_count() as usize;
            // SAFETY: both columns belong to this table and have its actual row count.
            let (velocity, pose) = unsafe {
                (
                    velocity.get_changed_ticks_slice(len),
                    pose.get_changed_ticks_slice(len),
                )
            };
            for (row, entity) in table.entities().iter().enumerate() {
                self.ticks.insert(
                    entity.to_bits(),
                    (
                        velocity[row].get().cast_const(),
                        pose[row].get().cast_const(),
                    ),
                );
            }
        }
        self.rows.store(0, Ordering::Relaxed);
        self.calls.store(0, Ordering::Relaxed);
        self.wrong_tick.store(false, Ordering::Relaxed);
    }
}

unsafe extern "C" fn run(host: *mut c_void, call: *const SystemCall) -> i32 {
    // SAFETY: the test owns Check and the query for the whole task scope. The map is
    // immutable during dispatch; atomics are the only shared mutable test state.
    let (check, call) = unsafe { (&*host.cast::<Check>(), &*call) };
    check.calls.fetch_add(1, Ordering::Relaxed);
    check.rows.fetch_add(call.len as usize, Ordering::Relaxed);
    for row in 0..call.len as usize {
        // SAFETY: the callback's entity slice has call.len live entries.
        let entity = unsafe { *call.entities.add(row) };
        let Some(&(velocity_tick, pose_tick)) = check.ticks.get(&entity) else {
            check.wrong_tick.store(true, Ordering::Relaxed);
            continue;
        };
        let expected = BevyTick::new(check.expected.load(Ordering::Relaxed));
        // SAFETY: these are this chunk's disjoint tick cells. The system must stamp
        // them before entering the callback; no other task may touch these rows.
        if unsafe { *velocity_tick != expected || *pose_tick != expected } {
            check.wrong_tick.store(true, Ordering::Relaxed);
        }
    }
    let result = check.result.load(Ordering::Relaxed);
    if result == RUN_OK {
        // SAFETY: the declared columns are Velocity then PhysicsPose; each callback
        // exclusively owns call.len contiguous rows in both native columns.
        let (velocities, poses) = unsafe {
            (
                (*call.columns).cast::<Velocity>(),
                (*call.columns.add(1)).cast::<PhysicsPose>(),
            )
        };
        for row in 0..call.len as usize {
            // SAFETY: row is within both disjoint chunk columns described above.
            unsafe {
                (*velocities.add(row)).linear[0] += 1.0;
                (*poses.add(row)).position[0] += (*velocities.add(row)).linear[0];
            }
        }
    }
    result
}

unsafe extern "C" fn json(_: *mut c_void, _: *const u8, _: u32) -> i32 {
    0
}
unsafe extern "C" fn boundary(_: *mut c_void, _: *const Tick) -> u32 {
    0
}
unsafe extern "C" fn pending(_: *mut c_void) -> u32 {
    0
}
unsafe extern "C" fn behaviours(_: *mut c_void, _: *const BehaviourCall) -> i32 {
    0
}
unsafe extern "C" fn command(_: *mut c_void, _: *mut CommandCall) -> i32 {
    0
}
unsafe extern "C" fn heat(_: *mut c_void, _: u32, _: u32) -> i32 {
    0
}
unsafe extern "C" fn diag(_: *mut c_void, _: u32) -> u64 {
    0
}
unsafe extern "C" fn shutdown(_: *mut c_void) -> i32 {
    0
}

fn plan(world: &mut World, check: &Check) -> SystemPlan {
    SystemPlan {
        host: std::ptr::from_ref(check) as usize,
        id: 0,
        columns: vec![
            (
                world.register_component::<Velocity>(),
                true,
                size_of::<Velocity>(),
            ),
            (
                world.register_component::<PhysicsPose>(),
                true,
                size_of::<PhysicsPose>(),
            ),
            (
                world.register_component::<PhysicsId>(),
                false,
                size_of::<PhysicsId>(),
            ),
            (
                world.register_component::<Summary>(),
                true,
                size_of::<Summary>(),
            ),
        ],
        resources: Vec::new(),
        with: Vec::new(),
        without: Vec::new(),
        serial: false,
        api: ScriptApi {
            load_modules: json,
            tick_boundary: boundary,
            has_pending: pending,
            run_system: run,
            run_behaviours: behaviours,
            run_command: command,
            set_heat: heat,
            request_compile: json,
            diag,
            shutdown,
        },
    }
}

fn spawn(world: &mut World, i: u64) -> bevy_ecs::entity::Entity {
    let mut entity = world.spawn((
        PhysicsId { value: i },
        Velocity::default(),
        PhysicsPose::default(),
        Summary,
    ));
    if i.is_multiple_of(2) {
        entity.insert(DenseA);
    }
    if i.is_multiple_of(3) {
        entity.insert(DenseB);
    }
    if i.is_multiple_of(5) {
        entity.insert(Sparse);
    }
    entity.id()
}

#[test]
fn chunk_stamps_precede_callbacks_and_preserve_columns_across_archetypes_and_recycled_rows() {
    ComputeTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(4).build());
    let mut world = World::new();
    let entities = (0..12_000)
        .map(|i| spawn(&mut world, i))
        .collect::<Vec<_>>();
    let excluded = world
        .spawn((PhysicsId { value: 99_999 }, Velocity::default()))
        .id();
    let excluded_tick = world
        .entity(excluded)
        .get_change_ticks::<Velocity>()
        .expect("excluded tick");
    let mut check = Box::<Check>::default();
    let mut state: SystemState<(ScriptQuery<'static, 'static>,)> =
        (ScriptQueryBuilder(plan(&mut world, &check)),).build_state(&mut world);
    let mut capacities = None;
    for (iteration, result) in [RUN_OK, RUN_FAULTED, RUN_DISABLED].into_iter().enumerate() {
        if iteration == 1 {
            for &entity in entities.iter().step_by(7) {
                world.despawn(entity);
            }
            for i in 20_000..21_500 {
                spawn(&mut world, i);
            }
        }
        check.refresh(&world);
        check.result.store(result, Ordering::Relaxed);
        let before = check
            .ticks
            .keys()
            .map(|bits| {
                let entity = bevy_ecs::entity::Entity::try_from_bits(*bits).expect("entity");
                let e = world.entity(entity);
                (
                    entity,
                    e.get_change_ticks::<Velocity>()
                        .expect("velocity tick")
                        .added,
                    e.get_change_ticks::<PhysicsId>()
                        .expect("readonly tick")
                        .changed,
                    world.get::<Velocity>(entity).expect("velocity").linear[0],
                )
            })
            .collect::<Vec<_>>();
        let (tick, current_capacities) = {
            let (mut query,) = state.get_mut(&mut world).expect("query");
            query.state.plan.serial = iteration == 2;
            check
                .expected
                .store(query.this_run.get(), Ordering::Relaxed);
            let tick = query.this_run;
            query.run(&Tick {
                frame: iteration as u64,
                seed: 1,
                elapsed: 0.0,
                dt: 1.0,
                reserved: 0,
            });
            (
                tick,
                (
                    query.state.scratch.cols.capacity(),
                    query.state.scratch.changed.capacity(),
                    query.state.scratch.calls.capacity(),
                    query.state.scratch.tables.capacity(),
                    query.state.scratch.sends.capacity(),
                    query.state.buffers.capacity(),
                ),
            )
        };
        if iteration == 2 {
            assert_eq!(
                Some(current_capacities),
                capacities,
                "all pointer/call scratch is reused after warmup"
            );
        }
        capacities = Some(current_capacities);
        assert_eq!(check.rows.load(Ordering::Relaxed), before.len());
        assert!(check.calls.load(Ordering::Relaxed) > 1);
        assert!(
            !check.wrong_tick.load(Ordering::Relaxed),
            "each chunk is stamped before its callback, even faulted/disabled"
        );
        for (entity, added, readonly, value) in before {
            let e = world.entity(entity);
            let ticks = e.get_change_ticks::<Velocity>().expect("written tick");
            assert_eq!(ticks.changed, tick);
            assert_eq!(ticks.added, added);
            assert_eq!(
                e.get_change_ticks::<Summary>()
                    .expect("zero-size tick")
                    .changed,
                tick
            );
            assert_eq!(
                e.get_change_ticks::<PhysicsPose>()
                    .expect("pose tick")
                    .changed,
                tick
            );
            assert_eq!(
                e.get_change_ticks::<PhysicsId>()
                    .expect("readonly tick")
                    .changed,
                readonly
            );
            assert_eq!(
                world.get::<Velocity>(entity).expect("native column").linear[0],
                value + if result == RUN_OK { 1.0 } else { 0.0 }
            );
        }
        let id = world.component_id::<Summary>().expect("summary id");
        for table in world.storages().tables.iter().filter(|t| {
            t.get_column(world.component_id::<PhysicsPose>().expect("pose id"))
                .is_some()
        }) {
            assert_eq!(
                table
                    .get_column(id)
                    .expect("tracked column")
                    .get_summary_tick()
                    .expect("summary")
                    .get(),
                tick
            );
        }
    }
    let after = world
        .entity(excluded)
        .get_change_ticks::<Velocity>()
        .expect("excluded unchanged");
    assert_eq!(after.changed, excluded_tick.changed);
    assert_eq!(after.added, excluded_tick.added);
}

#[test]
fn missing_resource_preserves_existing_row_stamp_side_effect_without_dispatch() {
    ComputeTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(4).build());
    let mut world = World::new();
    for i in 0..5000 {
        spawn(&mut world, i);
    }
    let mut check = Box::<Check>::default();
    check.refresh(&world);
    let mut plan = plan(&mut world, &check);
    plan.resources
        .push((world.register_component::<Missing>(), false));
    let mut state: SystemState<(ScriptQuery<'static, 'static>,)> =
        (ScriptQueryBuilder(plan),).build_state(&mut world);
    let tick = {
        let (mut query,) = state.get_mut(&mut world).expect("query");
        let tick = query.this_run;
        query.run(&Tick {
            frame: 0,
            seed: 1,
            elapsed: 0.0,
            dt: 1.0,
            reserved: 0,
        });
        tick
    };
    assert_eq!(check.calls.load(Ordering::Relaxed), 0);
    for &bits in check.ticks.keys() {
        let e = world.entity(bevy_ecs::entity::Entity::try_from_bits(bits).expect("entity"));
        assert_eq!(
            e.get_change_ticks::<Velocity>()
                .expect("velocity tick")
                .changed,
            tick
        );
        assert_eq!(
            e.get_change_ticks::<PhysicsPose>()
                .expect("pose tick")
                .changed,
            tick
        );
    }
}

#[test]
fn sparse_column_selects_only_matching_rows_and_reuses_one_small_run_buffer() {
    ComputeTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(4).build());
    let mut world = World::new();
    for i in 0..5000 {
        spawn(&mut world, i);
    }
    let mut check = Box::<Check>::default();
    check.refresh(&world);
    let before = check
        .ticks
        .keys()
        .map(|&bits| {
            let entity =
                world.entity(bevy_ecs::entity::Entity::try_from_bits(bits).expect("entity"));
            (
                bits,
                (
                    entity
                        .get_change_ticks::<Velocity>()
                        .expect("velocity tick")
                        .changed,
                    entity
                        .get_change_ticks::<PhysicsPose>()
                        .expect("pose tick")
                        .changed,
                    entity
                        .get_change_ticks::<Summary>()
                        .expect("summary row tick")
                        .changed,
                ),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut plan = plan(&mut world, &check);
    plan.columns.push((
        world.register_component::<Sparse>(),
        true,
        size_of::<Sparse>(),
    ));
    let sparse_id = world.component_id::<Sparse>().expect("sparse id");
    let sparse_entity = world
        .query::<(bevy_ecs::entity::Entity, &Sparse)>()
        .iter(&world)
        .next()
        .expect("sparse row")
        .0;
    let (_, cells) = world
        .storages()
        .sparse_sets
        .get(sparse_id)
        .expect("sparse storage")
        .get_with_ticks(sparse_entity)
        .expect("sparse ticks");
    assert!(
        cells.summary_tick.is_none(),
        "Bevy descriptors forbid summary ticks on sparse storage"
    );
    world.clear_trackers();
    let mut state: SystemState<(ScriptQuery<'static, 'static>,)> =
        (ScriptQueryBuilder(plan),).build_state(&mut world);
    let (tick, buffers) = {
        let (mut query,) = state.get_mut(&mut world).expect("query");
        let tick = query.this_run;
        check.expected.store(tick.get(), Ordering::Relaxed);
        query.run(&Tick {
            frame: 0,
            seed: 1,
            elapsed: 0.0,
            dt: 1.0,
            reserved: 0,
        });
        (tick, query.state.used)
    };
    assert_eq!(check.rows.load(Ordering::Relaxed), 1000);
    assert!(check.calls.load(Ordering::Relaxed) > 0);
    assert!(!check.wrong_tick.load(Ordering::Relaxed));
    assert_eq!(
        buffers, 1,
        "small fragmented queries share one serial command buffer"
    );
    let changed = world
        .query_filtered::<bevy_ecs::entity::Entity, bevy_ecs::query::Changed<Sparse>>()
        .iter(&world)
        .count();
    assert_eq!(
        changed, 1000,
        "typed sparse Changed filters observe per-entity stamps"
    );
    for &bits in check.ticks.keys() {
        let entity = bevy_ecs::entity::Entity::try_from_bits(bits).expect("entity");
        let e = world.entity(entity);
        let selected = world.get::<Sparse>(entity).is_some();
        assert_eq!(
            e.get_change_ticks::<Velocity>()
                .expect("velocity tick")
                .changed,
            if selected { tick } else { before[&bits].0 }
        );
        assert_eq!(
            e.get_change_ticks::<PhysicsPose>()
                .expect("pose tick")
                .changed,
            if selected { tick } else { before[&bits].1 }
        );
        assert_eq!(
            e.get_change_ticks::<Summary>()
                .expect("summary row tick")
                .changed,
            if selected { tick } else { before[&bits].2 }
        );
        assert_eq!(
            world
                .get::<Velocity>(entity)
                .expect("unchanged column")
                .linear,
            [if selected { 1.0 } else { 0.0 }, 0.0, 0.0]
        );
        if selected {
            assert_eq!(
                e.get_change_ticks::<Sparse>()
                    .expect("sparse write tick")
                    .changed,
                tick
            );
        }
    }
}

#[test]
fn sparse_default_disabling_filters_use_archetypes_instead_of_shared_tables() {
    ComputeTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(4).build());
    let mut world = World::new();
    world.register_disabling_component::<Sparse>();
    for i in 0..5000 {
        spawn(&mut world, i);
    }
    let mut check = Box::<Check>::default();
    check.refresh(&world);
    let before = check
        .ticks
        .keys()
        .map(|&bits| {
            let entity =
                world.entity(bevy_ecs::entity::Entity::try_from_bits(bits).expect("entity"));
            (
                bits,
                entity
                    .get_change_ticks::<Velocity>()
                    .expect("original tick")
                    .changed,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut state: SystemState<(ScriptQuery<'static, 'static>,)> =
        (ScriptQueryBuilder(plan(&mut world, &check)),).build_state(&mut world);
    let tick = {
        let (mut query,) = state.get_mut(&mut world).expect("query");
        assert!(
            !query.state.dense,
            "default sparse filters participate in dense eligibility"
        );
        let tick = query.this_run;
        check.expected.store(tick.get(), Ordering::Relaxed);
        query.run(&Tick {
            frame: 0,
            seed: 1,
            elapsed: 0.0,
            dt: 1.0,
            reserved: 0,
        });
        tick
    };
    assert_eq!(check.rows.load(Ordering::Relaxed), 4000);
    assert!(!check.wrong_tick.load(Ordering::Relaxed));
    for &bits in check.ticks.keys() {
        let entity = bevy_ecs::entity::Entity::try_from_bits(bits).expect("entity");
        let enabled = world.get::<Sparse>(entity).is_none();
        assert_eq!(
            world.get::<Velocity>(entity).expect("velocity").linear[0],
            if enabled { 1.0 } else { 0.0 }
        );
        assert_eq!(
            world
                .entity(entity)
                .get_change_ticks::<Velocity>()
                .expect("filtered tick")
                .changed,
            if enabled { tick } else { before[&bits] }
        );
    }
}
