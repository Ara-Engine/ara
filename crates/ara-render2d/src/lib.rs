#![warn(missing_docs)]
//! Instanced bindless 2D rendering, sharing Ara's render graph, post and capture.
mod batch;
mod renderer;
mod scene;
pub use renderer::Renderer2d;
pub use scene::{AtlasImage, Scene2d, SpriteDraw, TilemapDraw};
