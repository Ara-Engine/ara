//! Opt-in, deterministic 25 cm patch transport. Cached links define a positive
//! linear system; events update its forcing/operator and reverse dependencies.
//! Steady frames only read the solution. No screen colour or lighting history is
//! an input. G3 measures the complete path, including CPU discovery and publication.

// The native adapter and standalone gate harness exercise the same implementation.
#![allow(dead_code)]

mod channels;
mod gpu;
mod graph;
mod native;
mod readout;
mod scheduler;
#[cfg(test)]
mod tests;
mod visibility;

pub(crate) use gpu::ReadoutGpu;
pub(crate) use graph::{DirectionScheme, TraceGraph};
pub use native::{PatchTransport, TransportMedium, TransportOptions, TransportReport};
pub(crate) use scheduler::{Budget, EventSolver};

use glam::Vec3;

fn magnitude(value: Vec3) -> f32 {
    value.abs().max_element()
}

fn valid_radiance(value: Vec3) -> bool {
    value.is_finite() && value.cmpge(Vec3::ZERO).all()
}
