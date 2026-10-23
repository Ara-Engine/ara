# ara-anim-capture

Offline verification binary, not an animation/runtime dependency. Converts real solved
poses into narrow triangular bone prisms and uses ara-render's deterministic Vulkan
capture contract, provenance, entity IDs and labels. No substitute rasterizer.

Run CARGO_BUILD_JOBS=2 cargo run -p ara-anim-capture -- <output-directory>.
The default writes into ara-anim/captures/v2. Cooked renderer shaders are committed;
Vulkan validation layers/SDK must be configured for the current Windows machine.
The output includes PNG strips, metadata, legends and exact render-ID channels.

Gate with cargo test -p ara-anim-capture, cargo clippy -p ara-anim-capture --all-targets
-- -D warnings, and cargo fmt -p ara-anim-capture -- --check.
Keep capture geometry/debug visualization outside production animation code.
