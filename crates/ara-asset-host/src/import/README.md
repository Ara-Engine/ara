# External asset preparation

`run(request, context, cancellation, progress)` is the shared background entry:

- `Request { sources, destination, conflict, dry_run }` uses explicit absolute OS
  source files/folder roots and an authored-relative destination folder.
- The typed protocol exposes Rust `dry_run` as `dryRun`, following shared camelCase.
- `Outcome::Preflight(ImportPlan)` reports readonly `conflicts` and `planned`.
- `Outcome::Ready(PreparedImport)` owns immutable staging until typed commit or
  explicit `discard`. Dropping it only cleans its own sealed fresh staging path.

Wire conflict fields are exactly `source`, `destinationLogicalPath`,
`existingAssetId` (nullable), and `kind`. Kinds are `file`, `directory`,
`type_mismatch`, and `sidecar`. Collisions are recorded before the policy resolves
them, so Rename/Replace/Skip all retain real conflict rows. Rename keeps compound
extensions and paired sidecars together. A binary-model URI collision can regroup
the selection into a fresh folder instead of breaking its unchanged URI.

Both modes share path/root/link checks, complete destination validation, entry and
byte budgets, disk admission, exact source reads, importer/foreign-sidecar binding
resolution and out-of-process companion discovery. Dry run performs no staging,
cooking, file writes or inventory mutation. Cooking/tool execution errors remain
execution-phase errors. The host native-document adapter must perform its same
native/sidecar decoding in both modes; `read_entry` shares guarded chunk reads and
verifies the BLAKE3 proof before returning bytes to that adapter.

An explicit selected folder is authored content: visible nested names such as
`builds`, `target`, `packages` and `node_modules` remain content. Hidden engine/Git
trees are ignored and a whole Ara project drop is rejected. Links/reparse points
and linked ancestor components are refused before canonicalization/open. A model
can read companions only within the explicit selected folder roots or selected
files; an implicit parent directory never grants read authority.

`Context` supplies frozen destination facts, importer capabilities/effective
settings, the existing disposable `ProcessWorker`, and a platform disk callback.
The disk requirement is an estimate: staged bytes plus immutable incoming copies,
replacement undo snapshots, expanded/cooked allowance and metadata overhead.
Legacy project-root authoring may stage only in protected `<project>/.ara/...`;
modern authoring stages outside the authored root. No real user profile is used.

Prepared entry `digest` and existing entry `digest` are **BLAKE3 importer-byte
proofs**. Shared `ara_doc::file_state::FileState.hash` is **SHA-256**. Never compare
these identities directly. The host stores prepared bytes through FileHistory and
attaches the resulting SHA states to the pure typed command metadata.

The source module uses no native format parser in process and adds no dependency.
Focused tests use disposable paths under `ARA_IMPORT_TEST_ROOT`, falling back to
`CARGO_TARGET_DIR`. On the shared Windows PC, set the former to
`D:\AraBuild\ux-hub-target` before running them. No tests are run by this writer.
