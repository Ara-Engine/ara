# Showcase cross-engine benchmark

Final scope: **Showcase only; Outpost and Hangar not run.** Islam stopped the benchmark after its Showcase checkpoint. Outpost source preparation had begun, but no Outpost cross-engine results were accepted. No further engine runs were made after the stop instruction.

- Final report: https://ed90141500766ae0.snist.dev
- Immutable final version: https://ed90141500766ae0.snist.dev/v/2
- Full local archive: `D:\AraBench\`, with original 4K PNGs, source Blender/glTF, packaged players, videos, raw timings, logs and frozen projects.
- Ara baseline: `13e8b9da83c0fdd36d0d12ea3a7fb8ce7a819c23`, preview `0.0.2-alpha.3`.
- Unity: `6000.6.6f1`, URP `17.6.0`, Windows Mono, D3D12.
- Unreal: installed UE 5.7.4, Windows Development, D3D12.

This directory stores optional external benchmark tooling. Python, Blender, Unity and Unreal are not prerequisites for building Ara itself. No benchmark patch is applied to production crates on this branch.

## What is preserved

`ara/timing.patch` contains the exact visible-player, continuous-camera, duration/readiness, immediate-presentation and editor-orbit/Play instrumentation used for the accepted timings. `ara/capture.patch` adds the temporal-history-preserving readback used for the completed Showcase movie. Each patch applies independently to the baseline source commit. The capture adapter in `scripts/ara_benchmark.rs` is the compiled Showcase version; the later unbuilt color-only readback experiment is excluded.

`unity/` freezes the verified Showcase scripts, quality/renderer/post-process settings, scene, package lock and project settings before later-scene adaptation. Generated glTF and large baking payloads are omitted; the complete frozen project is local at `D:\AraBench\snapshots\showcase-unity`. The source glTF is `D:\AraBench\source\showcase\showcase.glb`.

`unreal/` freezes the project and rendering configuration. Scripts create the imported scene, camera sequence, common lights/sky, manual exposure, MRQ stills/movie and editor probes. Generated Content, DDC and builds stay local. An unused Android file-server settings section is omitted from the archived config.

`source/manifest.json` is the common 1,200-sample camera path, fixed cameras and lighting contract. `ara/project/` preserves the canonical Ara scene. `provenance.json` hashes the exact source and player artifacts. `results/` contains the report and accepted summaries, including separate quiet and call conditions. It does not include private process scans or build caches.

## Rerun after renderer work

**Do not run anything until Islam reserves the PC again.** These scripts intentionally use the original PC paths. Preserve `D:\AraBench` as the baseline. Create a separate rerun directory and consistently replace the `D:\AraBench` root in copies of the harness, including C#, Python, PowerShell and Rust instrumentation. Do not overwrite original measurements or published baseline media. Keep the guard repository at `D:\Coding\projects\desktop\snengine` unless Islam changes that requirement.

1. Reuse the frozen common Blender/glTF/manifest content. For a source reconstruction, run Blender's `build_source.py -- showcase`, then `environment.py showcase`; verify the rendered scene against the retained originals before measuring. The later helper-display suppression affects only hidden importer bone shapes. The accepted scene has 67 rendered mesh objects and 407,603 triangles.
2. Create an isolated Ara source snapshot. Apply `ara/timing.patch` at the recorded baseline, or adapt its instrumentation to the newer renderer and inspect every conflict. Never apply it blindly to the main checkout. Build the matching editor/player and CLI/export/import tools in release with `CARGO_BUILD_JOBS=8`, isolated target/config/crash directories, and the repository's `ara-dev cargo` companion checks. `prepare_ara.py showcase` authors through typed commands and exports the packaged player. Freeze its runtime template and executable before capture instrumentation.
3. Restore the frozen Unity project; provide the common glTF at `Assets/showcase.glb`. Run `run_unity.py prepare showcase`, `bake showcase`, then `package showcase`. The APV bake requires graphics-enabled batch mode. Keep domain reload on for the default Play-entry probe, and restore it after the reload-off probe.
4. Restore the Unreal project/config. Run `run_unreal.py import showcase`, `finalize showcase`, then `build showcase` (`RunUAT BuildCookRun`, Development). Keep `-build`; enabled plugins can require the installed engine's temporary target.
5. Check games/OBS with `guard.ps1` and `nvidia-smi pmon -c 1`. Measure a fresh engine-free call baseline; `measure_call_baseline.py` stores the 30-second reading and constructs the policy. Never reuse historical process identities. Only DWM allowance changes to idle p95 + five percentage points. Discord remains charged by the original 10% / five-sample foreign-load check. New GPU identities over .5% for two samples invalidate the additional check. Do not stop user apps.
6. Use **only `run_interleaved.py showcase`** for the primary runtime matrix: Ara → Unity → Unreal for each repeat, High/Epic at 1440p/4K, three repeats expanded to five for every engine in a noisy configuration. The script stores every run, guard evidence and spread decisions. There is no concurrent engine timing. All timing commands run under `cargo run -p ara-dev -- gpu --` from the required original repository.
7. Editor probes: `run_editor.py ara showcase`, `run_editor.py unity showcase`, `run_unreal.py editor showcase`. Orbit scopes/layouts differ; do not rank pixel-normalized editor renderer speed from these values. Startup markers differ too: no comparable OS first-present result was obtained because PresentMon ETW access was denied.
8. Capture separately: `capture.py <ara|unity> showcase shots`, `run_unreal.py shots showcase`, and `capture.py cycles showcase shots`. Freeze timing executables first. Build Ara's separate capture player using `prepare_capture_ara.py showcase --rebuild` and Unity's with `run_unity.py package-video showcase`. Run each video's capture separately (`capture.py <ara|unity> showcase video`, `run_unreal.py video showcase`). Do not score capture timings. MRQ completed all Showcase frames but exited abnormally during shutdown; retained encodes were independently decoded and verified. Future nonzero exits must be investigated, not silently accepted.
9. Validate all three fixed 4K cameras and exactly 1,200 video frames at 60 Hz. Run `summarize.py`, `editor_summary.py`, `metrics.py`; revise findings from the new evidence, then generate `report.py`. Keep baseline comparisons separate. Publication helpers upload original 4K PNGs and compact video copies; Snist limits HTML to 2 MB and media to 25 MB. Local CQ18 4K originals remain authoritative.

All commands above are instructions for a future authorized rerun; archiving this directory did not execute them. Camera interpolation, matched exposure and the corrected Blender light basis must remain intact. Do not use the old stepped-camera calibration results as the primary comparison.

## Checks performed at handoff

Both Ara patches pass `git apply --check` against the recorded source baseline. Python files are parsed without importing engines or launching jobs; `git diff --check` verifies the archive. Engine binaries/settings were already exercised by the accepted Showcase runs. No new engine run or build was started after cancellation. Original images and videos remain in `D:\AraBench\report\`.
