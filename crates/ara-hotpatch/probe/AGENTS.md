# ara-hotpatch-probe

Windowless native instanced renderer proving real Rust pass edits. `--fidelity`
instead keeps an RT FidelityView alive and exercises its real recording graph.
This bin needs
the `hotpatch` feature and optional dx through `ara-dev hotpatch -- render-probe`.
It acquires the GPU queue after the fat build; its child alone owns publication.

All captures, status, device caches and stop controls use the explicit scratch
output directory. Create `stop` there to exit and release GPU admission. No game
profile or tracked file is written by this executable.

Check with `cargo clippy -p ara-hotpatch-probe --all-targets --features hotpatch -- -D warnings`.
Manual edit measurements must restore their source edits and inspect captures.
