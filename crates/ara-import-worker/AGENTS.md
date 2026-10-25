# ara-import-worker

Disposable worker executable. Registers ara-importers (glTF, FBX, textures,
audio: audio.bin + audio.json), ara-import2d (ara.atlas2d, ara.tilemap2d), then
calls ara-texture-gpu::serve_worker. Audio settings and versions share the host's importer registry.
Custom worker executables can register animation/plugin importers too.
Stdout is reserved for the framed binary-safe worker protocol; log to stderr.
The optional headless BC service negotiates before cache lookup. Normal maps keep
qualified CPU ISPC; texture-free imports skip device creation. Negotiation warms
common GPU kernels once, and driver caches belong to the explicit engine profile.
CPU process/persistence fixtures disable the service on their ProcessWorker handle.
The ignored tests/gpu.rs probe verifies real negotiation, frames and encoded bytes;
run it through ara-dev gpu with private config/crash paths after frontend changes.

Run cargo test -p ara-import-worker (process integration tests), cargo check,
cargo clippy -p ara-import-worker --all-targets. Set CARGO_BUILD_JOBS=2.
Worker crash fixtures live only under tests; never add a crash switch to production.
Do not watch CI/PRs or spawn persistent background services here.

Process jobs honor ImportInput preset and thread_budget (default Balanced / bounded per-worker threads).
HotReload orchestrates preview/HQ as separate disposable processes/cache actions.
Progressive integration tests gate HQ persistence to test boundary/failure races.
examples/progressive_bench measures real worker publication in release mode.
