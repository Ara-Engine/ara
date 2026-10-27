# ara-hotpatch

Development-only Subsecond boundary and body-only compatibility checks.
Default features expose tooling (`Contract`) only. Player/editor opt into `runtime`
through `hotpatch`. Release hosts build without it.

`Hotpatch` is the sole process owner. Its websocket worker validates PID/build and
source compatibility, then queues a table. Call `frame_boundary` outside all hot
scopes. `Lease` protects parallel system/pass recording; publication defers while
any hot call executes. Existing GPU retirement stays in ara-gpu. Subsecond retains
code libraries for the process lifetime.

`generation() != 0` taints process simulation sessions. Keep replay/net guards and
the export marker aligned. Unsupported bodies request a rebuild; broaden the
source contract only with evidence that persisted state remains compatible.
Measured limitations: `spikes/s11-rust-hotpatch/RESULT.md`.

Check default tooling and `--features runtime` with focused clippy/tests. Real patch
tests need optional prepared dx (`ara-dev hotpatch --install-dx`) and scratch profiles.
The driver honors CARGO_TARGET_DIR for all fat/thin/tool caches. The reviewed
synchronous render/panel entries may change frame-local closures; retained closure
environments and every layout/signature still require rebuilding.
