# Prepared asset commands

`asset.move`, `asset.rename`, `asset.duplicate`, `asset.delete`, and `folder.create`
share one `cmd.apply` transaction and one undo/redo step. Parameters and output
paths are relative to `Workspace::reference_root()`; file-state keys are physical
project-relative paths. Root settings, packages, and derived data stay outside
this authoring boundary. Visible nested authored folders keep their chosen names.

The commands perform no filesystem reads, importer work, or blob-store writes.
The host freezes files, directory listings, parsed native documents, JSON owners,
project import policy, and the complete recognized-source dependency graph on a
worker. Sources without sidecars still participate in discovery. Unrecognized raw
files remain file facts; they do not pretend to be parsed consumers.

`PreparedInventory.metadata` contains `AssetMetadata` directly (`format` is
`ara.asset/1`). `FileState.hash` is SHA-256; consumer/input `blake3` fields are
independent importer identities. A shared host token binds the physical snapshot,
policy, importer versions, request, and base revision. The host revalidates that
snapshot immediately before committing the original batch.

`preview_assets(workspace, schema, batch, &mut inventory)` projects calls in order
on a private workspace with no I/O. It preserves the original physical proofs,
adds destination absences, and records one `AssetOperationMetadata` per normalized
request and staged-op offset. Later calls see projected documents, directories,
canonical JSON, source paths, and local-URI consumers. Per-call diagnostics retain
the original command index and parameter location; a failed preview never commits.

Preview returns canonical `AssetBlob` values and changed `AssetDocument` values.
The host encodes native documents with the real project codec, stores their bytes
through FileHistory, fills the matching operation's `document_states`, and runs a
second pure preview. Pure document checksums bind those codec states to the exact
values and deterministic IDs. Only final JSON blobs need storage. Actual commands
refuse missing or mismatched prepared proofs.

Move and delete reject external local-URI consumers with actionable owner paths.
A moving or duplicated model keeps its binary URI text and copies needed local
companions reversibly. Recognized companions recursively carry their own source
inputs and remain consumers in later batch calls. Identical destination bytes are
reused; different bytes fail the whole plan. Native duplication remaps asset and
record IDs, internal references, and contextual UI bindings using deterministic
IDs bound to the checked operation token; all collisions are checked before edits.

Bare strings are paths only in explicit owner fields: marker/policy main and UI
bootstrap paths, look defaults, UI resource fonts/images/click, and export-preset
icon/scriptProject. Generic `@path#id` references and typed identity references are
repaired separately. Entity names and arbitrary strings matching a folder name
remain unchanged. The same owner rules govern delete blockers.

`folder.create {path, parents:false}` requires an existing parent;
`parents:true` adds missing empty parents. Empty directories use the shared
`FileState::directory()` op and participate in undo/redo like regular files.

`asset.import` is one separately prepared asynchronous job accepting multiple
absolute OS sources. Its reflected `dryRun` field follows the shared camelCase protocol.
Read-only preparation returns `conflicts` and `planned` without staging, cooking,
blob-store writes, or a transaction. Real preparation attaches immutable parsed
entries plus `after_inventory` and commits through the same file-state primitive.
`remap_import` is the pure pre-cook identity/path adapter; transport entries keep
original foreign IDs so commit never remaps twice. Replace preserves destination
identity and author settings. The host checks edit:assets, import, and external
filesystem capabilities before reading the selected paths.

Focused gates (run by the integrating build owner):

```text
cargo test -p ara-cmd --test assets
cargo test -p ara-cmd builtin::asset
cargo test -p ara-asset-host
cargo clippy -p ara-cmd -p ara-asset-host --all-targets -- -D warnings
cargo test -p ara --test cli
```

Registry catalog and CLI reflection snapshots must be explicitly refreshed for
new reflected command fields. Validation uses existing catalogued diagnostics.
