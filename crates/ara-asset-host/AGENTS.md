# ara-asset-host

Host-side, read-only importer dependency inventory for typed asset lifecycle commands.
It maps each consumer source to its local URI inputs and hashes the exact bytes
returned by the existing out-of-process importer snapshot. It never parses models,
changes authored files, cooks artifacts or publishes runtime generations.

Public API: `DependencyInventory::prepare`, `from_snapshots`, `sources`, `inputs`,
`users`, `invalidated_by` and `verify`; `SourceDependencies`, `LocalInput`.
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

Preparation and verification read/hash files and belong on background/CLI workers,
never an editor frame. `prepare` delegates dependency discovery to `ProcessWorker`;
the importer executable and its memo cache remain owned by the caller.
Pass the complete current source inventory, including sources not yet imported.
Before committing an asset operation, verify these immutable inputs and the shared
file-state inventory token. A missing, unreadable, escaped or changed input fails
the operation; an incomplete graph must never authorize safe delete.
