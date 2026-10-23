# ara-texture-stream
Software virtual textures: 128x128 interiors, one-texel wrapping gutters,
bounded device-local RGBA8 page pool and GPU derivative/min-mip feedback.
Public API: Source::{rgba8,from_cooked}, Catalog, Residency, Binding, Stats.
Preparation/decoding belongs on the extraction worker. Completed frame boundaries
own feedback consumption, LRU planning and uploads; never wait in record().
The caller submits begin_frame(), then its material sampling passes with Binding,
then end_frame(). Declare Binding.resources() in every sampling graph pass.
Mark submitted completion after end_frame; prepare() refuses an in-flight slot.
Sample helper is shader_source(); copy it into the consumer's source directory
before offline cooking, so its source hash covers the exact sampling contract.
Pinned mip tails guarantee a valid fallback. Children publish only when their
parent was resident in an earlier frame; protect current requests and ancestors.
Cold starts/camera cuts may initially be coarse. Record desired-vs-resident error
and same-surface temporal mip delta; never infer pop-in quality from policy alone.
Tests: cargo test -p ara-texture-stream; clippy --all-targets -- -D warnings;
cargo fmt -p ara-texture-stream -- --check. Set CARGO_BUILD_JOBS=2.
