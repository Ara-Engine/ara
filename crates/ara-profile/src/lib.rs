//! One bounded instrumentation stream for agents, the editor and Tracy.
//! Register threads once; hot-path spans never allocate or lock. Collect only at
//! host frame boundaries. Worker tokens preserve the originating frame identity.

mod allocator;
mod capture;
mod compare;
mod model;
mod recorder;
mod ring;
mod summary;
mod tracing_layer;

pub use allocator::{AllocationSample, CountingAllocator, heap_bytes};
pub use capture::{read_capture, read_summary, write_capture, write_summary};
pub use compare::{Comparison, Delta, compare};
pub use model::*;
pub use recorder::{Config, FrameToken, Profiler, ThreadRegistration, scope};
pub use ring::SpanGuard;
pub use summary::{Distribution, Hitch, Metric, Summary, SummaryBudget};
pub use tracing_layer::ProfileLayer;
