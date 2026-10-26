# ara-asset

`AssetPipeline::configure` accepts command-authored in-memory settings; subsequent
snapshots use them before a debounced sidecar save. `unregister` cancels deleted
sources while preserving published cache generations for existing players and redo.

Pure importer contracts, BLAKE3 action/object cache, process jobs, reflected import
sidecars, dependency queries, filesystem events and D20 boundary publication.

Public API: `Importer`/`ImporterRegistry`, immutable `ImportInput`/`ImportOutput`,
`FileCache`/`CacheBackend`, `ProcessWorker`/`ImportJob`, `HotReload`/`AssetSlot`,
`FileWatcher`, `DependencyGraph`, `SidecarCommand`, `register_schema`,
`AssetPipeline`, `SourceAsset` and host-side `snapshot`.

Run `cargo check -p ara-asset`, `cargo test -p ara-asset`,
`cargo clippy -p ara-asset --all-targets`. Set CARGO_BUILD_JOBS=2 on the shared PC.

Importers are installed only in worker executables. Do not invoke importer code on
an editor frame. Source snapshots and cache I/O belong on background threads.
`ProcessWorker` admits at most four native child processes per host process,
shared across handles, discovery and cooks. Set `ARA_IMPORT_WORKERS` before startup
to override (positive integer, always capped by `available_parallelism`; invalid or
zero uses four). Excess jobs stay queued in `ImportJob` and launch when polled;
queued time does not consume the native worker deadline. Cancellation drops queued
requests; a background supervisor reaps children and releases their slots even if
their completed replies have not been polled. Cancellation returns without waiting
for the child. The cold-batch process test is
`cargo test -p ara-asset --test worker_limit` (artifacts under `target/`).
Queued discovery/version calls reserve one slot within the same process cap.
CPU-only and encoded imports each retain one slot when the other class is queued;
idle classes reserve none. Admission stays non-FIFO so an unpolled pending job
cannot block later polled work. `worker_fairness` covers both cases with real children.
Control processes, transport and reaping keep normal priority; native cooks stay
below normal. Dependency discovery does not wait behind an entire encoder queue.
HotReload only publishes from apply_boundary; never install results from poll.
Cancellation/supersession invalidate epochs. Keep current assets alive on errors.
Settings-selected source files use Importer::dependencies_with_settings and
ProcessWorker::discover_with_settings. Concurrent cold discovery shares supervised
one-shot batches; verified discovery memos avoid children on warm opens. Importer
version, source bytes, settings and worker arguments invalidate this pure metadata.
Old workers fall back to individual requests. `worker_metadata` covers both paths.
snapshot includes dependency bytes, so
companion edits participate in action identity and filesystem invalidation.

Sidecars are import documents with one i_settings record. Commands return kernel
Edits; submit these through ara-ops with author/revision metadata. Persist only the
canonical writer's output. The source-media path, not sidecar path, is indexed.

Importer upgrades never hard-fail: ProcessWorker::versions asks the worker for its
installed importer versions; ImportSettings::check_version says Current, Upgrade
(installed is newer: record it with SidecarCommand::Version and reimport) or Newer
(the sidecar needs a newer engine: a real failure). Generation = leading number.

GC requires writers to be quiescent and roots from every active/staged/undo
generation in every process. CacheBackend is the shared cache server seam.

CookPreset is a semantic input (default Balanced); thread_budget defaults to 16 and only
controls the private Rayon pool. Keep scheduling out of action identities.
HotReload stages Fast then final quality on misses; Ready.final_pass distinguishes
them. Cancel/supersede the entire epoch, including HQ. Never downgrade a final hit.
Export via exact ProcessWorker preset, or disable HotReload.progressive explicitly.
ImportContext.metrics is optional offline profiling; never encode timing values.
`ARA_IMPORT_TRACE_DIR` enables canonical JSON event streams (blank-line separated)
for stage, CPU, queue, IPC, cache and final boundary-publication measurements.
ARA_IMPORT_TRACE_CACHE_DETAIL=1 additionally separates cache read/create/write/persist.
Importer/ImporterRegistry::import_with_memo optionally share pure texture tasks;
custom importers and remote caches keep their existing default behavior. FileCache
uses a disposable `intermediates/` namespace. Both caches publish objects atomically
before action records, without per-file fsync. Every hit verifies all object hashes,
lengths and payloads; torn/missing data is a miss that re-imports. Writers repair bad
entries atomically; readers never unlink a concurrent repair. Different verified
results for one key remain errors. Quiescent GC evicts memo entries. Cache recovery,
memo availability and thread scheduling never change final action keys.
FileCache admission coordinates writers across processes on the destination volume
(Windows volume GUID, Unix device ID), using OS-locked leases and disposable
checksummed feedback in OS temp. Warm hits take no leases. Payload publication
latency excludes admission wait: p90 of 16 >=64 KiB writes halves the cap at 25 ms,
and recovers one slot at <=4 ms with hysteresis (1..8; initial four). Smaller action
records are admitted but do not hide payload pressure. Count every live lease when
shrinking; never expire a live writer's cap. Process exit releases slots. A corrupt
feedback record resets scheduling only. No fsync, prerequisite or key/version change.

ImportServices supplies an optional BlockEncoder. Resolve its immutable backend
before action lookup; encoded actions and texture memo keys include it. Capability
metadata hints defer prepare until a cache miss. Hints never alter backend action
identity; miss preparation must confirm the exact hinted identity before cooking.
`versions_with_encoder_hint` shares this device-free lookup with the version query;
project owners must reuse that ProcessWorker for their AssetPipeline.
EncoderUnavailable retries retained inputs with
the CPU key and disables that backend for the session. CPU contract fixtures use
ProcessWorker::with_texture_encoder(false); legacy CPU request bytes stay unchanged.
Optional EncoderEndpoint leases keep an out-of-process device pool alive across
jobs; ImportJob retains the lease even if its ProcessWorker handle is dropped.
record_import_trace records helper CPU/stages through the same opt-in trace sink.
