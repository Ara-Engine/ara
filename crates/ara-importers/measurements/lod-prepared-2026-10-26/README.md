# Prepared LOD experiment source archive

These are the exact machine-specific scripts/header used by the 2026-10-26
single-thread experiment. They are evidence, not a build dependency or a general
prepared-simplification API. No Cargo manifest or engine build references them.
The shipping registry dependency remains meshopt 0.7, unchanged.

`setup.ps1` copies the importer into the dedicated D: target, adds only a lexical
native context guard to `mesh::lods::append`, and creates a standalone Cargo
workspace under the worktree's ignored `target/`. Its source hash check is in the
canonical report: all other 20 importer source files match production exactly.

`generate.ps1` reads the existing registry's original `simplifier.cpp`, preserves
its original function as the control, and generates a separate prepared function
in the isolated dependency copy. Original meshoptimizer headers/licenses remain
there. The generated control function is checked byte-for-byte against the
registry source. `ara_prepared_state.h` supplies snapshot ownership, context
lifecycle and coarse timing counters.

`build.ps1` uses two Cargo jobs, no incremental compilation and the dedicated D:
target, then freezes the binary. `run.ps1` runs that same binary with mode 0
(original) or mode 1 (prepared), comparing every complete output to the existing
production cache. `analyze.ps1` validates counts and calculates the ABBA result.
Production code/dependencies, importer versions, keys and sidecars never change.

For reproduction on this machine, copy these files to
`target/ara-dev/import-speed/lod-prepared/`, then run `build.ps1`. The scripts
contain the exact worktree, owned target and retained input-project paths used.
All timing commands must use `ara-dev gpu --`, as recorded in `MEASUREMENTS.md`.
Do not run through a hold; the runner checks the local `HOLD` sentinel too.

The diagnostic supports only the single-thread corpus and current positive
attribute weights in mode 97. Pointer/count invariants abort on a context mismatch.
Every other mode calls the untouched control. A production API would require
separate ownership/concurrency/unsupported-input validation and explicit approval
before shipping a native dependency fork or shim.
