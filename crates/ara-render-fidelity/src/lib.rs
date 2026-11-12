#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
//! Ara's authored look as numbers a renderer consumes: the frame's environment (sky, fog,
//! exposure, display), colour grading and tone mapping, the render settings and the quality
//! presets. No GPU work happens here; Renderlab draws.
pub mod atmosphere;
pub mod colour;
pub mod environment;
pub mod material;
pub mod presets;
pub mod settings;
pub use colour::{AutoExposure, ExposureState, Grading, ToneMapper};
pub use settings::{PostSettings, Quality};
