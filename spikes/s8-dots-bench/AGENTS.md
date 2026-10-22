# S8 DOTS benchmark

Standalone throwaway crate and Unity/.NET projects; not part of the engine workspace.
Own only this directory. The parent agent owns engine decisions and milestones.

`src/model.rs` defines the shared mathematical contract. Mirror changes in both C# runners.
`src/ecs.rs` runs Bevy queries and deferred structural changes.
`src/lib.rs` owns buffers borrowed by the managed process over a C ABI.
Never retain ABI pointers after `s8_free`; no resize while C# holds a view.
The ABI is trusted, in-process, and single-owner. Parallel writes use disjoint ranges.

Checks: `cargo check --manifest-path spikes/s8-dots-bench/Cargo.toml`,
`cargo clippy --manifest-path spikes/s8-dots-bench/Cargo.toml --all-targets -- -D warnings`,
`cargo test --manifest-path spikes/s8-dots-bench/Cargo.toml`, and scoped `cargo fmt`.
Build/run only when explicitly authorized (this spike's task authorizes benchmark builds).
Set `CARGO_BUILD_JOBS=4`. Do not overlap timed runners or compile during measurements.
Use `run.ps1` for builds/runs and `analyze.py` for equivalence checks and results.
