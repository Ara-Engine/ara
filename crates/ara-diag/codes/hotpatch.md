# Development Rust patching (ARA7600–ARA7603)

## ARA7600 hotpatch::SETUP: Rust hotpatch tool is unavailable
owner: ara-hotpatch

The optional development patcher could not start. Ordinary builds do not need dx.

Causes:
- dx is missing, has a different protocol/native capability version, or cannot find its native linker.
- The Windows SDK/MSVC tools are incomplete, including editbin for the unoptimized Play companion's stack reserve.
- The player was started without ara-dev's source compatibility contract.

Fixes:
- Run `ara-dev hotpatch --install-dx` to prepare the pinned optional CLI with the native Rust save fix.
- Or point ARA_DX at an already prepared 0.8.0-alpha.3 tool (native capability v2).
- Launch with ara-dev hotpatch. Read docs/dev/ara-dev.md for Windows setup.
- Repair the existing MSVC C++/Windows SDK installation if link or editbin cannot be located.

## ARA7601 hotpatch::APPLIED: Rust patch applied
owner: ara-hotpatch

A local Rust patch was published at a quiescent frame boundary. Diagnostic data
includes generation, last patch wall time and compiler-reported latency. Existing
gameplay components and GPU resources remain alive. The process is development-only.

Causes:
- A supported synchronous Rust body changed under the optional dx driver.

Fixes:
- Continue local iteration, or rebuild/restart to return to a reproducible session.

## ARA7602 hotpatch::REJECTED: Rust patch needs rebuild
owner: ara-hotpatch

The patch was rejected and the last-good generation continues. The message names
the changed source contract or library-load error.

Causes:
- A type/resource layout, function signature, static, build input or source set changed.
- An async, generic, impl Trait or unreviewed closure-bearing body changed.
- The patch library could not be loaded for this process.

Fixes:
- Rebuild and restart through ara-dev hotpatch to establish a new source baseline.
- Keep live edits within synchronous, layout-stable function bodies.

## ARA7603 hotpatch::DETERMINISM: Rust-patched simulation is not reproducible
owner: ara-hotpatch

A Rust patch changed the process's code identity. Replay/snapshot headers and state
hashes mark this explicitly. Deterministic replay, checkpoint restore and network
continuation refuse a patched process or recording; ordinary local simulation can continue.

Causes:
- A patch was applied while a local session existed, or before it was created.

Fixes:
- Rebuild and restart before recording a reproducible replay or joining replicated Play.
- Use local development sessions while iterating Rust code.
