//! The baseline tone-map pass lives in `ara-render2d` so 2D does not depend on
//! this crate. `ToneCurve` is the CPU record from `ara-frame`.
pub use ara_frame::look::ToneCurve;
pub use ara_render2d::tonemap::ToneMap;
