#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used))]
//! Instanced bindless 2D rendering, sharing Ara's render graph, post and capture.
mod batch;
pub mod graph;
mod renderer;
mod scene;
pub mod tonemap;
pub use renderer::Renderer2d;
pub use scene::{AtlasImage, Scene2d, SpriteDraw, TilemapDraw};
