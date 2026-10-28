# ara-host
Project Engine Protocol host: sessions, services, attributed history, review, persistence and reconciliation; used by `ara serve`, CLI and tests.
## Public API
- `Project::open(root)` (needs `ara.project.json`), `main_scene()` (`main` in the
  marker, else `scenes/main.scene`), `project::discover(start)`,
  `project::no_project` (ARA1901 + `ara init` fix), `MARKER`, `STATE_DIR` (`.ara`). Scans skip hidden dirs, `target/`, `node_modules/`, `spikes/`, top-level `builds/` and `packages/`, nested projects.
- `project::authored_files(root, db)`: one semantic inventory, sorted by unique storage_path;
  authored paths are reference-relative, metadata/settings have Project scope.
- `project_layout::{AUTHORING_ROOT, authored_root(root)}`: one authored-root contract.
  Current projects use content-relative references and project-relative storage paths.
- `project_layout`: `require_current_layout` rejects old opens (ARA1914). `migration_plan` is read-only.
  `migrate_project` submits one typed, journaled command after consent and waits for its physical receipt; `Migration::apply` delegates.
  History stays append-only; Undo restores old bytes and closes normal authoring.
- `Workspace::insert_project_document` keeps root policy transactional without adding
  it to the asset index; `is_project_document` excludes it from portable game data.
- `Host::open(project, HostConfig)`: takes `.ara/lock` (ARA1903 if held), loads
  documents, replays `.ara/journal`. `call(conn, method, params)`,
  `handle_text(conn, json)`, `tick()` (watch + debounced flush + service ticks),
  `flush()`, `unsaved()`, `closed(conn)`, `drain_outbox()`, `stopping()`, `catalog()`,
  `register(Box<dyn Service>)`, `set_approval`, `kill()` (drop without flushing;
  crash tests). Implements `ara_proto::Endpoint` as `ConnId::LOCAL`.
- `Service { name, methods(db), call(cx, method, params), tick(cx) }`;
  `Method::new::<P>(db, name, summary).scope(..).read_only().destructive()
  .tool("ara_x") | .action("ara_x", "verb") | .sessionless()`; `parse::<P>(params)`.
- `Host::profiler()`: frame/timestamp/worker integration for the registered
  `profile.start/stop/summary/hitches/compare` service and `ara_profile` MCP tool.
  Presentation may own frame boundaries; host ticks reuse an already open frame.
  Summaries accept `topN`, `limit`, `maxBytes`; comparisons use JSON summary data.
- `Cx`: `session()`, `conn()`, `store()` (read-only), `registry()`, `root()`,
  `apply(ApplyParams)` (edits = `cmd.apply`), `log`, `notify`, `start_job`,
  `job_progress`, `finish_job`, `job_cancelled`.
- `Approval` hook (`DefaultPolicy`: humans everything, agents
  `Scope::agent_default()`); the decision is stored on the session and journaled;
  approvals are a `tracing` debug event (ARA1812), denials a warning (ARA1805).
- `core_methods()`: the catalog of the always-registered services (core, profile,
  diag, why) without a host (offline `ara docs`, MCP tests).
- `Host::why()`: the `ara why` registry; subsystems `register` explainers and `add`
  collectors. `HostConfig::diagnostics`: bus retention per severity.
- `Host::review_entities(txn)`: read-only per-entity review status for native
  hierarchy/viewport overlays; uses the same state as review accept/revert.
- Modules: `params` (all core params types), `query` (compact syntax), `text`
  (scene-to-text), `docs` (llms.txt, a page per code), `journal`, `review`, `jobs`,
  `tree`, `diag` (the bus service, `.ara/diagnostics.jsonl`), `why` (`HostFacts`).
## Diagnostics bus (`diag.rs`, `why.rs`)
- `Host::open` attaches the process's bus consumer; every tick drains other threads'
  emits into the store (`ara_diag::store`), and `Cx::log` / failed calls push
  directly, attributed to the calling session's author. Subsystems on the host's
  thread during a call are attributed through `bus::scope`.
- `logs` (`ara_logs`): grouped by code (counts, subjects, newest message, best fix)
  within `budget` characters; `group: false` lists entries. Answers are the text
  (`{cursor, total, text}`) unless `limit` asks for `entries` and `summary`: MCP
  clients pass `structuredContent` to the model, so JSON costs tokens. `diag.query`
  always answers in full and adds time, tick and text filters; `diag.subscribe` streams
  `diag.event {subscription, record}`; `diag.clear`; `diag.report` lets other processes
  push diagnostics (`engine: true`, `tick`, per-item `causedBy` index).
- Engine reports (bus events, `diag.report`) persist in `.ara/diagnostics.jsonl`
  (appended every 500 ms, rewritten when 4× larger than the store); call errors do
  not. Restored records keep causal links (by dedup key).
- `why` (`ara_why`): `Why::explain` over `HostFacts` (resolved trees cached per
  question, effective component values, the asset index, project files, the bus,
  a profiler summary); `list: true` lists explainers. It answers `{text, fixes}`
  (runnable fixes with their cause code); `full: true` returns the whole `Report`.
  `docs.search` answers a code
  query with its `errors/<code>` page.
## Collaboration (D24)
- `collab.join/leave/presence/who/submit/sync/undo/redo/conflicts/activity`;
  optional `branch.open/list/status/submit/merge/discard`. Branch submit merges
  immediately. Legacy claim/release/heartbeat/claims methods are advisory only;
  no ops guard, idle policy, approval gate or claim rejection remains.
- `HostConfig::overwrite_window` defaults to ten minutes. Ordinary command writes
  and replica conflicts share the conflict log. Both participants receive
  `collab.conflict`; `editor.overwrite` also goes to editor subscribers and logs
  ARA2503, visible through `logs`/`diag`.
- `editor.gesture {session,doc,entity,path,active}` is human-only. The host stages
  agent command ops touching those fields, applies other fields immediately, and
  releases held ops in order after end or two seconds. Replica agent submits
  and optional atomic branch submission use the same gesture boundary. No sleeping on the host
  thread. `cmd.apply` reports `gestureHeld` and preserves turn checkpoints.
  Split commits share a turn ID for Undo turn; rollback cancels held operations.
  Editor gizmos, scrubs, text commits and Escape call begin/end automatically.
- Hub reads `collab.activity` without opening an artificial editor session;
  presence remains attributed to actual participants.
- Origins and txn/session maps preserve author attribution. Every call drains
  committed ops, publishes notifications and persists optional branches.
## Core methods (`core/`)
`rpc.discover`; `session.begin/status/diff/checkpoint/rollback/end`;
`history.list/undo/redo`; `review.list/accept/revert`; `schema.list/describe/search`;
`query`; `inspect`; `cmd.apply`; `logs`; `docs.search`;
`jobs.list/status/start/cancel`; `subscribe`/`unsubscribe` (topics `ops`, `diagnostics`, `history`, `jobs`); `doc.sync`; `daemon.shutdown`.
CLI/daemon register `run`/`capture`/`play`/`test` through ara-play-host; bare Hosts report ARA1806 for absent services.
- `cmd.apply` / `inspect`: `doc` is optional (`core/pick.rs`): the one document
  holding every entity the call names (IDs, addresses, `name:` selectors), else
  the only scene, else the main scene; else ARA1814 with a fix per candidate. Named documents also match by unique stem or file name (`outpost`).
- `cmd.apply` returns `doc` (path) and `text`: what was applied where, then every
  created / modified / deleted record with its name and symbol (`would …` for dry
  runs) and the rollback checkpoint. Agent sessions need a `why` (ARA1610); the
  error comes with everything a dry run of the batch reports. Registry fixes are
  relocated to `commands[i].args…` like their locations. `as` may equal `args.id` and may skip its `$`. `schema.describe` takes `names` or one `name`.
- `cmd.apply`'s catalog entry lists every command with its args in the `cmd`
  field's description (`list_commands`), so MCP clients see them without lookups.
- `session.begin` also returns `documents` (path, kind, ID) and `mainScene`.
- Native `.ui` XML projects to `UiSource`: `doc.create` seeds markup; `ui.source.apply`
  validates lossless ops. Save/fmt use `Project::read_document`/`write_document`,
  preserving XML/identity comments. Reconcile UI conflicts before native persistence.
- `Cx::main_scene` reads unsaved transactional `project.setMain` settings for Play's default.
- `inspect` entities and `query` terms accept names: `name:Crate`, a bare word (`query`: names containing it; `inspect`: one exact match).
- Scene-to-text flags unresolved merge conflicts with the `ara resolve` hint.
## Semantics
- Every call: route → session + scope (host) → params validated against the
  params type → service → **drain**. The drain journals each commit, tracks agent
  commits for review, schedules the flush and notifies subscribers. Command scopes are checked by the ara-cmd registry inside `cmd.apply`.
- Review: agent transactions start `accepted` under amended D24; `review.revert` undoes a whole
  transaction (store undo) or a subset of entities (a compensating transaction
  based on the target's revision, so later edits conflict). Accept remains an
  idempotent compatibility action for humans and agents. Redo restores `accepted`.
- Entity-only revert selects the caller agent's latest applicable turn; human
  callers select the latest turn, including human-authored edits. Explicit transaction IDs may target other
  agents. Results carry author, reason and IDs; compensating transactions keep
  their untracked status across journal replay.
- Checkpoints: one per agent `cmd.apply` turn at its prior revision, plus manual ones.
  `session.rollback` undoes the session's later transactions all or nothing.
- Journal: `txn`, `flush` (doc, rev, FNV hash), `session`, `ended`, `checkpoint`,
  `review`, `base` lines. On open, each file's revision is its newest flush entry
  with a matching hash; newer transactions are re-applied. Compaction (past 4096
  entries, nothing unsaved or being written) keeps the newest 1024 transactions.
- File I/O runs on the file thread (`file_io.rs`) in request order; source/asset snapshot API: [`file-history.md`](../../docs/dev/file-history.md).
  thread: journal lines and compaction, `diagnostics.jsonl`, writes after `debounce`
  (temp + rename; `flush` journaled when it lands), removals, `notify` events (30 s
  rescan); `flush()` waits for it all. `watch_every` zero checks per tick (tests). An
  external edit three-way merges (base = last synced) as a human `external edit`; invalid
  files and conflicts mark the doc diverged until `doc.sync {keep: memory|disk}` or a clean edit.
## Gotchas
- Host/store IDs use `HostConfig::seed`; reopening with the same test seed re-mints them.
- `*.types.json` changes are only reported (ARA1912); restart to load them. Sessions
  survive restarts; subscriptions don't. Commands return before their journal line
  lands (in commit order): a crash loses only queued lines; no fsync per line.
## Testing
`cargo test -p ara-host`: query parsing, logs, journal compaction, review, a held file thread.
`tests/host.rs` copies fixtures to scratch: protocol, scopes, dry runs, symbols, review, recovery, external edits/conflicts, file thread, services/jobs/subscriptions, document inference, text and `why`.
`tests/collab.rs`: advisory claims, presence, branches/restart/submits and participant undo. Scene-to-text snapshots.
`tests/diag.rs` serializes the global bus: grouped/attributed logs, cross-thread
subscriptions, report/clear, executable `why` fixes and restart persistence.
`Cx::create_document` delegates to `doc.create`, including format templates; fill documents with `Cx::apply` for normal attribution/history/undo.
## Structured reference (`docs.json`)
`docs::structured` writes canonical `ara.docs/1` JSON via `ara_json`: `format`, `engineVersion`, `protocolVersion`, `types`, `commands`, `methods`, `diagnostics`, `csharpExamples`, `topics`.
Catalogs use stable schema/command/method/code keys. Fields carry type, unit, range, materialized default, doc, replication and examples.
Commands carry parameter JSON Schemas; methods carry scope, params and extensible result Schemas. Optional result properties accommodate parameter-dependent payloads.
C# examples carry page ID/source/region/compiled code from `dotnet/examples/`, embedded in markdown, full text and MCP search.
`ara docs --json` prints the file-count summary. Packages ship `docs.json`; signed release manifests list its hash, size and URL.
Run `cargo test -p ara-host --test docs` for round-trip and complete catalog/region coverage.
