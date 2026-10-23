//! Ara presentation audio, with identical offline and SDL render paths.
//!
//! Gameplay owns intent in ECS components. This crate owns only presentation
//! cursors, DSP history and device state; none of these are replicated or rolled back.
//! Authoring edits still go through ara-ops, never through this runtime API.

#![warn(missing_docs)]

pub mod capture;
pub mod codec;
pub mod device;
pub mod effects;
pub mod importer;
pub mod mixer;
pub mod schema;
pub mod spatial;
pub mod steam;
mod steam_alloc;
pub mod streaming;
mod voice;
#[cfg(windows)]
pub mod wasapi;

pub use kira::Frame;
pub use mixer::{AudioEngine, BusId, EngineConfig, Snapshot, VoiceId};
pub use schema::{AudioListener, AudioSource, register};
pub use voice::{Clip, Playback, PreparedVoice, VoiceCommand, VoiceParameters};

/// Fixed DSP quantum. SDL may request any number of frames; partial quanta are cached.
pub const QUANTUM: usize = 128;

/// Failures on the control, decode or device path. Rendering itself is infallible.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    /// A parameter violates the audio API's bounds.
    #[error("invalid audio configuration: {0}")]
    Invalid(&'static str),
    /// A bounded resource or command queue is full.
    #[error("audio capacity exhausted: {0}")]
    Capacity(&'static str),
    /// A bus or voice handle does not exist in this engine.
    #[error("unknown audio handle")]
    UnknownHandle,
    /// An operating system, SDL or Steam Audio operation failed.
    #[error("audio backend: {0}")]
    Backend(String),
    /// A media stream is invalid or unsupported.
    #[error("audio codec: {0}")]
    Codec(String),
    /// A file or pack read failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Audio operation result.
pub type Result<T> = std::result::Result<T, AudioError>;
