//! C# systems as bevy systems. [`ScriptQuery`] is a custom `SystemParam` built from a
//! system's declared columns and resources at runtime: it registers that access with
//! the scheduler (so non-conflicting systems run in parallel), walks the matched tables
//! for column base pointers, splits them into chunks and runs one C# call per chunk on
//! the compute task pool. Structural ops land in per-chunk buffers, applied at bevy
//! sync points in chunk order.

use core::ffi::c_void;
use std::cell::UnsafeCell;

use bevy_ecs::change_detection::Tick as BevyTick;
use bevy_ecs::component::{ComponentId, IS_RESOURCE};
use bevy_ecs::query::{FilteredAccess, QueryBuilder, QueryState};
use bevy_ecs::storage::TableRow;
use bevy_ecs::system::{
    SystemAccess, SystemMeta, SystemParam, SystemParamBuilder, SystemParamValidationError,
};
use bevy_ecs::world::unsafe_world_cell::UnsafeWorldCell;
use bevy_ecs::world::{FilteredEntityMut, World};
use bevy_tasks::ComputeTaskPool;
use nonmax::NonMaxU32;

use crate::abi::{ScriptApi, SystemCall, Tick};
use crate::ecs::cmdbuf::{self, CmdBuf, ScriptCatalog, ScriptEvents};

/// Rows below this run on the calling thread (no task-pool dispatch).
const PARALLEL_MIN_ROWS: usize = 4096;
/// Smallest chunk handed to one task.
const MIN_CHUNK_ROWS: usize = 1024;

/// One C# system's declared access, resolved to bevy component ids.
#[derive(Clone, Debug)]
pub(crate) struct SystemPlan {
    /// Stable host context handle, carried as an integer across task-pool jobs.
    pub host: usize,
    /// System id (`module << 16 | index`).
    pub id: u32,
    /// Columns in parameter order: `(component, write, element size)`.
    pub columns: Vec<(ComponentId, bool, usize)>,
    /// Resources in parameter order: `(resource component, write)`.
    pub resources: Vec<(ComponentId, bool)>,
    /// Components the entity must also have.
    pub with: Vec<ComponentId>,
    /// Components the entity must not have.
    pub without: Vec<ComponentId>,
    /// Run chunks one after another on the calling thread: the system writes a resource
    /// (shared by every chunk) or makes structural changes (entity ids must be reserved in
    /// a deterministic order).
    pub serial: bool,
    /// The C# entry points.
    pub api: ScriptApi,
}

/// Per-run pointers: only valid during one run of the system that owns them.
#[derive(Default)]
struct Scratch {
    /// Column base pointers, `columns.len()` per chunk.
    cols: Vec<*mut u8>,
    /// Changed-tick bases parallel to `cols`; null for read-only columns.
    changed: Vec<*const UnsafeCell<BevyTick>>,
    /// Resource pointers.
    res: Vec<*mut u8>,
    /// One call per chunk.
    calls: Vec<ChunkCall>,
    /// `(entities, len, first base index in cols)` per table.
    tables: Vec<(*const u64, usize, usize)>,
    /// `calls` as task-pool handles.
    sends: Vec<SendPtr>,
}

// SAFETY: the pointers in `Scratch` are written and read only inside one run of the
// owning system (bevy gives a system exclusive use of its state); they are dead
// between runs and never dereferenced outside the access the system declared.
unsafe impl Send for Scratch {}
// SAFETY: see above; nothing reads a `Scratch` through a shared reference.
unsafe impl Sync for Scratch {}

/// State of a [`ScriptQuery`]: the plan, the bevy query and reusable buffers.
pub(crate) struct ScriptQueryState {
    plan: SystemPlan,
    query: QueryState<FilteredEntityMut<'static, 'static>>,
    scratch: Scratch,
    buffers: Vec<CmdBuf>,
    used: usize,
}

/// The system param that runs one C# system over its matched tables.
pub(crate) struct ScriptQuery<'w, 's> {
    world: UnsafeWorldCell<'w>,
    state: &'s mut ScriptQueryState,
    this_run: BevyTick,
}

/// Builds a [`ScriptQuery`]'s state from a [`SystemPlan`].
pub(crate) struct ScriptQueryBuilder(pub SystemPlan);

// SAFETY: `build` creates the state from the world the system is built for, which is
// what `ScriptQuery::init_access` and `get_param` require.
unsafe impl<'w, 's> SystemParamBuilder<ScriptQuery<'w, 's>> for ScriptQueryBuilder {
    fn build(self, world: &mut World) -> ScriptQueryState {
        let mut b = QueryBuilder::<FilteredEntityMut<'static, 'static>>::new(world);
        for (id, write, _) in &self.0.columns {
            if *write {
                b.mut_id(*id);
            } else {
                b.ref_id(*id);
            }
        }
        for id in &self.0.with {
            b.with_id(*id);
        }
        for id in &self.0.without {
            b.without_id(*id);
        }
        ScriptQueryState {
            query: b.build(),
            plan: self.0,
            scratch: Scratch::default(),
            buffers: Vec::new(),
            used: 0,
        }
    }
}

// SAFETY: `init_access` registers every component the system reads or writes (through
// the bevy query) and every resource, so the scheduler never runs it alongside a
// conflicting system; `get_param` only touches data within that access.
unsafe impl SystemParam for ScriptQuery<'_, '_> {
    type State = ScriptQueryState;
    type Item<'w, 's> = ScriptQuery<'w, 's>;

    fn init_state(_world: &mut World) -> Self::State {
        unreachable!("ScriptQuery state is always built by ScriptQueryBuilder")
    }

    fn init_access(
        state: &Self::State,
        meta: &mut SystemMeta,
        access: &mut SystemAccess,
        world: &mut World,
    ) {
        let set = access.require_shared_access::<Self>(meta);
        state
            .query
            .init_access(None, set, world.as_unsafe_world_cell_readonly());
        for (id, write) in &state.plan.resources {
            let mut filter = FilteredAccess::default();
            if *write {
                filter.add_write(*id);
            } else {
                filter.add_read(*id);
            }
            filter.and_with(IS_RESOURCE);
            if access.try_add(filter).is_err() {
                panic!("C# system {} declares a resource twice", state.plan.id);
            }
        }
        meta.set_has_deferred();
    }

    fn apply(state: &mut Self::State, _meta: &SystemMeta, world: &mut World) {
        let used = std::mem::take(&mut state.used);
        if state.buffers[..used].iter().all(|b| b.ops.is_empty()) {
            return;
        }
        world.resource_scope(|world, catalog: bevy_ecs::prelude::Mut<ScriptCatalog>| {
            world.resource_scope(|world, mut events: bevy_ecs::prelude::Mut<ScriptEvents>| {
                let ScriptEvents { cues, problems } = &mut *events;
                for buf in &mut state.buffers[..used] {
                    cmdbuf::apply(world, &catalog, &buf.ops, cues, problems, None);
                    buf.ops.clear();
                }
            });
        });
    }

    unsafe fn get_param<'w, 's>(
        state: &'s mut Self::State,
        _meta: &SystemMeta,
        world: UnsafeWorldCell<'w>,
        change_tick: BevyTick,
    ) -> Result<Self::Item<'w, 's>, SystemParamValidationError> {
        state.query.update_archetypes_unsafe_world_cell(world);
        Ok(ScriptQuery {
            world,
            state,
            this_run: change_tick,
        })
    }
}

/// A raw pointer that may cross into a task-pool task.
#[derive(Clone, Copy)]
struct SendPtr(*const ChunkCall);

// SAFETY: each pointer is handed to exactly one task, points into the system's scratch
// (alive for the whole scope) and the call it describes covers rows no other task sees.
unsafe impl Send for SendPtr {}

impl SendPtr {
    fn get(self) -> *const ChunkCall {
        self.0
    }
}

/// Internal metadata leaves the C# ABI and the task's single-pointer capture unchanged.
struct ChunkCall {
    call: SystemCall,
    changed: *const [*const UnsafeCell<BevyTick>],
    this_run: BevyTick,
}

impl ChunkCall {
    /// # Safety
    /// The call and pointer grid are live and frozen for the task scope; this chunk
    /// exclusively owns its data and changed-tick rows under the declared access.
    unsafe fn run(
        &self,
        host: usize,
        run: unsafe extern "C" fn(*mut c_void, *const SystemCall) -> i32,
    ) {
        // SAFETY: the caller guarantees a live grid and disjoint column row ranges.
        unsafe { stamp_changed(&*self.changed, self.call.len as usize, self.this_run) };
        // SAFETY: the same access/lifetime contract covers the unchanged ABI call.
        unsafe { run(host as *mut c_void, &self.call) };
    }
}

/// # Safety
/// Every non-null pointer covers `len` live changed-tick cells, exclusively owned
/// by this chunk. No other worker or Rust reference accesses these same cells.
unsafe fn stamp_changed(changed: &[*const UnsafeCell<BevyTick>], len: usize, tick: BevyTick) {
    for &column in changed {
        if column.is_null() {
            continue;
        }
        // SAFETY: the caller supplies the actual live row range for this column.
        for cell in unsafe { std::slice::from_raw_parts(column, len) } {
            // SAFETY: these disjoint cells are writable under the declared access.
            unsafe { *cell.get() = tick };
        }
    }
}

impl ScriptQuery<'_, '_> {
    /// Runs the C# system over every matched table, one call per chunk.
    pub fn run(&mut self, tick: &Tick) {
        let st = &mut *self.state;
        st.used = 0;
        let ncols = st.plan.columns.len();
        let scratch = &mut st.scratch;
        scratch.cols.clear();
        scratch.changed.clear();
        scratch.res.clear();
        scratch.calls.clear();
        scratch.tables.clear();
        scratch.sends.clear();
        // SAFETY: storage metadata is readable; columns are only touched within the declared access.
        let storages = unsafe { self.world.storages() };
        let mut total = 0usize;
        'tables: for table_id in st.query.matched_tables() {
            let Some(table) = storages.tables.get(table_id) else {
                continue;
            };
            let len = table.entity_count() as usize;
            if len == 0 {
                continue;
            }
            let first = scratch.cols.len();
            for (id, write, _) in &st.plan.columns {
                let Some(col) = table.get_column(*id) else {
                    // Keep the prior partial stamp side effect when a later declared
                    // component uses unsupported sparse storage instead of a column.
                    // SAFETY: workers have not started, and these are the preceding
                    // writable columns' complete row ranges for this table.
                    unsafe { stamp_changed(&scratch.changed[first..], len, self.this_run) };
                    scratch.cols.truncate(first);
                    scratch.changed.truncate(first);
                    continue 'tables;
                };
                // SAFETY: `len > 0`, so row 0 exists. Column data lives in interior-mutable
                // storage, so writing through the pointer is fine under the declared access.
                let base = unsafe { col.get_data_unchecked(TableRow::new(NonMaxU32::ZERO)) }
                    .as_ptr()
                    .cast_mut();
                let changed = if *write {
                    if let Some(summary) = col.get_summary_tick() {
                        summary.set(self.this_run);
                    }
                    // SAFETY: `len` is this column's actual row count. Row stamping
                    // is deferred to the worker that exclusively owns each chunk.
                    unsafe { col.get_changed_ticks_slice(len) }.as_ptr()
                } else {
                    std::ptr::null()
                };
                scratch.cols.push(base);
                scratch.changed.push(changed);
            }
            scratch
                .tables
                .push((table.entities().as_ptr().cast::<u64>(), len, first));
            total += len;
        }
        for (id, write) in &st.plan.resources {
            // SAFETY: the resource access was declared in `init_access`.
            let ptr = unsafe {
                if *write {
                    self.world
                        .get_resource_mut_by_id(*id)
                        .map(|m| m.into_inner().as_ptr())
                } else {
                    self.world
                        .get_resource_by_id(*id)
                        .map(|p| p.as_ptr().cast_mut())
                }
            };
            match ptr {
                Some(p) => scratch.res.push(p),
                None => {
                    // Preserve the previous stamp side effect even when a resource
                    // disappears after registration and no C# callback can run.
                    for &(_, len, first) in &scratch.tables {
                        // SAFETY: no workers have started; these are complete table
                        // ranges within this system's declared column write access.
                        unsafe {
                            stamp_changed(
                                &scratch.changed[first..first + ncols],
                                len,
                                self.this_run,
                            )
                        };
                    }
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
        // Chunk column pointers go after the per-table bases; reserve so they never move.
        let chunk_count: usize = scratch
            .tables
            .iter()
            .map(|(_, len, _)| len.div_ceil(batch.min(*len)))
            .sum();
        scratch.cols.reserve(chunk_count * ncols);
        scratch.changed.reserve(chunk_count * ncols);
        scratch.calls.reserve(chunk_count);
        if st.buffers.len() < chunk_count {
            st.buffers.resize_with(chunk_count, CmdBuf::default);
        }
        let allocator = self.world.entity_allocator();
        for buf in &mut st.buffers[..chunk_count] {
            buf.reset(allocator);
        }
        let mut chunk = 0;
        for &(entities, len, first) in &scratch.tables {
            let mut start = 0;
            while start < len {
                let n = batch.min(len - start);
                let col_at = scratch.cols.len();
                for (c, (_, _, size)) in st.plan.columns.iter().enumerate() {
                    // SAFETY: `start < len`, so the offset stays inside the column.
                    scratch
                        .cols
                        .push(unsafe { scratch.cols[first + c].add(start * size) });
                    let changed = scratch.changed[first + c];
                    scratch.changed.push(if changed.is_null() {
                        changed
                    } else {
                        // SAFETY: the base covers `len` tick cells and `start < len`.
                        unsafe { changed.add(start) }
                    });
                }
                scratch.calls.push(ChunkCall {
                    call: SystemCall {
                        system: st.plan.id,
                        len: u32::try_from(n).unwrap_or(u32::MAX),
                        // SAFETY: `start < len` rows of the table's entity slice.
                        entities: unsafe { entities.add(start) },
                        // SAFETY: `col_at + ncols <= cols.len()`, and `cols` does not reallocate (reserved above).
                        columns: unsafe { scratch.cols.as_ptr().add(col_at) },
                        resources: scratch.res.as_ptr(),
                        tick,
                        commands: std::ptr::from_mut(&mut st.buffers[chunk]).cast::<c_void>(),
                    },
                    // SAFETY: the parallel grid has the same reserved length and
                    // column indices as `cols`; neither grid moves during dispatch.
                    changed: std::ptr::slice_from_raw_parts(
                        unsafe { scratch.changed.as_ptr().add(col_at) },
                        ncols,
                    ),
                    this_run: self.this_run,
                });
                chunk += 1;
                start += n;
            }
        }
        st.used = chunk;
        let run = st.plan.api.run_system;
        let host = st.plan.host;
        if scratch.calls.len() == 1 || st.plan.serial {
            for call in &scratch.calls {
                // SAFETY: the call describes live columns within this system's access;
                // calls run one at a time, in chunk order.
                unsafe { call.run(host, run) };
            }
            return;
        }
        scratch
            .sends
            .extend(scratch.calls.iter().map(|c| SendPtr(c)));
        let sends = &scratch.sends;
        pool.scope(|s| {
            for &call in sends {
                s.spawn(async move {
                    // SAFETY: each call covers rows no other task touches, within the
                    // system's declared access; its buffer is its own; resources are
                    // read-only here (writers run serially, above).
                    unsafe { (*call.get()).run(host, run) };
                });
            }
        });
    }
}

#[cfg(test)]
#[path = "system_tests.rs"]
mod tests;
