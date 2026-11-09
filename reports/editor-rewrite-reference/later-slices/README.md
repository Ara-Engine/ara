# Later UX slices preserved from dd0adc3bf

Prepared from `dd0adc3bf` (direct parent `d6c3b6c09`) for sequential application to `5b4c39d61`.
These are split copies of Root's own preserved WIP, not new cross-agent implementation commits.
No repository checkout, index, refs or tracked files were changed by this task. No cargo, builds or code tests were run.

## Artifacts and application proof

1. `01-import-defaults.patch`: 30 files. Typed per-importer defaults, production policy reads, atomic mixed-selection import UI, field bindings, commands and focused tests.
2. `02-package.patch`: 6 files. Package profile/format/output flow on the existing exporter and canonical save receipt; actual completed package artifacts, checked reveal, UI and tests.
3. `03-profiler-ui.patch`: 14 files. Scoped target UI and owned private backend port; actual editor path remains available, player transport deliberately absent until Gaps' handle adapter is attached.

Apply in this order, committing and gating each slice separately. Later patches use earlier UI additions as context. They are separate checkpoint slices, not independent alternate-base patches.
`manifest.json` lists every exact file and SHA-256 of each patch.
`prepare.py` is the reproducible generator. `apply-check/` is a plain scratch file tree (no .git or worktree).
Each patch passed `git apply --check`, was then actually applied to that scratch tree, and every resulting file was compared to its intended intermediate contents. The final 44-file union exactly reconstructs dd0adc3bf except `shell/monkey_support.rs`, where b7's scriptJob ID/quantized age evidence is deliberately preserved alongside the profiler drain state.

These results establish patch applicability and reconstruction only. Compilation, Clippy, runtime tests and visual acceptance remain Root's stream.

## Shared integration ownership

- Import: `shell/mod.rs` gets import_batch module/state/initialization and Binding::ImportBatchField; choices.rs gets ImportBatch/ImportGroup and their lowering; commands/table.rs gets only assets.import_defaults + close/reset. input.rs commits numeric/bool import values immediately and handles ImportBatchField. inspector_panel.rs shows the explicit defaults page before ordinary selected content.
- Package: shell/mod.rs gets last_packages + initialization and packageResult in layout_report; choices.rs gets PackageFormat; commands/table.rs gets build.package + confirm/reveal. Existing Shell::build_profile signature and D30 journey resolver are preserved.
- Profiler: shell/mod.rs gets module/state/initialization, polling participation and CSS; choices.rs gets ProfilerTarget; commands/mod.rs gets target/freeze/compare availability facts; commands/table.rs replaces only profiler actions. actions.rs fills those new Ctx facts for the static/editor service context. editor.rs gets the optional owned backend field, initialized None. monkey_support.rs combines profiler pending work with the existing b7 script-job evidence.
- `app/mod.rs` has no WIP delta and is not present in these patches. No menus.rs edits or new Hub wizard/startup edits are duplicated; New/Open startup support was already in d6.

## Corrections and integration requirements before the gates

1. Concrete unresolved-path risk in the preserved import WIP: `crates/ara-asset/src/import_defaults.rs` declares `MeshSettings.texture_presets` as `BTreeMap<String, ara_asset::CookPreset>`, copied from its former ara-importers location. ara-asset/lib.rs does not alias itself as ara_asset. Change that field to `BTreeMap<String, CookPreset>` (already imported) or `crate::CookPreset` in the import checkpoint before compilation. This artifact intentionally does not silently modify the saved source.
2. The import slice changes reflected ProjectImportSettings by adding ImporterDefaults and moves the public TextureSettings/MeshSettings/audio settings definitions into ara-asset with importer re-exports. The reflection snapshot is NOT present in dd0. Run the explicit opt-in reflection regeneration and `cargo test -p ara --test cli`, then commit the regenerated snapshot in the SAME import checkpoint. Check generated C# bindings if the normal snapshot workflow emits them. Existing reflected names/docs/serde defaults should remain stable apart from the additive policy map.
3. The profiler backend is a private UI port only. `Editor::play_profiling` is initialized None; no production PlayProfilingHandle adapter exists in this WIP. Do not describe live-player capture as complete. Merge Gaps' actual backend and implement the thin owned adapter with exact incarnation/ticket checks before claiming acceptance. The existing tests are controlled-backend UI state tests.
4. Profiler completion diagnostics are retained for display. Keep Status diagnostics display-only when attaching the real backend if its ingress already publishes them, consistent with the recovery seam's publication rule; do not introduce duplicate re-logging.
5. Rebase/real-merge current main first as required by orchestration. The artifact application proof is specifically against 5b4c39d61. Any changed hardfile context after main integration needs Root's serial review; do not apply whole dd0 files over newer work.
6. This task did not assess or change renderer submission/fence code, Host reconciliation, project layout/migration or export backend algorithms.

## Suggested focused verification (not executed here)

- Import: ara-asset tests (especially tests/import_defaults.rs), ara-importers commands::project_defaults_tests, ara-asset-host bindings::defaults_tests and service suites, editor shell::import_batch tests; touched crates' strict all-target Clippy, full editor --lib, fmt, CLI reflection gate. Run ordinary Hub-service/import-worker test targets touched by the mechanical new defaults literal fields. No new diagnostic code was introduced by this WIP, but the normal coverage gate remains appropriate.
- Package: editor shell::build_options and shell::build tests; full editor --lib and strict all-target Clippy/fmt. Native Package form journey and a real completed package artifact/reveal proof still required; checkbox/control tests alone do not establish export acceptance.
- Profiler: shell::profiler_targets tests, full editor --lib, strict all-target Clippy/fmt; later true player target start/stop/snapshot/freeze, stale-incarnation rejection and scoped comparison journey after the production adapter lands. Keep GPU absent/unavailable truthful.

All tests must retain private target profiles. Golden/reflection changes require their explicit opt-in. The native GPU routes obey Root's gate-test exclusion and formal quiet slot rules.