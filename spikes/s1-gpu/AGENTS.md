# S1 Vulkan spike

Own standalone workspace; no engine public API. `src/gpu.rs` owns Vulkan lifetime,
`src/gpu/resources.rs` owns allocation and resource helpers,
`accel.rs` builds BLAS/TLAS, `render.rs` records the cross-queue experiment,
`passes.rs` records raster/readback passes, `pipelines.rs` owns binding layout and
cache measurements, `window.rs` owns SDL/WSI.

Run `./setup.ps1`, `./compile-shaders.ps1`, then `./run.ps1` from this directory.
Every Cargo invocation must have `CARGO_BUILD_JOBS=4`. Scoped gates:
`cargo check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `cargo fmt --all -- --check`.

Shaders compile offline to checked-in `cooked/` artifacts for the Mac handoff.
Never replace Slang source with GLSL/HLSL, or commit tools/target/cache files.
SDK tools and SDL are local prerequisites; no global installs.
Runtime intentionally waits after each frame for deterministic capture/timing and
resource reuse. Separate compute-family dispatch is proven; overlap is not claimed.
