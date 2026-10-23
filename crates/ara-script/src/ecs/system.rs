//! C# systems as bevy systems. [`ScriptQuery`] is a custom `SystemParam` built from a
//! system's declared columns and resources at runtime: it registers that access with
//! the scheduler (so non-conflicting systems run in parallel), walks the matched tables
//! for column base pointers, splits them into chunks and runs one C# call per chunk on
//! the compute task pool. Structural ops land in per-chunk buffers, applied at bevy
//! sync points in chunk order.

use core::ffi::c_void;

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
    /// The C# entry points.
    pub api: ScriptApi,
}

/// Per-run pointers: only valid during one run of the system that owns them.
#[derive(Default)]
struct Scratch {
    /// Column base pointers, `columns.len()` per chunk.
    cols: Vec<*mut u8>,
    /// Resource pointers.
    res: Vec<*mut u8>,
    /// One call per chunk.
    calls: Vec<SystemCall>,
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
struct SendPtr(*const SystemCall);

// SAFETY: each pointer is handed to exactly one task, points into the system's scratch
// (alive for the whole scope) and the call it describes covers rows no other task sees.
unsafe impl Send for SendPtr {}

impl SendPtr {
    fn get(self) -> *const SystemCall {
        self.0
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
                    scratch.cols.truncate(first);
                    continue 'tables;
                };
                // SAFETY: `len > 0`, so row 0 exists. Column data lives in interior-mutable
                // storage, so writing through the pointer is fine under the declared access.
                let base = unsafe { col.get_data_unchecked(TableRow::new(NonMaxU32::ZERO)) }
                    .as_ptr()
                    .cast_mut();
                if *write {
                    // SAFETY: the system has write access to this column and `len` is its row count.
                    for t in unsafe { col.get_changed_ticks_slice(len) } {
                        // SAFETY: no one else reads or writes these ticks while we hold write access.
                        unsafe { *t.get() = self.this_run };
                    }
                    if let Some(summary) = col.get_summary_tick() {
                        summary.set(self.this_run);
                    }
                }
                scratch.cols.push(base);
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
                None => return, // resource missing: the host reports it at registration
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
                }
                scratch.calls.push(SystemCall {
                    system: st.plan.id,
                    len: u32::try_from(n).unwrap_or(u32::MAX),
                    // SAFETY: `start < len` rows of the table's entity slice.
                    entities: unsafe { entities.add(start) },
                    // SAFETY: `col_at + ncols <= cols.len()`, and `cols` does not reallocate (reserved above).
                    columns: unsafe { scratch.cols.as_ptr().add(col_at) },
                    resources: scratch.res.as_ptr(),
                    tick,
                    commands: std::ptr::from_mut(&mut st.buffers[chunk]).cast::<c_void>(),
                });
                chunk += 1;
                start += n;
            }
        }
        st.used = chunk;
        let run = st.plan.api.run_system;
        if scratch.calls.len() == 1 {
            // SAFETY: the call describes live columns within this system's access.
            unsafe { run(&scratch.calls[0]) };
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
                    // system's declared access; its buffer is its own.
                    unsafe { run(call.get()) };
                });
            }
        });
    }
}
