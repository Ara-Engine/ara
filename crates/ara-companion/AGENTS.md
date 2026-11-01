# ara-companion

`ara` renders through the sibling `ara-player` and imports through the sibling
`ara-import-worker`. Building only `-p ara` leaves older companions that load this tree's
newly cooked shaders against old Rust-side data (an old player once drew star patterns
over every wall; a missing worker renders imported models as placeholders). This crate
makes `ara` refuse them with `ARA7308`.

## Public API

- `enforce()`: called once by the `ara` binary. Until then every check passes, so tests,
  the editor and the player never refuse anything (`ara-dev` already builds the editor's
  companions together, in one Cargo invocation).
- `verify(exe) -> Result<(), Refusal>`: called where a companion is spawned
  (`ara-asset`'s `ProcessWorker`, `ara-play-host`'s player pool). `Refusal`'s `Display`
  is the whole diagnostic: the stale binary, the source file that changed and the exact
  `cargo build -p ...` that fixes it.

## How it decides

A companion is checked only when it is `ara-player` or `ara-import-worker`, sits beside
the running `ara` (so an `ARA_PLAYER`/`ARA_IMPORT_WORKER` override elsewhere is explicit
intent), and the source tree it was built from exists at the path baked in at compile
time. Packaged builds have no tree, so they are never refused.

The companion is stale when any `src/**`, `Cargo.toml` or `build.rs` in its path-dependency
closure (read from the manifests; dev-dependencies and `optional` ones are skipped) is
newer than the executable. That is Cargo's own mtime rule, so a rebuild always clears it.
A missing companion is refused with the build command too.

Verdicts are cached per executable and mtime: workers spawn once per import job.

`ARA_ALLOW_STALE_COMPANIONS=1` skips the check (a stray `.rs` file Cargo never compiles
can keep a companion "stale" forever).

## Test

`cargo test -p ara-companion`.
