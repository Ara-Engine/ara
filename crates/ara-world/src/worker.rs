use crate::{CellBlob, Result, invalid, source::CellSource};
use ara_sim::{PreparedStreamEntity, SimRegistry};
use std::{
    collections::VecDeque,
    sync::{Arc, mpsc},
    thread,
};

pub(crate) struct LoadedCell {
    pub blob: CellBlob,
    pub prepared: VecDeque<PreparedStreamEntity>,
    pub identities: Vec<String>,
    pub charge: usize,
}
pub(crate) struct Completion {
    pub cell: String,
    pub result: Result<Option<LoadedCell>>,
}
enum Work {
    Load(String),
    Retire(Box<LoadedCell>),
}

/// Fixed worker threads with one in-flight job each and bounded result channels.
/// Windows workers run below normal priority. Dropping never joins from a frame.
pub struct WorkerPool {
    pub(crate) cell_limit: usize,
    senders: Vec<mpsc::SyncSender<Work>>,
    busy: Vec<bool>,
    results: mpsc::Receiver<(usize, Completion)>,
    retired: VecDeque<Box<LoadedCell>>,
}
impl WorkerPool {
    /// Creates 1..=16 workers, with a hard per-cell retained-memory admission limit.
    pub fn new(
        source: Arc<dyn CellSource>,
        registry: SimRegistry,
        workers: usize,
        cell_limit: usize,
    ) -> Result<Self> {
        if !(1..=16).contains(&workers) || cell_limit == 0 {
            return Err(invalid("invalid worker count or cell memory limit"));
        }
        let registry = Arc::new(registry);
        let (send, results) = mpsc::sync_channel(workers);
        let mut senders = Vec::new();
        for slot in 0..workers {
            let (requests, jobs) = mpsc::sync_channel(1);
            senders.push(requests);
            let (send, source, registry) = (send.clone(), source.clone(), registry.clone());
            thread::Builder::new()
                .name(format!("ara-world-{slot}"))
                .spawn(move || {
                    lower_priority();
                    while let Ok(work) = jobs.recv() {
                        let completion = match work {
                            Work::Load(cell) => {
                                let result =
                                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                        load(&*source, &registry, &cell, cell_limit)
                                    }))
                                    .unwrap_or_else(|_| Err(invalid("cell worker panicked")));
                                Completion {
                                    cell,
                                    result: result.map(Some),
                                }
                            }
                            Work::Retire(cell) => {
                                let id = cell.blob.cell.to_owned();
                                drop(cell);
                                Completion {
                                    cell: id,
                                    result: Ok(None),
                                }
                            }
                        };
                        if send.send((slot, completion)).is_err() {
                            break;
                        }
                    }
                })?;
        }
        Ok(Self {
            cell_limit,
            senders,
            busy: vec![false; workers],
            results,
            retired: VecDeque::new(),
        })
    }

    pub(crate) fn request(&mut self, cell: &str) -> bool {
        let Some(slot) = self.busy.iter().position(|busy| !busy) else {
            return false;
        };
        if self.senders[slot].try_send(Work::Load(cell.into())).is_ok() {
            self.busy[slot] = true;
            true
        } else {
            false
        }
    }
    pub(crate) fn poll(&mut self) -> Option<Completion> {
        if let Ok((slot, completion)) = self.results.try_recv() {
            self.busy[slot] = false;
            return Some(completion);
        }
        None
    }
    pub(crate) fn retire(&mut self, cell: LoadedCell) {
        self.retired.push_back(Box::new(cell));
    }
    pub(crate) fn collect(&mut self) {
        for slot in 0..self.busy.len() {
            if self.busy[slot] {
                continue;
            }
            let Some(cell) = self.retired.pop_front() else {
                break;
            };
            match self.senders[slot].try_send(Work::Retire(cell)) {
                Ok(()) => self.busy[slot] = true,
                Err(
                    mpsc::TrySendError::Full(Work::Retire(cell))
                    | mpsc::TrySendError::Disconnected(Work::Retire(cell)),
                ) => self.retired.push_front(cell),
                Err(_) => unreachable!("only retirement work was sent"),
            }
        }
    }
}

fn load(
    source: &dyn CellSource,
    registry: &SimRegistry,
    cell: &str,
    limit: usize,
) -> Result<LoadedCell> {
    let mut blob = source.load(cell, limit / 8)?;
    if blob.cell != cell || blob.schema != crate::cook::schema_hash(registry.schema()) {
        return Err(invalid("cell ID/schema mismatch"));
    }
    blob.validate()?; // Validate custom sources too, not just file-backed sources.
    let mut prepared = VecDeque::with_capacity(blob.entities.len());
    let mut identities = Vec::with_capacity(blob.entities.len());
    let mut charge = std::mem::size_of::<LoadedCell>();
    for row in std::mem::take(&mut blob.entities) {
        // Includes retained identity/mapping nodes, prepared values and ECS overhead.
        let row_bytes = row
            .record
            .iter()
            .map(|(key, value)| 96 + key.capacity() + value_bytes(value))
            .sum::<usize>();
        if row_bytes > 16 * 1024 {
            return Err(invalid(
                "streamed row exceeds 16 KiB; move large data into cooked assets",
            ));
        }
        charge = charge
            .checked_add(1024 + row.identity.len() * 4 + row_bytes * 3)
            .ok_or_else(|| invalid("cell memory accounting overflow"))?;
        if charge > limit {
            return Err(invalid(
                "cell exceeds reserved memory; split it or raise cell reservation",
            ));
        }
        identities.push(row.identity.to_owned());
        prepared.push_back(registry.prepare_stream_entity(row.identity, row.record)?);
    }
    Ok(LoadedCell {
        blob,
        prepared,
        identities,
        charge,
    })
}

fn value_bytes(value: &serde_json::Value) -> usize {
    use serde_json::Value;
    std::mem::size_of::<Value>()
        + match value {
            Value::String(s) => s.capacity(),
            Value::Array(a) => {
                a.capacity() * std::mem::size_of::<Value>()
                    + a.iter().map(value_bytes).sum::<usize>()
            }
            Value::Object(o) => o
                .iter()
                .map(|(k, v)| 96 + k.capacity() + value_bytes(v))
                .sum(),
            _ => 0,
        }
}

#[cfg(windows)]
fn lower_priority() {
    unsafe extern "system" {
        fn GetCurrentThread() -> *mut std::ffi::c_void;
        fn SetThreadPriority(thread: *mut std::ffi::c_void, priority: i32) -> i32;
    }
    // SAFETY: pseudo-handle is valid for the calling worker; -1 is BELOW_NORMAL.
    unsafe {
        SetThreadPriority(GetCurrentThread(), -1);
    }
}
#[cfg(not(windows))]
fn lower_priority() {}
