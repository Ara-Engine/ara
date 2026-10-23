//! Versioned, little-endian cooked assets. See FORMAT.md for the byte contract.
#![deny(missing_docs)]

mod container;
mod mesh;
mod motion;
mod texture;

pub use container::{Builder, Container, FormatError, Kind, Section};
pub use mesh::{Lod, Mesh, Meshlet, QuantizedVertex, SkinExtra};
pub use motion::{Channel, Clip, Joint, Skeleton, write_skeleton};
pub use texture::{Texture, TextureFormat, write_ktx2};
