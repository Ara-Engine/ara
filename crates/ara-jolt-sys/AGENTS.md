# ara-jolt-sys

Pinned Jolt 5.6.0 (`e77f175595e64cb44218cc9d9d56fc365ad0e36a`) and Ara's C ABI.
The unmodified upstream source and MIT license are committed under `vendor/`.
No CMake, bindgen, build-time download, or prebuilt library is involved.

## API and ownership

- `src/lib.rs` and `src/types.rs` are the ABI declaration sources. Run `python generate_abi.py` after
  changing it, then rustfmt. This regenerates the C header and layout probes.
- A world owns every shape, character, vehicle and constraint. Body handles carry
  Jolt's generation. Character/vehicle/joint slots are never reused.
- Pointer/count pairs are borrowed only for a synchronous call. No callback enters Rust.
- Native calls catch C++ exceptions and report sentinels plus a thread-local error.
  Contact callbacks use bounded, preallocated atomic-index storage without allocation.
- World mutation requires exclusive ownership. Read-only extraction/queries can share
  a world between ticks; never query concurrently with an update.
- The host-job ABI is the exception: Begin/End bracket an exclusive update, and
  Run borrows only the job queue on Bevy workers. Join every worker before another
  boundary, extraction, restore or destruction. No native callback enters Rust.
- A failed capacity/update faults the session; restore a valid checkpoint or rebuild.
  Allocation/process exhaustion and upstream fatal assertions are not recoverable errors.

## Build

- `cc` compiles Jolt and the shim with identical ABI defines. Native solver code
  stays optimized even in a debug editor. Cargo's jobserver bounds C++ parallelism.
- Set `CARGO_BUILD_JOBS=2` on the shared Windows machine.
- x86-64 currently requires AVX2/BMI/LZCNT/F16C, as in S6. Apple/Android arm64
  uses Jolt's NEON path. Additional platform toolchains have not been run here.
- Default `cross-platform-deterministic` enables `JPH_CROSS_PLATFORM_DETERMINISTIC`,
  precise floating point and no FMA. S6 measured a 7–12% cost. Disabling defaults
  selects fast local simulation; peers/replays must agree on flags.
- `bench` adds the direct C++ Update harness; `profile` also instruments copies in
  OUT_DIR, never vendor/. Phase counters are exclusive summed worker CPU time,
  not additive wall time. They are deliberately absent from ordinary game builds.
- Use the MSVC C++ tools installed with VS 18 on Windows.

## Validation

`cargo test -p ara-jolt-sys` checks C/Rust sizes, alignments and every field offset.
`cargo clippy -p ara-jolt-sys --all-targets -- -D warnings`.
`cargo fmt -p ara-jolt-sys -- --check`.
Behavioral/rollback tests live in `ara-physics-jolt` and exercise this ABI.

## Gotchas

- Never pass untrusted asset/network bytes to native RestoreState. Safe snapshots
  have private construction, plus configuration/topology/checksum validation.
- Restore requires identical body/shape/constraint/character construction order.
  Structural commands in a rollback window require reconstruction and replay.
- CharacterVirtual state excludes its shape. Ara saves crouch stance and reinstalls
  the correct capsule before restoring the character's contacts and motion state.
- Jolt contact callback and broadphase iteration order are not deterministic. Sort
  results before simulation consumes them. Contact stop also means body deactivation.
- Virtual-character candidates use a refitted AABB tree and stable CharacterID
  order, then Jolt's exact collision implementation. Sweeps honor the collector's
  fraction (which can exceed one for padding). Rebuild this cache after restore.
- Body creation/removal and column extraction reuse world-owned scratch. The temp
  allocator retains Jolt's fallback; benchmark its peak and fallback count when
  changing contact capacity. The 20k fixture needs more than 128 MiB.
- Keep upstream headers/license intact. Ara-owned shim code is under `shim/`.
