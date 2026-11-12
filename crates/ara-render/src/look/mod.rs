//! The raster look. CPU records live in `ara-frame` and are re-exported here.
//! GPU passes apply them after the G-buffer.
pub use ara_frame::look::atmosphere;
pub mod captures;
pub(crate) mod cascades;
pub(crate) mod far;
pub(crate) mod frame;
pub mod gi;
pub(crate) mod gpu;
pub(crate) mod horizon;
pub(crate) mod lights;
pub(crate) mod local;
pub(crate) mod ltc;
pub(crate) mod moments;
pub(crate) mod passes;
pub use ara_frame::look::placement;
pub(crate) mod reflect;
pub(crate) mod refract;
pub(crate) mod screen_gi;
pub(crate) mod trace;
pub(crate) mod translucent;
pub(crate) mod transport;
pub mod volumetric;
pub(crate) mod vsm;

pub use ara_frame::look::{
    AmbientOcclusion, AutoExposure, FarShadows, Fog, Grading, Lens, LocalShadows, Look, OverPreset,
    Quality, RayQueries, ReflectionCaptures, Reflections, ScreenGi, Shadows, Sky, SsrTuning,
    ToneCurve, Upscaler, VirtualShadows, AoTuning, DepthOfField, DepthOfFieldQuality,
    FogVolume, FogVolumeShape, MotionBlurQuality, VolumetricFog, VolumetricFogQuality,
    EMITTER_RADIANCE, EXPOSURE_CURVE_KEYS, MAX_FOG_VOLUMES,
};
