# Native profiler target journey candidate

D-only prepared source, not compiled, gated or run. Base for exact integration patches: a5c2c20b5 (contains profiler UI 476c7e28b and real backend a2b8ad1a). All three tracked-source patches passed git apply --check and actual application in apply-check/, a plain file tree without .git. This proves applicability only.

Copy profiler_verify.rs and profiler_verify/ into crates/ara-editor/src/app/. Apply options-entry.patch, app-wiring.patch and shell-report.patch serially, reviewing newer Root hardfile context. Never copy app-mod.rs.candidate over Root's current app/mod.rs; that full file exists for patch generation/review only. controls.rs.candidate is likewise the complete read-only report candidate, not an instruction to overwrite unrelated work.

The independent --verify-profiler flag creates a fresh layout-v4 scene under --output/profiler-project-<PID>, refuses stale output fixtures, never opens/mutates the positional project, and forces normal offscreen/profile isolation through Options. The typed setup creates a camera, box and light, sets Host+two separate clients / low quality, then takes a Scene fingerprint/content-history baseline. Measured actions are raw SDL_PushEvent -> Window::poll_tagged -> production event loop. Opening/maximizing Profiler uses its native palette and Shift+Space. Client selection uses the actual dropdown inventory and keyboard selection; no direct registry runner, fake Status, controlled adapter, profile file or fallback target is used.

Eight cases:
1. Native Play start, actual Client roster and native open/maximize Profiler.
2. Select first actual Client incarnation and start recording.
3. Explicit Follow target/Snapshot click yields a newer capture and preserves recording.
4. Freeze stops recording; the immutable capture stays byte-summary-identical across owner ticks.
5. Baseline retains the first client's explicit scope.
6. Select and record/freeze a second real Client; retain the first baseline.
7. Comparison displays both distinct world/player/source/binding scopes.
8. Stop Play, keep the selected unavailable incarnation, start replacement Play with new identities, prove no implicit selection or data fallback, then stop the replacement too.

The read-only report addition exposes actual bounded capture facts: complete frame count/duration range, real player.frame scope presence, span count, producer threads, selected frame, GPU samples (null when absent), stable frame digest, roster/recording/compare/pending facts. It never clones a Profile or logs Status diagnostics. Aggregation runs only when this verification/report is explicitly requested. The verifier samples reports at 4 Hz and reads an exact report at successful capture boundaries.

Assertions require <=120 nonzero-duration closed frames, real player.frame spans, exact selected incarnation, CPU label equality with the actual selected frame, truthful GPU dash when that frame has no samples, separate baseline/current scope labels, Scene hash and authoring revision unchanged, disabled stale target controls and no fallback. Profiling timings are informal (shared GPU); this route imposes no speed threshold.

Evidence: profiler-targets.json, one original native PNG/layout/feedback JSON per successful case, and bounded pointer/selection trace. The shared feedback geometry audit must pass. A failure stops the route, writes the failing phase/report and is never accepted; the app's existing game.close path closes its owned Play state before validation. No universal worker-retirement claim is added.

Seven CPU-only tests validate the fail-closed evidence predicates (missing scope, substitute editor data, open/zero-duration frames, capacity, missing real scopes/binding). They are not synthetic native acceptance. Root must run them plus touched-crate gates, full editor library and a matching Windows editor/player build before actual GPU proof.

Optional runner-support.patch targets only D orchestration scripts (not repository files): it adds profiler to existing run-native.ps1 and generic gate guard mode lists, and treats its positional project as the internally replaced unused fixture. Root must review/apply it after the current runner has finished; this task did not modify or restart any active script.

After Root applies the sources, gates/builds a matching checkpoint and applies runner mode support:

powershell -NoLogo -NoProfile -File D:/AraBuild/ux-editor-target/orchestration/run-native-gate-guarded.ps1 -CheckpointSHA <eight-character-built-source> -Mode profiler -EvidenceMode correctness-queued

This preserves ara-dev gpu admission and the existing newest-main-gate polling/preemption rule. No GPU slot, build, test, native process, UI window or process termination was used while preparing this candidate.
