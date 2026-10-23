# Ara editor v1

`ara-editor` is the native Ara UI shell and the project's in-process Engine
Protocol authority. `ara-editor-gpu` implements the shared UI instance ABI on the
renderer’s existing Vulkan device. The viewport is sampled directly; screenshots
alone read pixels back to the CPU.

From an initialized project:

```powershell
$env:CARGO_BUILD_JOBS='2'
cargo run -p ara-editor -- <project> --sample
```

`--sample` creates the four-entity Outpost graybox only if the scene is empty.
Content is always saved through the host. Docking is saved per project in
`.ara/editor-layout.json`; `editor.layout.reset` restores the default arrangement.
Use `run.ps1 -Mode Window -Project <project>` for process-scoped Vulkan setup on
this Windows workstation. SDL linking/runtime provisioning belongs to ara-platform;
the script does not assume a downloaded DLL or modify the user's PATH.

## Two-minute demo

1. Select **Salvage crate** in Hierarchy. Enter `1.5m` in position X. Multi-select
   with Ctrl/Shift; shared inspector fields show mixed values.
2. Alt-drag to orbit, middle-drag to pan, wheel to zoom, right-drag plus WASD to
   fly. Press F to frame. W/E/R select move/rotate/scale. Use World/Local,
   Pivot/Center and Snap controls. A drag commits one transaction; Escape cancels.
3. Drag a hierarchy row to reparent/reorder, use its context menu to create,
   rename, duplicate, delete or unpack a prefab. Search accepts `type:Transform`.
4. Open Ctrl+K or Ctrl+Shift+P. Search the reflected command registry and submit
   the generated parameter form. Ctrl+Z/Y undo/redo content independently of
   selection and camera history.
5. Connect `ara mcp --root <project>` (or the normal CLI) to the running
   editor. Apply an attributed batch with a `why`. Open **Agents**, inspect
   was → now, then Accept or Revert the turn. Try approval/idle apply policies.
6. Request `ara_capture` with `target: "viewport"`, `"editor"` or `"layout"`.
   Viewport captures include marks, legend, picking IDs and provenance metadata.
   Toggle Light, move dock tabs and splitters, then reset the layout.

Editor navigation is exposed as `editor.*` protocol methods and generated
`ara_session` actions, e.g. `{"action":"editor.theme","session":"…","light":true}`.
Use discovery for their exact schemas. Content actions use `ara_apply` and the
same command registry as the inspector. `.ara/daemon.json` is owned by the window
authority and removed on clean shutdown; no second daemon should own the project.

## Verification

```powershell
./crates/ara-editor/run.ps1 -Mode Check
./crates/ara-editor/run.ps1 -Mode Verify -Project <scratch-project>
```

The checks cover typed UI interactions, real WebSocket edits/HTTP MCP capture,
approval attribution, review/revert, hierarchy scale and native GPU goldens.
Verification uses the production window/event loop, compares raster/MSDF rendering
with CpuGpu, captures both semantic themes and checks Vulkan validation.
`tests/goldens` is reviewed reference imagery; ordinary tests never overwrite it.
`evidence/final` contains GPU screenshots, interaction results, CPU comparison
diffs, viewport marks and raw timings. Golden comparison tolerance is 1-SSIM ≤
0.005; GPU/CPU glyph comparison tolerance is 0.01.

Before visual polish (v0), release timings on the RTX 4070 SUPER at 1440×900 with the four-entity scene:
idle active time p50 0.016 ms / p95 0.033 ms; warm camera command through completed
presentation p50 1.37 ms / p95 1.69 ms (maximum 4.29 ms). Event-loop intervals are
p50 6.51 ms / p95 6.69 ms, supporting the 144 Hz target. These are 5,000 idle and
240 warm input samples, not a large-scene renderer benchmark. The active-time
measurements use elapsed wall time, exclude idle wait/background file scanning,
and include scheduling delays: one idle sample exceeded 2 ms (11.85 ms).
Cold startup input is recorded separately (6.37 ms). Native submissions currently
serialize on completion. The 100k hierarchy retains 134 UI nodes; scrolling to
the last row takes 0.44 ms, with no idle paint work (initial model build: 618 ms).

Windows frame pacing waits on an owned [high-resolution timer](https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-createwaitabletimerexw)
and the [window message queue](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-msgwaitformultipleobjectsex),
so input wakes the loop immediately. `evidence/timings-coarse-wait.json` preserves
the release measurement before this fix: its message timeout rounded waits to a
15.48 ms median despite only 0.027 ms of active idle work.

## Visual polish v1

The native shell uses bundled Inter and JetBrains Mono, Lucide SVGs, D18 radius
and color tokens, icon tool groups, a centered Play pill, searchable input fields,
axis chips with coalesced scrubbing, inline hierarchy rename, component fold and
options controls, asset preview tiles, selection bars and semantic change badges.
Scene and bottom tabs have active indicators. Agents turns have compact cards,
avatars, a reason, entity/field diffs and review actions. Ctrl+K opens a centered
fuzzy palette with highlighted matches, groups and right-aligned shortcuts.

Hover/press feedback and palette opening settle in 140 ms. `editor.preferences`
exposes `reduceMotion` and the `quality` toggle through the same protocol as the
native controls. Fidelity uses ara-render-fidelity on the renderer's existing
device, with bounded temporal settling. Raster stays available. The GPU draws an
antialiased, distance-faded ground grid, axis lines, silhouettes and hover IDs.
The view cube selects canonical camera orientations. No CPU viewport bridge is
introduced. Glyph atlases, Scene targets, SDL input/IME and Windows UIA geometry
all follow the selected display scale.

Reproduce the full capture and timing matrix on a scratch project:

```powershell
./crates/ara-editor/verify-polish.ps1 -Project <scratch-project>
```

Evidence is in `evidence/polish/1440` and `evidence/polish/2560`: dark/light Scene,
dark/light palette, Agents review, interaction results, raw timing samples and
CPU/GPU glyph comparisons. `evidence/polish/comparison.html` is a self-contained
before/after comparison with theme switching. Native Scene and palette goldens
are in `tests/goldens`; all four compare at 1-SSIM 0 in the reviewed run.

Release measurements on 2026-10-23, RTX 4070 SUPER, fidelity enabled:

| Capture / scale | Idle active p95 | Camera command to present p50 / p95 | Idle interval p95 |
|---|---:|---:|---:|
| 1440x900 / 1x | 0.0314 ms | 2.4883 / 3.9517 ms | 6.6320 ms |
| 2560x1440 / 2x | 0.0364 ms | 3.6986 / 3.9100 ms | 6.6208 ms |

Each size has 5,000 idle and 240 warm camera samples; both active p95 values fit
the 6.94 ms / 144 Hz budget. Measurements are elapsed wall time around active
work, excluding idle wait, and include scheduling delays. The 2x run retains one
117.84 ms idle outlier; the 1x maximum is 0.934 ms. Raw samples are preserved,
including that outlier. The 100k hierarchy retains 181 nodes: initial model build
617.8 ms, scroll to the last row 0.461 ms, with no idle paint work.

Visual review: every final Scene, palette and review capture was opened at both
sizes. The result is acceptable for v1: dense, calm, rounded, legible and consistent
with T3 Code's default design language. Review caught and corrected overlay
ordering, offscreen swatch painting, a clipped view cube/card, row icon spacing,
and DPI coordinate mismatches. The 2x capture explicitly uses `--dpi 2` so its
raster scale is reproducible independently of the current monitor setting.

## Integration boundaries

- `world::EditWorldView` isolates the temporary document projection from ara-sim.
  Provisional gizmos overlay that projection; committed transforms use `transform.set`.
- Play/Pause/Step are disabled with explanatory tooltips until the simulation
  player is connected. The Game dock is reserved for that output.
- `world/gltf.rs` is a temporary source geometry loader; ara-asset owns cooked
  meshes, texture streaming, importer jobs and thumbnails. Browser drag uses stable
  `@path#id` references (mesh IDs from import sidecars).
- The C# host can register reflected schemas and host services at the project
  boundary; this editor does not start or hotload C# itself. The inspector already
  consumes schema docs, units, ranges, defaults, enums and references.
- Windows clipboard, OS IME and AccessKit use native adapters. The automated
  route exercises UI helpers; it does not claim a manual screen-reader/IME audit.
