# ara-asset-host

Background preparation for typed asset lifecycle and OS-file import commands.
It maps consumer sources to their local URI inputs, plans conflicts and prepares
immutable snapshots. Native model parsers/cooking run in disposable importer
processes. Authored files change only through the host's typed transaction.

Public API: `DependencyInventory::prepare`, `from_snapshots`, `sources`, `inputs`,
`users`, `invalidated_by` and `verify`; `SourceDependencies`, `LocalInput`.
`import::run` prepares an import or a read-only conflict plan. `AssetService`
supplies one asynchronous preparation job for a complete asset/folder batch.
Paths are relative to the caller's authored root, with `/` separators. URI spelling
is preserved separately from the canonical local input path so relocation can
preserve companion relationships without rewriting binary models.
`prepare` takes separate project and reference roots: project settings resolve from
the former; source/reference paths and the resulting graph resolve from the latter.
Legacy layouts can pass the same root twice. The caller uses the shared project
contract to choose these roots; this crate never guesses a folder name.

Run `cargo test -p ara-asset-host` and
`cargo clippy -p ara-asset-host --all-targets -- -D warnings`.
Tests use disposable directories and never touch the real user profile.
On Windows set `CARGO_TARGET_DIR` and `ARA_IMPORT_TEST_ROOT` on D:, with explicit
scratch config/crash/TEMP paths; no new targets or profiles on C:.

Preparation and verification read/hash files and belong on background/CLI workers,
never an editor frame. `prepare` delegates dependency discovery to `ProcessWorker`;
the importer executable and its memo cache remain owned by the caller.
Pass the complete current source inventory, including sources not yet imported.
Before committing an asset operation, verify these immutable inputs and the shared
file-state inventory token. A missing, unreadable, escaped or changed input fails
the operation; an incomplete graph must never authorize safe delete.

`asset.import` with `dryRun: true` performs the same source, path, sidecar and
companion validation without staging/history writes or cooking. Conflict rows
use `source`, `destinationLogicalPath`, `existingAssetId` and `kind`.
Real preparation uses `FileHistory` for undo bytes; BLAKE3 importer proofs and
SHA-256 file-state tokens are separate identities. Never compare them directly.
Revalidate bytes, directory membership, importer/policy metadata and authority
revision before one commit. Wait for the host's nonblocking physical receipt
before marking the job done; cancellation stops at the commit boundary.
