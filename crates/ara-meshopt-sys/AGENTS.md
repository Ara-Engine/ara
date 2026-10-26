# ara-meshopt-sys

Owned prepared contexts for sequential attribute-aware importer LOD targets.
The native source is meshoptimizer 1.3 as bundled by meshopt 0.7.0, exactly the
workspace's existing dependency. Vendor files are unmodified; hashes/provenance
are in vendor/REVISION.md and the MIT license in vendor/LICENSE.

## API

Prepared::new owns/rescales all inputs; no borrowed pointer survives construction.
Prepared::simplify(&mut self) resets pristine state before each new LOD0 target.
Drop frees the context. Contexts stay on one worker, with no global/TLS context.
Independent contexts may run concurrently. Only attribute mode 97 is exposed:
LockBorder | Permissive | RegularizeLight, positive finite weights, lock bits 0–1.
Other entry points remain in the unmodified registry meshopt dependency.
Native exceptions are caught at the ABI. Failed calls never expose partial output.

## Build and validation

cc compiles shim/context.cpp; no generator, bindgen, CMake, Python or download.
Keep native optimization/FP flags compatible with meshopt's existing build.
Set CARGO_BUILD_JOBS=2 on this shared PC.
Run cargo fmt, clippy --all-targets -- -D warnings and test -p ara-meshopt-sys.
Permanent fixtures compare indices and error bits against registry meshopt 0.7.
Source-hash tests prevent silent vendor drift. Update both sides deliberately.
The opt-in real corpus check is ara-importers' geometry_replay example with
--features prepared-lods and a completed production-cache project (101 Outpost
models). See ara-importers/MEASUREMENTS.md. Timing runs use ara-dev gpu --.

Do not enable prepared-lods by default before its fresh-project NVMe ABBA wins.
No importer/key/version changes: every output must stay byte-identical.
