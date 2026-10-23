# ara-asset

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
HotReload only publishes from apply_boundary; never install results from poll.
Cancellation/supersession invalidate epochs. Keep current assets alive on errors.

Sidecars are import documents with one i_settings record. Commands return kernel
Edits; submit these through ara-ops with author/revision metadata. Persist only the
canonical writer's output. The source-media path, not sidecar path, is indexed.

Importer upgrades never hard-fail: ProcessWorker::versions asks the worker for its
installed importer versions; ImportSettings::check_version says Current, Upgrade
(installed is newer: record it with SidecarCommand::Version and reimport) or Newer
(the sidecar needs a newer engine: a real failure). Generation = leading number.

GC requires writers to be quiescent and roots from every active/staged/undo
generation in every process. CacheBackend is the shared cache server seam.

CookPreset is a semantic input (default HQ); thread_budget defaults to 16 and only
controls the private Rayon pool. Keep scheduling out of action identities.
HotReload stages Fast then final quality on misses; Ready.final_pass distinguishes
them. Cancel/supersede the entire epoch, including HQ. Never downgrade a final hit.
Export via exact ProcessWorker preset, or disable HotReload.progressive explicitly.
ImportContext.metrics is optional offline profiling; never encode timing values.
