//! Streaming boundary, ordering, tick and retained-metadata checks.
use super::*;

/// Independent typed-world oracle for physical runs and the established batch cuts.
fn expected_sparse_trace(world: &World, batch: usize, parallel: bool) -> Vec<Vec<u64>> {
    fn flush(
        run: &mut Vec<u64>,
        trace: &mut Vec<Vec<u64>>,
        used: &mut usize,
        batch: usize,
        parallel: bool,
    ) {
        let mut first = 0;
        while first < run.len() {
            let budget = if parallel { batch - *used } else { batch };
            let len = budget.min(run.len() - first);
            trace.push(run[first..first + len].to_vec());
            first += len;
            if parallel {
                *used = (*used + len) % batch;
            }
        }
        run.clear();
    }
    let sparse = world.component_id::<Sparse>().expect("sparse filter");
    let velocity = world.component_id::<Velocity>().expect("velocity");
    let pose = world.component_id::<PhysicsPose>().expect("pose");
    let mut trace = Vec::new();
    let mut run = Vec::new();
    let mut used = 0;
    for archetype in world
        .archetypes()
        .iter()
        .filter(|a| a.contains(sparse) && a.contains(velocity) && a.contains(pose))
    {
        let mut previous = None;
        for row in archetype.entities() {
            let addresses = (
                std::ptr::from_ref(world.get::<Velocity>(row.id()).expect("typed velocity"))
                    as usize,
                std::ptr::from_ref(world.get::<PhysicsPose>(row.id()).expect("typed pose"))
                    as usize,
            );
            if let Some((v, p)) = previous
                && addresses != (v + size_of::<Velocity>(), p + size_of::<PhysicsPose>())
            {
                flush(&mut run, &mut trace, &mut used, batch, parallel);
            }
            run.push(row.id().to_bits());
            previous = Some(addresses);
        }
        flush(&mut run, &mut trace, &mut used, batch, parallel);
    }
    trace
}

fn take_sparse_trace(query: &mut ScriptQuery<'_, '_>) -> Vec<Vec<u64>> {
    query.state.buffers[..query.state.used]
        .iter_mut()
        .flat_map(|buffer| std::mem::take(&mut buffer.ops))
        .map(|op| {
            assert_eq!(op.op, crate::abi::CMD_CUE);
            let (ids, remaining) = op.data.as_chunks::<8>();
            assert!(remaining.is_empty(), "trace contains complete entity IDs");
            ids.iter().map(|bytes| u64::from_ne_bytes(*bytes)).collect()
        })
        .collect()
}

fn retained_query_scratch(state: &ScriptQueryState) -> usize {
    let dense = &state.scratch;
    dense.cols.capacity() * size_of::<*mut u8>()
        + dense.changed.capacity() * size_of::<*const UnsafeCell<BevyTick>>()
        + dense.res.capacity() * size_of::<*mut u8>()
        + dense.calls.capacity() * size_of::<ChunkCall>()
        + dense.tables.capacity() * size_of::<(*const u64, usize, usize)>()
        + dense.sends.capacity() * size_of::<SendPtr>()
        + state.sparse.retained_stats().bytes
        + state.buffers.capacity() * size_of::<CmdBuf>()
}

#[test]
fn streaming_preserves_callback_boundaries_packet_order_ticks_and_faulted_calls_after_churn() {
    ComputeTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(4).build());
    for serial in [true, false] {
        let mut world = World::new();
        let mut sources = Vec::new();
        for block in 0..2 {
            for index in 0..5003 {
                let mut entity = world.spawn((
                    PhysicsId {
                        value: (block * 5003 + index) as u64,
                    },
                    Velocity::default(),
                    PhysicsPose::default(),
                    Summary,
                    Sparse,
                ));
                if block == 1 {
                    entity.insert(DenseA);
                }
                sources.push(entity.id());
            }
        }
        let mut check = Box::<Check>::default();
        check.capture_trace.store(true, Ordering::Relaxed);
        let mut plan = plan(&mut world, &check);
        plan.with
            .push(world.component_id::<Sparse>().expect("sparse filter"));
        plan.serial = serial;
        let mut state: SystemState<(ScriptQuery<'static, 'static>,)> =
            (ScriptQueryBuilder(plan),).build_state(&mut world);
        let mut warmed_bytes = None;
        for (iteration, result) in [RUN_OK, RUN_FAULTED, RUN_DISABLED].into_iter().enumerate() {
            if iteration == 1 {
                for &entity in sources.iter().step_by(7) {
                    world.despawn(entity);
                }
                for index in 20_000..20_137 {
                    spawn(&mut world, index);
                }
            }
            check.refresh(&world);
            check.result.store(result, Ordering::Relaxed);
            let selected = world.query::<&Sparse>().iter(&world).count();
            let batch =
                (selected / (ComputeTaskPool::get().thread_num().max(1) * 4)).max(MIN_CHUNK_ROWS);
            let expected = expected_sparse_trace(&world, batch, !serial);
            let (actual, bytes) = {
                let (mut query,) = state.get_mut(&mut world).expect("query");
                check
                    .expected
                    .store(query.this_run.get(), Ordering::Relaxed);
                query.run(&Tick {
                    frame: iteration as u64,
                    seed: 1,
                    elapsed: 0.0,
                    dt: 1.0,
                    reserved: 0,
                });
                assert_eq!(
                    query.state.scratch.calls.capacity(),
                    0,
                    "sparse runs retain no ChunkCall array"
                );
                assert_eq!(
                    query.state.scratch.cols.capacity(),
                    0,
                    "sparse runs retain no per-row dense grid"
                );
                (
                    take_sparse_trace(&mut query),
                    retained_query_scratch(query.state),
                )
            };
            assert_eq!(
                actual, expected,
                "same world/budget preserves every callback ID slice and length"
            );
            assert_eq!(check.rows.load(Ordering::Relaxed), selected);
            assert!(
                !check.wrong_tick.load(Ordering::Relaxed),
                "faulted/disabled calls still receive stamped rows"
            );
            if iteration == 2 {
                assert_eq!(Some(bytes), warmed_bytes, "steady packet scratch is reused");
            }
            warmed_bytes = Some(bytes);
        }
    }
}

#[test]
fn fragmented_streaming_retains_packet_scratch_instead_of_per_entity_calls() {
    ComputeTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(4).build());
    let mut world = World::new();
    for index in 0..100_000u64 {
        let mut entity = world.spawn((
            PhysicsId { value: index },
            Velocity::default(),
            PhysicsPose::default(),
            Summary,
        ));
        if index.is_multiple_of(2) {
            entity.insert(Sparse);
        }
    }
    let mut check = Box::<Check>::default();
    check.refresh(&world);
    let mut plan = plan(&mut world, &check);
    plan.with
        .push(world.component_id::<Sparse>().expect("sparse filter"));
    let mut state: SystemState<(ScriptQuery<'static, 'static>,)> =
        (ScriptQueryBuilder(plan),).build_state(&mut world);
    let mut previous = None;
    for _ in 0..2 {
        check.rows.store(0, Ordering::Relaxed);
        check.wrong_tick.store(false, Ordering::Relaxed);
        let (mut query,) = state.get_mut(&mut world).expect("query");
        check
            .expected
            .store(query.this_run.get(), Ordering::Relaxed);
        query.run(&Tick {
            frame: 0,
            seed: 1,
            elapsed: 0.0,
            dt: 1.0,
            reserved: 0,
        });
        let bytes = retained_query_scratch(query.state);
        assert!(
            bytes < 128 * 1024,
            "50k fragmented rows must not retain row descriptors: {bytes}"
        );
        assert_eq!(query.state.scratch.calls.capacity(), 0);
        assert_eq!(check.rows.load(Ordering::Relaxed), 50_000);
        assert!(!check.wrong_tick.load(Ordering::Relaxed));
        assert!(query.state.sparse.retained_stats().max_pending_id_capacity <= 16);
        if let Some(previous) = previous {
            assert_eq!(bytes, previous, "warm scratch capacity is stable");
        }
        previous = Some(bytes);
        if let Some(path) = std::env::var_os("ARA_SPARSE_SCRATCH_OUTPUT") {
            let output = serde_json::json!({"live_rows":100_000,"selected_rows":50_000,
                "pool_workers":ComputeTaskPool::get().thread_num(),"caller_participates":true,
                "retained_scratch_bytes":bytes,"packets":query.state.used,
                "pending_id_capacity":query.state.sparse.retained_stats().pending_id_capacity,
                "retained_chunk_call_capacity":query.state.scratch.calls.capacity(),
                "scope":"retained query metadata; excludes plan/schema/world and transient task allocations","timed":false});
            std::fs::write(
                path,
                serde_json::to_vec_pretty(&output).expect("scratch JSON"),
            )
            .expect("opt-in scratch measurement");
        }
    }
}

#[test]
fn streaming_missing_resource_marks_selected_sparse_and_table_ticks_without_callbacks() {
    ComputeTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(4).build());
    let mut world = World::new();
    for index in 0..10_000 {
        spawn(&mut world, index);
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
    let mut plan = plan(&mut world, &check);
    plan.columns.push((
        world.component_id::<Sparse>().expect("sparse column"),
        true,
        size_of::<Sparse>(),
    ));
    plan.resources
        .push((world.register_component::<Missing>(), false));
    world.clear_trackers();
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
        assert_eq!(query.state.used, 0);
        tick
    };
    assert_eq!(check.calls.load(Ordering::Relaxed), 0);
    for &bits in check.ticks.keys() {
        let entity = bevy_ecs::entity::Entity::try_from_bits(bits).expect("entity");
        let row = world.entity(entity);
        let selected = world.get::<Sparse>(entity).is_some();
        let expected = if selected { tick } else { before[&bits] };
        assert_eq!(
            row.get_change_ticks::<Velocity>()
                .expect("table tick")
                .changed,
            expected
        );
        assert_eq!(
            row.get_change_ticks::<PhysicsPose>()
                .expect("other table tick")
                .changed,
            expected
        );
        assert_eq!(
            row.get_change_ticks::<Summary>()
                .expect("summary row tick")
                .changed,
            expected
        );
        if selected {
            assert_eq!(
                row.get_change_ticks::<Sparse>()
                    .expect("sparse tick")
                    .changed,
                tick
            );
        }
    }
}
