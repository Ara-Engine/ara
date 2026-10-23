use serde::{Deserialize, Serialize};

/// Stable producer-thread identity within a capture.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadInfo {
    /// Index used by CPU spans.
    pub id: u32,
    /// Human-readable worker or subsystem name.
    pub name: String,
}

/// Completed CPU scope. Times use the capture's monotonic origin, in nanoseconds.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CpuSpan {
    /// Index into `Profile::labels`.
    pub label: u32,
    /// Producer thread ID.
    pub thread: u32,
    /// Thread-local scope identity (unique across the capture).
    pub id: u64,
    /// Enclosing scope identity on the same thread, or zero.
    pub parent: u64,
    /// Start relative to capture origin.
    pub start_ns: u64,
    /// Inclusive CPU duration.
    pub duration_ns: u64,
    /// Exclusive CPU duration, subtracting nested scopes.
    pub self_ns: u64,
    /// Bytes requested in this scope, including children.
    pub allocated_bytes: u64,
    /// Bytes requested in this scope excluding children.
    pub self_allocated_bytes: u64,
}

/// A resolved pass duration, associated with its submitting CPU frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuPass {
    /// Render-graph label index in `Profile::labels`.
    pub label: u32,
    /// Actual execution queue label index; independent queues may overlap.
    pub queue: u32,
    /// Device timestamp delta, never a CPU submission duration.
    pub duration_ns: u64,
}

/// Out-of-process/worker activity. Durations are measured in that worker's clock;
/// only the originating frame ID is shared across clock domains.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct JobActivity {
    /// Job category label (`asset.import`, `nav.bake`, `compile.server`).
    pub label: u32,
    /// Stable worker/process identity supplied by the job manager.
    pub worker: u64,
    /// Worker-measured elapsed duration in nanoseconds.
    pub duration_ns: u64,
    /// True for a completed job; false for an in-progress activity sample.
    pub complete: bool,
}

/// Engine counters in the order documented by each field.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
pub struct Counters {
    /// Live entities.
    pub entities: u64,
    /// Submitted draws.
    pub draws: u64,
    /// Submitted triangles.
    pub triangles: u64,
    /// Live particles.
    pub particles: u64,
    /// Active audio voices.
    pub voices: u64,
    /// Active physics bodies.
    pub physics_bodies: u64,
    /// Received network bytes this frame.
    pub net_received: u64,
    /// Sent network bytes this frame.
    pub net_sent: u64,
}

/// Memory snapshots; GPU categories are populated by the owning resource manager.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
pub struct Memory {
    /// Process live requested bytes, when the counting allocator is installed.
    pub cpu_heap: u64,
    /// Bytes allocated during the CPU frame across the process.
    pub allocated_bytes: u64,
    /// Number of allocation/reallocation requests during this frame.
    pub allocations: u64,
    /// GPU textures.
    pub gpu_textures: u64,
    /// GPU buffers.
    pub gpu_buffers: u64,
    /// GPU acceleration structures.
    pub gpu_acceleration: u64,
    /// Other GPU allocations.
    pub gpu_other: u64,
}

/// OS measurement surrounding a scenario; absent samples are explicit, never zero load.
#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
pub struct BackgroundLoad {
    /// All-process CPU utilization as a fraction of machine capacity.
    pub total_cpu: Option<f64>,
    /// This process's utilization as a fraction of machine capacity.
    pub process_cpu: Option<f64>,
    /// Other processes' utilization (saturating total minus process).
    pub background_cpu: Option<f64>,
}

/// A CPU frame, with delayed GPU/worker data attached by its immutable ID.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Frame {
    /// Unique, monotonically increasing ID; zero is reserved for no frame.
    pub id: u64,
    /// Start relative to the capture origin.
    pub start_ns: u64,
    /// Wall duration; overlapping worker CPU durations are not summed into this.
    pub duration_ns: u64,
    /// CPU timeline, with parent links also usable as a flame graph.
    pub spans: Vec<CpuSpan>,
    /// Resolved GPU pass list; no fabricated absolute GPU/CPU clock alignment.
    pub gpu: Vec<GpuPass>,
    /// Worker/process job samples, separate from the CPU flame graph.
    pub jobs: Vec<JobActivity>,
    /// Subsystem-supplied counters.
    pub counters: Counters,
    /// CPU and GPU memory snapshot.
    pub memory: Memory,
    /// Background activity when measured by a benchmark scenario.
    pub load: BackgroundLoad,
    /// Ring, nesting or per-frame capacity losses; nonzero means incomplete data.
    pub dropped: u64,
}

/// Serializable capture and editor data API. Nanoseconds and bytes are integral.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Profile {
    /// Format version. Readers reject unsupported versions.
    pub version: u32,
    /// Frame budget used for hitch detection.
    pub budget_ns: u64,
    /// Interned names; CPU records carry only indices.
    pub labels: Vec<String>,
    /// Producer identities and names.
    pub threads: Vec<ThreadInfo>,
    /// Rolling frame history in chronological order.
    pub frames: std::collections::VecDeque<Frame>,
    /// Results arriving after their frame was evicted.
    pub orphaned_results: u64,
}

impl Frame {
    /// CPU timeline; parent IDs form flame-graph edges within each thread.
    pub fn timeline(&self) -> &[CpuSpan] {
        &self.spans
    }

    /// Direct children of a scope on one thread; zero selects flame-graph roots.
    pub fn flame_children(&self, thread: u32, parent: u64) -> impl Iterator<Item = &CpuSpan> {
        self.spans
            .iter()
            .filter(move |s| s.thread == thread && s.parent == parent)
    }
}
