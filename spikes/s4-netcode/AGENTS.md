# S4 netcode spike

This standalone crate compares Lightyear/Replicon and Renet behind Ara schemas.
It is throwaway evidence for M2, not a shipping engine crate.

- `gameplay.rs` owns ECS state and deterministic simulation; no netcode imports.
- `schema.rs` declares policy; `adapter.rs` validates policies and projects state.
- `ly.rs` and `ly/` contain native registration, input, replication and prediction.
- `session.rs` contains shared process contracts and the Renet adapter.
- `local.rs` must construct only a plain ECS world with no network serialization.
- `harness.rs` runs identical scripts through real conditioned UDP child processes.

Set `CARGO_BUILD_JOBS=4` for every Cargo invocation on this shared machine.
Use the repository-pinned Rust toolchain. No global installations.

From this directory, run:

```powershell
cargo fmt --all -- --check
cargo check --locked --all-features --all-targets
cargo clippy --locked --all-features --all-targets -- -D warnings
cargo test --locked --all-features
cargo test --locked --no-default-features
```

`rerun.ps1` performs the complete comparison, including sequential fresh-target
builds, per-stack socket scenarios and Local benchmarks. It also runs
`summarize.py`, which asserts the six gates from committed JSON reports.
See RESULT.md for exact individual commands, measured costs and limitations.

Keep generated targets and per-run files ignored. The harness must reap its child
processes on success and failure. Inventory privacy must be checked on received
components, not only after projection. Final digests must never repair replicas.
Do not edit parent-owned decisions or milestones; propose changes in RESULT.md.
