//! The behaviour phase: every behaviour type runs serially over the tables holding its
//! data component, with exclusive world access (behaviours may `Get<T>()` any
//! component), in a deterministic order: module, then behaviour, then table, then row.

use core::ffi::c_void;

use bevy_ecs::change_detection::Tick as BevyTick;
use bevy_ecs::component::ComponentId;
use bevy_ecs::entity::Entity;
use bevy_ecs::query::{QueryBuilder, QueryState};
use bevy_ecs::storage::TableRow;
use bevy_ecs::world::World;
use bevy_ecs::world::unsafe_world_cell::UnsafeWorldCell;
use nonmax::NonMaxU32;

use crate::abi::{BehaviourCall, Frame, PHASE_FRAME, ScriptApi, Tick};
use crate::ecs::cmdbuf::CmdBuf;

/// One behaviour type, resolved to its data component.
pub(crate) struct BehaviourPlan {
    /// Behaviour id (`module << 16 | index`).
    pub id: u32,
    /// The data component.
    pub data: ComponentId,
    query: QueryState<Entity>,
}

impl BehaviourPlan {
    /// A plan whose query matches every entity with the data component.
    pub fn new(world: &mut World, id: u32, data: ComponentId) -> Self {
        let query = QueryBuilder::<Entity>::new(world).with_id(data).build();
        BehaviourPlan { id, data, query }
    }
}

/// What one behaviour phase runs with.
pub(crate) struct Phase<'a> {
    /// Stable host context handle.
    pub host: *mut c_void,
    /// The C# entry points.
    pub api: &'a ScriptApi,
    /// `PHASE_TICK` or `PHASE_FRAME`.
    pub phase: u32,
    /// The current tick.
    pub tick: &'a Tick,
    /// The current frame (frame phase only).
    pub frame: Option<&'a Frame>,
    /// The change tick written rows are marked with.
    pub this_run: BevyTick,
}

/// Runs one phase of every behaviour. The caller holds exclusive access to the world
/// (`cell` was made from `&mut World`) and has published it to C# for `Get<T>()`.
/// `chunks` is reusable scratch.
pub(crate) fn run_phase(
    cell: UnsafeWorldCell<'_>,
    plans: &mut [BehaviourPlan],
    p: &Phase<'_>,
    buf: &mut CmdBuf,
    chunks: &mut Vec<(*const u64, *mut u8, usize)>,
) {
    let Phase {
        host,
        api,
        phase,
        tick,
        frame,
        this_run,
    } = *p;
    // `(entities, data, len)` per table, gathered before calling C# so no Rust borrow of
    // the storage is held while C# reads and writes components.
    for plan in plans.iter_mut() {
        plan.query.update_archetypes_unsafe_world_cell(cell);
        chunks.clear();
        // SAFETY: exclusive world access (see above).
        let storages = unsafe { cell.storages() };
        for table_id in plan.query.matched_tables() {
            let Some(table) = storages.tables.get(table_id) else {
                continue;
            };
            let len = table.entity_count() as usize;
            let Some(col) = table.get_column(plan.data) else {
                continue;
            };
            if len == 0 {
                continue;
            }
            // SAFETY: `len > 0`, so row 0 exists.
            // Column data lives in interior-mutable storage, so writing through it is fine
            // under exclusive access.
            let data = unsafe { col.get_data_unchecked(TableRow::new(NonMaxU32::ZERO)) }
                .as_ptr()
                .cast_mut();
            if phase != PHASE_FRAME {
                // Properties are written through `data`; mark the rows changed.
                // SAFETY: exclusive access; `len` is the column's row count.
                for t in unsafe { col.get_changed_ticks_slice(len) } {
                    // SAFETY: nothing else touches these ticks during the exclusive phase.
                    unsafe { *t.get() = this_run };
                }
            }
            chunks.push((table.entities().as_ptr().cast::<u64>(), data, len));
        }
        for &(entities, data, len) in chunks.iter() {
            let call = BehaviourCall {
                behaviour: plan.id,
                phase,
                len: u32::try_from(len).unwrap_or(u32::MAX),
                reserved: 0,
                entities,
                data,
                tick,
                frame: frame.map_or(std::ptr::null(), std::ptr::from_ref),
                commands: std::ptr::from_mut(&mut *buf).cast::<c_void>(),
            };
            // SAFETY: the call describes live rows of the data column; no structural
            // change happens until the phase ends (ops are buffered in `buf`).
            unsafe { (api.run_behaviours)(host, &call) };
        }
    }
    chunks.clear();
}
