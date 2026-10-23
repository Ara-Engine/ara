# ara-render-fidelity

M3 lighting and post extension over ara-render's Graph and ara-gpu's device.
Scene snapshots and physical units are exactly ara-render::scene's contract.
No authoritative gameplay or authoring state lives here.

## API
- `FidelityView`: per-view resources; takes a borrowed device at frame boundaries.
- `Settings`, `FrameInput`, `UpscalerInput`: validated temporal/physical contracts.
- `ProbeVolume`, `BakedProbes`: automatic DDGI volume and offline fallback data.
- `scene::GpuScene`: stable instance IDs, masked ray-query material inputs, AS cache.
- `Reference`: high-sample, undenoised reference transport, shared shader semantics.

## Gates
Use CARGO_BUILD_JOBS=2. `cargo test -p ara-render-fidelity`,
`cargo clippy -p ara-render-fidelity --all-targets -- -D warnings`,
`cargo fmt -p ara-render-fidelity -- --check`.
Cook offline with `cargo run -p ara-shader-cook --
crates/ara-render-fidelity/shaders/pipelines.json crates/ara-render-fidelity/cooked`.
GPU sequence/golden/performance gates live in ara-pathtrace.
Use run.ps1 there for process-local Vulkan implicit-layer isolation.

## Contracts
Linear HDR, right-handed Y-up metres; camera depth is positive view distance.
Motion is current-to-previous UV, without jitter; previous depth/normal/ID reject
history. Light edits retain geometric history, use responsive radiance weights,
and update more probes with 16 rays each inside the same per-frame ray budget.
Direct radiance has separate temporal history; incident radiance uses a wavelet
filter without luminance rejection on rough surfaces. Checkerboard transport must
repair invalid history before temporal resolve. Reference targets stay RGBA32F.
Exposure is applied once after reconstruction. Capture is explicit; ordinary
frames never read back image data. GPU pipelines only consume cooked artifacts.
Every bindless input is declared to the graph, including masked textures.
Frames currently serialize because the merged GPU API serializes diagnostics.
Release resources using `destroy` before dropping the external device.
See README for unsupported backend contracts and measured gates; do not claim
unmeasured refit, NRD, FSR2, DLSS, or async overlap.
