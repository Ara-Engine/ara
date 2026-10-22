//! Bevy glue for C# systems: chunk construction, system sets, and schedule building from the
//! system list the C# module registered.

use crate::schema::{self, Chunk, MAX_COLUMNS, SHAPES, ScriptApi, Tick};
use bevy_ecs::prelude::*;
use bevy_ecs::schedule::{MultiThreadedExecutor, SystemSet};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// The current tick, read by every C# system wrapper.
#[derive(Resource, Clone, Copy)]
pub struct SimTick(pub Tick);

/// System set holding exactly one C# system; used for deterministic ordering edges.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CsSet(pub u32);

/// Counts chunk calls per OS thread name; used to prove C# runs on task-pool threads.
pub type ThreadTally = Arc<Mutex<BTreeMap<String, u64>>>;

/// A C# system as the host sees it: an id into the C# system table plus the script API.
#[derive(Clone)]
pub struct CsSystem {
    pub id: u32,
    pub api: ScriptApi,
    pub tally: Option<ThreadTally>,
}

impl CsSystem {
    /// Runs this system over one chunk. Faults are contained and reported on the C# side.
    pub fn run(&self, chunk: &Chunk, tick: &Tick) {
        if let Some(t) = &self.tally {
            let name = std::thread::current()
                .name()
                .unwrap_or("<unnamed>")
                .to_owned();
            *t.lock().expect("tally lock").entry(name).or_default() += 1;
        }
        // SAFETY: `chunk` points at table columns borrowed by the calling query for the duration of
        // this call, with `len` valid rows per column; C# only touches rows < len.
        unsafe { (self.api.run_system)(self.id, chunk, tick) };
    }
}

/// Starts a chunk for one contiguous run of entities.
pub fn chunk_for(entities: &[Entity]) -> Chunk {
    Chunk {
        len: u32::try_from(entities.len()).expect("chunk rows fit u32"),
        column_count: 0,
        // `Entity` is `repr(C, align(8))` and documented as equivalent to a u64 (`to_bits`).
        entities: entities.as_ptr().cast::<u64>(),
        col0: std::ptr::null_mut(),
        col1: std::ptr::null_mut(),
        col2: std::ptr::null_mut(),
        col3: std::ptr::null_mut(),
    }
}

/// Appends the next column pointer in shape order.
pub fn push_column(chunk: &mut Chunk, ptr: *mut u8) {
    let slot = match chunk.column_count {
        0 => &mut chunk.col0,
        1 => &mut chunk.col1,
        2 => &mut chunk.col2,
        3 => &mut chunk.col3,
        _ => panic!("shape has more than {MAX_COLUMNS} columns"),
    };
    *slot = ptr;
    chunk.column_count += 1;
}

/// One registered C# system: name and shape, as reported by `system_info`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemEntry {
    pub id: u32,
    pub name: String,
    pub shape: u32,
}

/// Reads the C# system table.
pub fn read_systems(api: &ScriptApi) -> Vec<SystemEntry> {
    // SAFETY: plain calls into the C# runtime; buffers are valid for the given capacity.
    let count = unsafe { (api.system_count)() };
    (0..count)
        .map(|id| {
            let mut info = schema::SystemInfo {
                shape: 0,
                name_len: 0,
            };
            let mut buf = [0u8; 128];
            // SAFETY: `info` and `buf` are valid for writes; the capacity matches `buf`.
            let rc =
                unsafe { (api.system_info)(id, &mut info, buf.as_mut_ptr(), buf.len() as u32) };
            assert_eq!(rc, 0, "system_info({id}) failed");
            let name = String::from_utf8_lossy(&buf[..info.name_len as usize]).into_owned();
            SystemEntry {
                id,
                name,
                shape: info.shape,
            }
        })
        .collect()
}

/// True if two shapes' accesses conflict (one writes a component the other touches).
fn conflicts(a: u32, b: u32) -> bool {
    let (a, b) = (&SHAPES[a as usize], &SHAPES[b as usize]);
    a.columns.iter().any(|ca| {
        b.columns
            .iter()
            .any(|cb| ca.component == cb.component && (ca.write || cb.write))
    })
}

/// Builds a multi-threaded schedule for the registered systems. Conflicting systems are ordered
/// by registration order (deterministic); non-conflicting ones may run in parallel.
pub fn build_schedule(
    api: ScriptApi,
    systems: &[SystemEntry],
    tally: Option<ThreadTally>,
) -> Schedule {
    let mut schedule = Schedule::default();
    schedule.set_executor(MultiThreadedExecutor::new());
    for (i, s) in systems.iter().enumerate() {
        let after: Vec<CsSet> = systems[..i]
            .iter()
            .filter(|p| conflicts(p.shape, s.shape))
            .map(|p| CsSet(p.id))
            .collect();
        let sys = CsSystem {
            id: s.id,
            api,
            tally: tally.clone(),
        };
        assert!(
            schema::add_shape_system(&mut schedule, s.shape, sys, &after),
            "unknown shape {}",
            s.shape
        );
    }
    schedule
}
