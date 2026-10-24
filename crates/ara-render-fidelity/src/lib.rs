#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
//! Graph-backed hardware ray-query lighting and reference rendering.
pub mod atmosphere;
mod capture;
pub mod colour;
mod controls;
pub mod environment;
mod features;
mod frame;
pub mod material;
mod pipeline_variants;
pub mod presets;
pub mod probes;
mod render;
mod replacement;
mod resampling;
mod shadow_source;
mod surface_bake;
pub use shadow_source::SunShadowSource;
pub mod scene;
pub mod settings;
mod targets;
mod view;
pub use colour::{AutoExposure, ExposureState, Grading, ToneMapper};
pub use features::{FeatureInputs, ImageInput};
pub use settings::{
    FrameInput, Mode, PostSettings, Quality, Settings, Tier, Upscaler, UpscalerInput,
};
pub use view::FidelityView;
