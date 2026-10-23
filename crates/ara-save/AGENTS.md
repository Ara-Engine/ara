# ara-save

Save games from schema-selected ECS state, migrations, atomic slots and agent tools.
Gameplay remains in components; resources are registered infrastructure/service data.

## APIs

- `PersistDeclaration { type_name, storage, persist }` extends reflected types with
  an explicit flag without duplicating fields/defaults/versions. `SaveSchema::new`
  binds these declarations to SchemaDb; unflagged/runtime/presentation data is omitted.
- `SaveSchema::capture/restore` uses ara-sim's selected-state bridge and the existing
  typed/dynamic codecs. Capture/apply only at an approved simulation boundary.
- `PersistentState` includes stable identity/hierarchy and the engine's tick/RNG/
  identity allocator. Input holds, pending commands, network baselines/sequences,
  presentation phase and editor source documents are excluded. Resync networking
  and rebuild presentation after restore. Compare persistent hashes at the same tick.
- `SaveGame::capture/from_state`, `encode/decode`: ARASAVE v1 binary envelope,
  compressed CBOR, strict lengths, BLAKE3 envelope checksum and persistent state hash.
  Versions come from SchemaDb; normalize_component applies its upgrade hook once
  per version step for both components and resources. Future versions fail closed.
- `SlotStore::write/load/list/restore_backup`: worker-side filesystem operations,
  synced same-directory temp files, atomic replacement, verified previous-generation
  backup, bounded read/decompression and OS-released process locks. `load` reports
  backup recovery without rewriting disk; explicit repair requires authority.
- `SlotMetadata` takes host timestamp/playtime and an optional @path#id thumbnail.
  Whole-file hash is a cloud conflict token; write(expected) rejects stale updates.
  Cloud transport/account/credentials belong to the host, not simulation.
- `diff` walks migrated persistent data with sorted RFC6901 field pointers.
- `tools::register` returns reflected save.write/load/list/diff/restore_backup
  descriptors. The runtime host dispatches functions in tools.rs, outside ara-cmd's
  authoring-only ReadCtx/EditCtx. `prepare_write/write` and `load/apply_load` split
  tick-boundary work from IO. ThumbnailHook is optional worker-side capture.

## Authority and migration

Authority is trusted host configuration, never a request parameter. Only Local and
Server can write/repair/apply authoritative saves. Replica clients send server intent.
Register all persistent component/resource codecs before capture. The policy must
use the same schemas as SimRegistry. Changes to the persistence type set require an
explicit policy migration; field evolution reuses each type's version/upgrade hook.
References into excluded state are rejected; preserve a selected component on every
referenced entity. Hierarchy anchors are captured without copying runtime fields.
Restoring removes absent persistent entities, preserving excluded child entities by
detaching before despawn. Surviving entities retain their nonpersistent components.
Saves are persistence projections, not complete replay checkpoints: use Snapshot
for prediction rollback/replay of active inputs and pending work.

## Checks

Set CARGO_BUILD_JOBS=2. Run cargo check/test -p ara-save, cargo clippy -p ara-save
--all-targets -- -D warnings, and cargo fmt -p ara-save -- --check. Also check/test/
clippy ara-sim when modifying its persistence hooks. No external build prerequisites.
