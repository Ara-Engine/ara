//! Sparse query streaming. Packet row ranges and local pointer grids live only
//! under the parent query's declared access; component payloads remain in ECS.
use core::ffi::c_void;
use std::cell::UnsafeCell;

use bevy_ecs::archetype::ArchetypeId;
use bevy_ecs::change_detection::Tick as BevyTick;
use bevy_ecs::component::ComponentId;
use bevy_ecs::query::QueryState;
use bevy_ecs::storage::{Column, ComponentSparseSet, TableRow};
use bevy_ecs::world::FilteredEntityMut;
use bevy_ecs::world::unsafe_world_cell::UnsafeWorldCell;
use bevy_tasks::ComputeTaskPool;

use super::{MIN_CHUNK_ROWS, PARALLEL_MIN_ROWS, ScriptQuery, SystemPlan, stamp_changed};
use crate::abi::{SystemCall, Tick};
use crate::ecs::cmdbuf::CmdBuf;

/// A logical row range; unlike component pointers it is cheap to retain per packet.
#[derive(Clone, Copy)]
struct SparseRange {
    archetype: ArchetypeId,
    first: usize,
    len: usize,
}

/// Metadata cached once for an archetype range, never a component payload copy.
#[derive(Clone, Copy)]
enum SparseColumn {
    Table {
        column: *const Column,
        changed: *const UnsafeCell<BevyTick>,
    },
    Sparse {
        set: *const ComponentSparseSet,
        write: bool,
    },
}

impl SparseColumn {
    /// # Safety
    /// The row is live in this accessor's archetype; declared query access permits
    /// the returned pointers, and storage stays fixed until this packet completes.
    unsafe fn fetch(
        self,
        entity: bevy_ecs::entity::Entity,
        row: TableRow,
    ) -> (*mut u8, *const UnsafeCell<BevyTick>) {
        match self {
            Self::Table { column, changed } => {
                // SAFETY: the caller supplies the actual table row and live column.
                let data = unsafe { (*column).get_data_unchecked(row) }
                    .as_ptr()
                    .cast_mut();
                let tick = if changed.is_null() {
                    changed
                } else {
                    // SAFETY: the changed grid covers every row in this table.
                    unsafe { changed.add(row.index()) }
                };
                (data, tick)
            }
            Self::Sparse { set, write } => {
                // SAFETY: the set is live and immutable metadata for this query run.
                let set = unsafe { &*set };
                if write {
                    let (data, cells) = set
                        .get_with_ticks(entity)
                        .expect("matched sparse range contains its component");
                    (data.as_ptr().cast_mut(), std::ptr::from_ref(cells.changed))
                } else {
                    (
                        set.get(entity)
                            .expect("matched sparse range contains its component")
                            .as_ptr()
                            .cast_mut(),
                        std::ptr::null(),
                    )
                }
            }
        }
    }
}

/// One worker's reusable metadata. IDs cover only its pending contiguous callback.
#[derive(Default)]
struct SparsePacket {
    ranges: Vec<SparseRange>,
    accessors: Vec<SparseColumn>,
    entities: Vec<u64>,
    columns: Vec<*mut u8>,
    changed: Vec<*const UnsafeCell<BevyTick>>,
    next_columns: Vec<*mut u8>,
    next_changed: Vec<*const UnsafeCell<BevyTick>>,
}

// SAFETY: a packet is exclusively owned by one worker while its raw pointers are
// live. They are never dereferenced between runs or outside declared query access.
unsafe impl Send for SparsePacket {}
// SAFETY: packet mutation/dereferencing requires an exclusive borrow; shared access
// only observes owned metadata, and no component reference is stored here.
unsafe impl Sync for SparsePacket {}

impl SparsePacket {
    fn reset(&mut self, columns: usize) {
        self.ranges.clear();
        self.entities.clear();
        self.accessors.clear();
        self.columns.resize(columns, std::ptr::null_mut());
        self.changed.resize(columns, std::ptr::null());
        self.next_columns.resize(columns, std::ptr::null_mut());
        self.next_changed.resize(columns, std::ptr::null());
    }
}

#[derive(Default)]
pub(super) struct SparseScratch {
    resources: Vec<*mut u8>,
    packets: Vec<SparsePacket>,
}

#[cfg(test)]
pub(super) struct ScratchStats {
    pub(super) bytes: usize,
    pub(super) pending_id_capacity: usize,
    pub(super) max_pending_id_capacity: usize,
}

// SAFETY: resource pointers share the owning system's run-scoped access contract;
// packet ownership is exclusive, and all pointers are dead between query runs.
unsafe impl Send for SparseScratch {}
// SAFETY: shared state never dereferences these resources; run requires &mut state.
unsafe impl Sync for SparseScratch {}

impl SparseScratch {
    #[cfg(test)]
    pub(super) fn retained_stats(&self) -> ScratchStats {
        let bytes = self.resources.capacity() * size_of::<*mut u8>()
            + self.packets.capacity() * size_of::<SparsePacket>()
            + self
                .packets
                .iter()
                .map(|packet| {
                    packet.ranges.capacity() * size_of::<SparseRange>()
                        + packet.accessors.capacity() * size_of::<SparseColumn>()
                        + packet.entities.capacity() * size_of::<u64>()
                        + (packet.columns.capacity() + packet.next_columns.capacity())
                            * size_of::<*mut u8>()
                        + (packet.changed.capacity() + packet.next_changed.capacity())
                            * size_of::<*const UnsafeCell<BevyTick>>()
                })
                .sum::<usize>();
        ScratchStats {
            bytes,
            pending_id_capacity: self
                .packets
                .iter()
                .map(|packet| packet.entities.capacity())
                .sum(),
            max_pending_id_capacity: self
                .packets
                .iter()
                .map(|packet| packet.entities.capacity())
                .max()
                .unwrap_or(0),
        }
    }

    fn prepare(
        &mut self,
        world: UnsafeWorldCell<'_>,
        query: &QueryState<FilteredEntityMut<'static, 'static>>,
        rows: usize,
        batch: usize,
        parallel: bool,
        columns: usize,
    ) -> usize {
        let count = if parallel { rows.div_ceil(batch) } else { 1 };
        if self.packets.len() < count {
            self.packets.resize_with(count, SparsePacket::default);
        }
        for packet in &mut self.packets[..count] {
            packet.reset(columns);
        }
        let archetypes = world.archetypes();
        let mut packet = 0;
        let mut used = 0;
        for archetype in query.matched_archetypes() {
            let len = archetypes[archetype].len() as usize;
            let mut first = 0;
            while first < len {
                let n = if parallel {
                    (batch - used).min(len - first)
                } else {
                    len - first
                };
                self.packets[packet].ranges.push(SparseRange {
                    archetype,
                    first,
                    len: n,
                });
                first += n;
                if parallel {
                    used += n;
                    if used == batch {
                        packet += 1;
                        used = 0;
                    }
                }
            }
        }
        count
    }
}

/// Copyable, run-scoped permissions; only disjoint packet ranges may use it in parallel.
#[derive(Clone, Copy)]
struct SparseContext<'w, 'a> {
    world: UnsafeWorldCell<'w>,
    plan: &'a SystemPlan,
    tick: &'a Tick,
    this_run: BevyTick,
    resources: *const *mut u8,
    batch: usize,
}

// SAFETY: the caller keeps metadata/resources live through the task scope. Parallel
// packets own disjoint rows/buffers, and resource writers execute only on the caller.
unsafe impl Send for SparseContext<'_, '_> {}
// SAFETY: the context is immutable metadata. Dereferencing its component/resource
// pointers requires the same unsafe, disjoint-access contract as run itself.
unsafe impl Sync for SparseContext<'_, '_> {}

impl SparseContext<'_, '_> {
    /// # Safety
    /// This packet owns disjoint matched rows and its buffer until scope completion;
    /// no structural changes or conflicting references may exist while calls run.
    /// Parallel calls may share resources only when every resource is read-only.
    unsafe fn run(self, packet: &mut SparsePacket, buffer: &mut CmdBuf) {
        let archetypes = self.world.archetypes();
        // SAFETY: declared query access covers every component fetched below.
        let storages = unsafe { self.world.storages() };
        for range_index in 0..packet.ranges.len() {
            let range = packet.ranges[range_index];
            let archetype = &archetypes[range.archetype];
            let table = &storages.tables[archetype.table_id()];
            packet.accessors.clear();
            for &(id, write, _) in &self.plan.columns {
                let accessor = if let Some(column) = table.get_column(id) {
                    let changed = if write {
                        // SAFETY: the grid uses this table's actual row count.
                        unsafe { column.get_changed_ticks_slice(table.entity_count() as usize) }
                            .as_ptr()
                    } else {
                        std::ptr::null()
                    };
                    SparseColumn::Table { column, changed }
                } else {
                    SparseColumn::Sparse {
                        set: storages
                            .sparse_sets
                            .get(id)
                            .expect("matched archetype column has sparse storage"),
                        write,
                    }
                };
                packet.accessors.push(accessor);
            }
            for row in &archetype.entities()[range.first..range.first + range.len] {
                for (column, accessor) in packet.accessors.iter().enumerate() {
                    // SAFETY: the range is part of this accessor's matched archetype.
                    let (data, tick) = unsafe { accessor.fetch(row.id(), row.table_row()) };
                    packet.next_columns[column] = data;
                    packet.next_changed[column] = tick;
                }
                let len = packet.entities.len();
                let contiguous = len != 0
                    && self
                        .plan
                        .columns
                        .iter()
                        .enumerate()
                        .all(|(column, &(_, _, size))| {
                            // SAFETY: one archetype uses fixed allocations. The pending run
                            // is contiguous by induction; its next slot may be one-past.
                            let next = unsafe { packet.columns[column].add(len * size) };
                            let changed = packet.changed[column];
                            let next_tick = if changed.is_null() {
                                changed
                            } else {
                                // SAFETY: the same contiguous range proof covers tick cells.
                                unsafe { changed.add(len) }
                            };
                            next == packet.next_columns[column]
                                && next_tick == packet.next_changed[column]
                        });
                if len != 0 && !contiguous {
                    // SAFETY: the pending run owns its selected contiguous cells.
                    unsafe { self.flush(packet, buffer) };
                }
                if packet.entities.is_empty() {
                    packet.columns.copy_from_slice(&packet.next_columns);
                    packet.changed.copy_from_slice(&packet.next_changed);
                }
                packet.entities.push(row.id().to_bits());
                if packet.entities.len() == self.batch {
                    // SAFETY: callback boundaries match the previous batch limit.
                    unsafe { self.flush(packet, buffer) };
                }
            }
            // Archetype and packet boundaries are observable batch boundaries.
            if !packet.entities.is_empty() {
                // SAFETY: the current range finishes before changing allocations.
                unsafe { self.flush(packet, buffer) };
            }
        }
    }

    /// # Safety
    /// The pending run's pointers and IDs are live, contiguous and exclusively owned.
    unsafe fn flush(self, packet: &mut SparsePacket, buffer: &mut CmdBuf) {
        let len = packet.entities.len();
        // SAFETY: only write columns carry pointers; each covers this pending run.
        unsafe { stamp_changed(&packet.changed, len, self.this_run) };
        let call = SystemCall {
            system: self.plan.id,
            len: u32::try_from(len).unwrap_or(u32::MAX),
            entities: packet.entities.as_ptr(),
            columns: packet.columns.as_ptr(),
            resources: self.resources,
            tick: self.tick,
            commands: std::ptr::from_mut(buffer).cast(),
        };
        // SAFETY: the stack call and owned grids/IDs remain unchanged until C# returns.
        unsafe { (self.plan.api.run_system)(self.plan.host as *mut c_void, &call) };
        packet.entities.clear();
    }
}

impl ScriptQuery<'_, '_> {
    /// Sparse execution keeps only row-range packets; workers resolve each pending
    /// callback directly from frozen ECS storage rather than retaining per-row calls.
    pub(super) fn run_sparse(&mut self, tick: &Tick) {
        let st = &mut *self.state;
        st.used = 0;
        let archetypes = self.world.archetypes();
        // SAFETY: metadata is readable and column writes were registered by SystemParam.
        let storages = unsafe { self.world.storages() };
        let mut total = 0;
        for id in st.query.matched_archetypes() {
            let archetype = &archetypes[id];
            if archetype.is_empty() {
                continue;
            }
            total += archetype.len() as usize;
            let table = &storages.tables[archetype.table_id()];
            for &(id, write, _) in &st.plan.columns {
                if write
                    && let Some(column) = table.get_column(id)
                    && let Some(summary) = column.get_summary_tick()
                {
                    summary.set(self.this_run);
                }
            }
        }
        st.sparse.resources.clear();
        // Keep resource lookup before the empty-row return, matching dense and the
        // previous sparse path's mutable-resource change-tick side effects.
        for &(id, write) in &st.plan.resources {
            // SAFETY: the resource access was declared in init_access.
            let pointer = unsafe {
                if write {
                    self.world
                        .get_resource_mut_by_id(id)
                        .map(|value| value.into_inner().as_ptr())
                } else {
                    self.world
                        .get_resource_by_id(id)
                        .map(|value| value.as_ptr().cast_mut())
                }
            };
            match pointer {
                Some(pointer) => st.sparse.resources.push(pointer),
                None => {
                    // SAFETY: no workers or callbacks started. Mark every selected
                    // writable row exactly as the previous missing-resource path.
                    unsafe {
                        stamp_sparse_missing(self.world, &st.query, &st.plan.columns, self.this_run)
                    };
                    return;
                }
            }
        }
        if total == 0 {
            return;
        }
        let pool = ComputeTaskPool::get();
        let batch = if total < PARALLEL_MIN_ROWS {
            usize::MAX
        } else {
            (total / (pool.thread_num().max(1) * 4)).max(MIN_CHUNK_ROWS)
        };
        let parallel = !st.plan.serial && total >= PARALLEL_MIN_ROWS;
        let count = st.sparse.prepare(
            self.world,
            &st.query,
            total,
            batch,
            parallel,
            st.plan.columns.len(),
        );
        if st.buffers.len() < count {
            st.buffers.resize_with(count, CmdBuf::default);
        }
        for buffer in &mut st.buffers[..count] {
            buffer.reset(self.world.entity_allocator());
        }
        st.used = count;
        let context = SparseContext {
            world: self.world,
            plan: &st.plan,
            tick,
            this_run: self.this_run,
            resources: st.sparse.resources.as_ptr(),
            batch,
        };
        if !parallel {
            // SAFETY: all selected ranges and the single buffer are owned by the
            // caller; this includes every Commands/resource-writing system.
            unsafe { context.run(&mut st.sparse.packets[0], &mut st.buffers[0]) };
            return;
        }
        pool.scope(|scope| {
            for (packet, buffer) in st.sparse.packets[..count]
                .iter_mut()
                .zip(&mut st.buffers[..count])
            {
                let context = &context;
                scope.spawn(async move {
                    // SAFETY: packets partition matched rows in logical order.
                    // Storage is frozen, resources are read-only and buffers unique.
                    unsafe { context.run(packet, buffer) };
                });
            }
        });
    }
}

/// # Safety
/// The query's selected rows are writable under declared filtered access. No
/// callbacks or structural mutations run while this cold fallback stamps them.
unsafe fn stamp_sparse_missing(
    world: UnsafeWorldCell<'_>,
    query: &QueryState<FilteredEntityMut<'static, 'static>>,
    columns: &[(ComponentId, bool, usize)],
    tick: BevyTick,
) {
    let archetypes = world.archetypes();
    // SAFETY: metadata stays fixed and the caller supplies declared write access.
    let storages = unsafe { world.storages() };
    for id in query.matched_archetypes() {
        let archetype = &archetypes[id];
        let table = &storages.tables[archetype.table_id()];
        for row in archetype.entities() {
            for &(id, write, _) in columns {
                if !write {
                    continue;
                }
                let cell = if let Some(column) = table.get_column(id) {
                    // SAFETY: the row belongs to this archetype's live table.
                    unsafe { column.get_changed_tick_unchecked(row.table_row()) }
                } else {
                    storages
                        .sparse_sets
                        .get(id)
                        .expect("matched sparse column")
                        .get_changed_tick(row.id())
                        .expect("matched sparse row")
                };
                // SAFETY: this selected row is exclusively writable under query access.
                unsafe { *cell.get() = tick };
            }
        }
    }
}
