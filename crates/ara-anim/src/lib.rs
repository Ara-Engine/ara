//! Deterministic, allocation-free skeletal animation over immutable cooked assets.
#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[cfg(test)]
mod codec_tests;
mod compile;
mod compression;
mod controller;
pub mod data;
pub mod document;
mod evaluate;
pub mod graph;
pub mod ik;
mod measure;
mod packed_block;
mod packed_wire;
pub mod pose;
mod prediction;
mod quantization;
mod sampling;
mod sampling_plan;
mod skeleton;

pub use compression::{CompressionReport, CompressionSettings, CookedClip};
pub use controller::{Animator, Controller, Event, Snapshot};
pub use glam;
pub use pose::{Pose, RootMotion, Transform};
pub use sampling::Sampler;
pub use skeleton::Skeleton;

/// A validation or runtime contract violation, before the output is modified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}

pub(crate) fn ensure(ok: bool, message: &str) -> Result<(), Error> {
    if ok {
        Ok(())
    } else {
        Err(Error(message.into()))
    }
}

// Stable content signatures bind caches/snapshots to the exact cooked program.
pub(crate) fn signature(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}
