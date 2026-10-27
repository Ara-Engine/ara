# File state in authoring transactions

`ara-doc::file_state::FileState` is an immutable `{hash, len, kind}` token. Hashes
are lowercase SHA-256; `FileKind::Directory` has a distinguished token and includes
empty folders. Absence is `None`. Paths are canonical project-relative strings,
with their spelling and casing retained. The kernel performs no filesystem I/O.

`EditCtx::file_entries()` and `file_state(path)` read the worker-prepared inventory.
`set_file(path, Option<FileState>)` emits `Op::File { doc, id, path, prev, state }`.
The inverse swaps `prev` and `state`; the normal transaction, attribution, conflict,
undo, redo and journal paths handle it. `doc` is the batch's anchor; `id` is a stable
per-path pseudo-record key. Entries may be created, replaced or deleted. Directory
creation precedes descendants; deletion follows descendants. A native document's
record/lifecycle ops remain separate from its physical path transitions.

`ara-host::file_state::FileHistory::new(project_root)` opens the adapter without I/O:

| API | Result |
| --- | --- |
| `put(bytes)` | Durably stored snapshot token, deduplicated by hash. |
| `snapshot(path)` | Optional current token, storing regular-file bytes. |
| `snapshot_tree(path)` | Parent-first inventory, including empty directories. |
| `observe(path)` | Current token without copying bytes into history. |
| `read(token)` | Verified snapshot bytes, for source or media-reference edits. |
| `preflight(path, expected)` | ARA1501 if the current entry differs. |
| `preflight_ops(ops)` | Check every transition and required blob before writes. |
| `restore(path, state)` | Restore bytes/directory, or remove an empty entry. |
| `materialize(ops)` | Preflight, then materialize; attempt rollback on write failure. |

These methods perform I/O and belong on the owner's preparation/file worker.
Regular-file snapshots stream through a bounded buffer into `.ara/file-history/`.
Temporary blobs are synced before rename; readers verify hash and length. Symlinks,
escaping paths and `.ara`/`.git` targets are refused. Removal is never recursive.

At completion, `Cx::observe_files` (or `Store::observe_file`) publishes the initial
inventory and destination absence to the pure kernel. The typed batch then stages
and commits against those tokens. Host drain appends the transaction to the journal
before queueing file materialization on the same ordered file thread. Native-document
creation validation recognizes planned source-media writes in that transaction.
File undo/redo preparation uses the same snapshots and runs as a standard job.

`PreparedInventory` carries initial `entries` (including explicit absence), complete
directory-child listings, and an opaque `metadata` JSON value. `ReadCtx::prepared_files`
and `prepared_metadata` expose this immutable handle; `EditCtx::prepared_metadata`
exposes the same owner slot. A command cannot `set_file` outside the checked inventory.
Filesystem aliases share state/conflict keys, while ops retain the requested spelling.
Every adapter must use `ara_doc::file_state::{same_path, path_key}` rather than its
own case conversion. Windows conservatively combines Unicode simple uppercase and
simple case folding, without locale, expansions or normalization. This includes
ordinal aliases such as dotless i/I and sigma/final sigma; ASCII keys remain compatible
with existing history. macOS conservatively uses Unicode 9 full case folding and canonical
decomposition for APFS aliases (also on case-sensitive volumes). Other Unix platforms
preserve case. Contradictory alias states and duplicate directory aliases are refused.

Every observation, snapshot and restore rechecks the path, including the target.
Windows refuses all `FILE_ATTRIBUTE_REPARSE_POINT` entries, including junctions;
Unix refuses symlinks and device changes below the project root. The protected
snapshot-store path is checked too. Unsafe paths return ARA1003 (I/O); the file
worker checks again after parent creation and before publishing restored bytes.

`FileHistory::prepare_tree` builds a complete subtree on a worker. Before authority
commit, call `revalidate` for byte/existence/directory checks, or `revalidate_with`
to additionally check opaque policy/worker identities. The adapter never parses
importers or owner metadata. Finish through `Cx::apply_prepared(params, inventory,
prepared_at_revision)`: it checks the captured authority revision, publishes the
handle only for that batch, preserves the initiating session and returns ordinary
command outputs/transaction/ops. A changed authority or inventory returns ARA1501;
refresh/revalidate before retrying. Source replacement uses distinct preparation
and pre-commit revalidation worker phases.

On restart, the host recovers file transitions before rebuilding document history.
Durable `fileFlush` acknowledgments distinguish completed writes from pending
transaction intents. After a completed flush, external bytes are preserved even
when they intentionally restore an older version. Only unfinished transitions
are replayed; their last matching state identifies a partial write. Unknown bytes
are preserved and reported as an external conflict. Compaction retains the last
file acknowledgment, including absence. `restore_documents` replays only
document data; the host separately supplies recovered file observations.

`source.replace {path, baseHash, text}` requires an existing C# source whose exact
hash matches analysis. It returns a standard preparation job, then an ordinary
`cmd.apply` result with transaction and ops. External changes refuse apply/undo
with ARA1501. CLI `Engine::call` advances local endpoints until preparation jobs
finish, so offline commands cannot exit and discard their queued work.
