# ara-host

One project's engine host behind the Engine Protocol: sessions and scopes, the
method registry (`Service`) and the core methods, review state, checkpoints, the
journal, always-saved persistence and external-edit reconciliation. The daemon
(`ara serve`) serves it; the offline CLI and tests call it in-process.
Depends on ara-cmd, ara-ops, ara-doc, ara-merge, ara-schema, ara-proto, ara-diag.

## Public API

- `Project::open(root)` (needs `ara.project.json`), `project::discover(start)`,
  `project::no_project` (ARA1901 + `ara init` fix), `MARKER`, `STATE_DIR` (`.ara`).
  Scans skip hidden dirs, `target/`, `node_modules/`, `spikes/`, nested projects.
- `Host::open(project, HostConfig)`: takes `.ara/lock` (ARA1903 if held), loads
  documents, replays `.ara/journal`. `call(conn, method, params)`,
  `handle_text(conn, json)`, `tick()` (watch + debounced flush + service ticks),
  `flush()`, `closed(conn)`, `drain_outbox()`, `stopping()`, `catalog()`,
  `register(Box<dyn Service>)`, `set_approval`, `kill()` (drop without flushing;
  crash tests). Implements `ara_proto::Endpoint` as `ConnId::LOCAL`.
- `Service { name, methods(db), call(cx, method, params), tick(cx) }`;
  `Method::new::<P>(db, name, summary).scope(..).read_only().destructive()
  .tool("ara_x") | .action("ara_x", "verb") | .sessionless()`; `parse::<P>(params)`.
- `Cx`: `session()`, `conn()`, `store()` (read-only), `registry()`, `root()`,
  `apply(ApplyParams)` (edits = `cmd.apply`), `log`, `notify`, `start_job`,
  `job_progress`, `finish_job`, `job_cancelled`.
- `Approval` hook (`DefaultPolicy`: humans everything, agents
  `Scope::agent_default()`); the decision is stored on the session, journaled and
  logged (ARA1812, denials ARA1805).
- `core_methods()`: the core catalog without a host (offline `ara docs`).
- Modules: `params` (all core params types), `query` (compact syntax), `text`
  (scene-to-text), `docs` (llms.txt), `journal`, `logs`, `review`, `jobs`, `tree`.

## Core methods (`core/`)

`rpc.discover`; `session.begin/status/diff/checkpoint/rollback/end`;
`history.list/undo/redo`; `review.list/accept/revert`; `schema.list/describe/search`;
`query`; `inspect`; `cmd.apply`; `logs`; `docs.search`;
`jobs.list/status/start/cancel`; `subscribe`/`unsubscribe` (topics `ops`,
`diagnostics`, `history`, `jobs`); `doc.sync`; `daemon.shutdown`.
Reserved families (`run`, `capture`, `play`, `test`) answer ARA1806 until a
service registers them.

## Semantics

- Every call: route → session + scope (host) → params validated against the
  params type → service → **drain**. The drain journals each commit, tracks agent
  commits for review, schedules the flush and notifies subscribers. Command scopes
  are checked by the ara-cmd registry inside `cmd.apply`.
- Review: agent transactions start `applied`; `review.revert` undoes a whole
  transaction (store undo) or a subset of entities (a compensating transaction
  based on the target's revision, so later edits conflict). Only human sessions
  accept. A redo (undo of an undo) makes the original `applied` again.
- Checkpoints: one per agent `cmd.apply` turn (at the revision before it), plus
  manual ones; `session.rollback` undoes the session's later transactions all or
  nothing.
- Journal: `txn`, `flush` (doc, rev, FNV hash), `session`, `ended`, `checkpoint`,
  `review`, `base` lines. On open, each file's revision is its newest flush entry
  with a matching hash; newer transactions are re-applied. Compaction (past 4096
  entries, only when nothing is unsaved) keeps the newest 1024 transactions.
- Flush writes changed documents canonically after `debounce` (temp + rename) and
  always journals a `flush` entry, even when the text is unchanged.
- Watcher (polling): changed file → read → three-way merge (base = last synced)
  → human `external edit` transaction. Invalid files and conflicts mark the doc
  diverged (no flushes) until `doc.sync {keep: memory|disk}` or a clean edit.

## Gotchas

- Host IDs (sessions, checkpoints, jobs) and the store's IDs come from
  `HostConfig::seed`; reopening with the same seed in tests re-mints the same IDs.
- `*.types.json` changes are only reported (ARA1912); restart to load them.
- No fsync per journal line: safe against process crashes, not power loss.
- Sessions survive restarts (journaled); subscriptions don't.

## Testing

`cargo test -p ara-host`. Unit tests: query parsing, logs, journal compaction,
review roll-up. `tests/host.rs` (fixture project copied to a temp dir): JSON-RPC
round trips, scopes, dry run = apply, symbols, review per entity, rollback, crash
replay, external edits/conflicts, service registration, subscriptions with a
registry job's progress. Snapshots: scene-to-text.
