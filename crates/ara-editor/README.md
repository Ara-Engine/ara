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
The axis sphere and colored caps select canonical camera orientations. No CPU viewport bridge is
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

## Editor panels v1

The Assets dock has a folder tree, grid/list views, `type:` search, source-backed
references and drag targets in the scene, hierarchy and inspector. Texture
decoding, mesh cooking/rendering and material thumbnails run off the UI thread;
the project cache retains their results. Select an asset to edit reflected import
settings, reimport with cancellable job progress, or inspect uses/used-by links.
Short drawers use horizontal thumbnail tiles so filenames stay visible. The grid
reflows after initial layout, splitter changes and monitor/size changes.

Material and animation editors share a culled canvas with typed ports, validated
connections, pan/zoom, box selection, reroutes, comments, groups, a minimap and a
search-to-add palette. **Frame / F** fits the cards while leaving space for the
minimap. Fresh default views frame after layout, preserving an explicitly
positioned canvas camera. Material graph and DSL edits share command/undo history;
the generated shader renders the sphere/plane preview and reports node errors
while retaining the last good image. Animation supports states, transitions and
blend spaces. VFX uses ordered module stacks, parameter controls and a live
fixed-tick GPU preview. The Profiler shows actual CPU spans, GPU passes, counters,
hitches and comparisons of frozen `.araprofile` captures.

Try **Material**, add an `Add` node, connect two compatible sockets, then Ctrl+Z.
Switch to **VFX** to reorder a module and change a parameter; open **Animation**
to add a state or blend sample. In **Profiler**, record, freeze, set the baseline,
record another interval and compare. These actions also appear in protocol
discovery. Canvas navigation stays separate from content undo.

```powershell
./crates/ara-editor/verify-panels.ps1 -ScratchRoot D:/AraBuild/panel-check
```

This release route imports real media, drags a mesh into the scene, edits import
settings and reimports, then creates/connects/undoes graph nodes through the UI.
It captures every panel in both themes at exact physical 1440x900 (1x) and
2560x1440 (2x), and gates 1,000 warm 500-node pan/zoom samples at p95 below 4 ms.
`evidence/panels/{1440,2560}` contains images, layout reports, interaction results
and all timing samples. Verification rejects a window with the wrong extent.

Final release measurements on the RTX 4070 SUPER, **measured on a shared, loaded
host** (16 logical CPUs; other workspace builds/tests were not paused):

| Physical size / scale | 500-node pan/zoom p50 / p95 | Idle CPU p95 | Idle interval p95 |
|---|---:|---:|---:|
| 1440x900 / 1x | 1.8368 / 3.1629 ms | 0.1257 ms | 6.6801 ms |
| 2560x1440 / 2x | 2.1074 / 2.8820 ms | 0.1333 ms | 6.7015 ms |

Each size retains 1,000 active and 1,500 idle samples. The earlier 3.1002 / 3.5037
ms p95 pass is preserved alongside the 4.0371 and 11.2370 ms noisy runs, stage
timings and CPU-load observations. No outliers were removed. These are shared-host
observations, not an isolated performance guarantee; the parent will repeat the
benchmarks together on a quiet machine after integration. See
[`evidence/panels/README.md`](evidence/panels/README.md) for the full qualification,
raw evidence and verification commands.

All 36 final captures were visually inspected in dark/light at both sizes,
including full-resolution HiDPI asset captions. I would accept these panels as
v1: the layout is coherent, labels and controls are legible, and previews show
actual engine output. The compact drawer no longer hides filenames. The
[`capture gallery`](evidence/panels/gallery.html) compares every panel and theme.

## Editor UX v2, phase 1

D23 shell: the native OS frame styled from the theme (DWM dark mode plus caption,
border and text colours, switched live), and one 36 px menu bar row. The row holds
the nine menus, the project/scene breadcrumb, the Play cluster, a search box that
opens the palette, and status chips.

- **One command registry** (`shell/commands`, 272 commands). Each command has an id,
  a human title, a menu path and default shortcuts, and drives the menus, the
  palette, context menus, shortcuts and toolbar buttons. `editor.commands` exports
  it to MCP. Disabled items say why (155 enabled, 117 with a reason). Edit ▸ Undo
  names the next command. Alt/F10 reach the menus with mnemonics, and typing in
  an open menu filters it, submenus included.
- **Settings** (Ctrl+,): Preferences/Project scopes, search, a modified-only filter,
  per-setting reset, and undoable edits (`editor.settings.*`). Keyboard Shortcuts
  has Ara, Unreal, Unity and Blender presets, key recording and conflict notes.
- **Detachable panels.** Drag any tab (except Scene) out of the window, or use
  ⋯ ▸ Move Panel To ▸ Move Panel to New Window. Each floating panel is an owned OS window on
  the same GPU device, with its own swapchain. Drop targets light up across
  windows and Esc cancels. Window rects persist in the layout, and off-screen
  windows are recovered at startup. Window ▸ Layouts has six built-in layouts,
  Save Layout As… and Reset.
- **Density.** Compact is the default, with 22 px tree rows and 24 px property rows.
  The Inspector has two columns with a resizable label column, reset arrows,
  modified markers and real enum, bool and colour widgets. Selection is a solid
  `selection-surface`. The viewport gets 57% of a 2560×1369 window (39% before).
- **Panels:** 28 px headers with ⋯ menus, icon toolbars, real empty states, and
  per-row actions as hover icons. Right-click anywhere opens a registry-built
  context menu: rows, fields, tabs, the viewport and empty space.
- **Viewport:** a v2 toolbar with tools, space, pivot, snaps, camera, view mode
  and Show. The gizmo is constant-size with plane handles and hover/active
  states. A readout pill shows the value while dragging, typing a value enters it,
  and fading/hiding follows Appendix C.

Try: Ctrl+K "fra"; right-click a hierarchy row or an Inspector field; drag the
Inspector tab out of the window, then drop it back on the right dock; open Ctrl+,
and switch the keymap preset to Unreal; Window ▸ Layouts ▸ Animation.

```powershell
./crates/ara-editor/verify-ux.ps1 -Release
```

This route captures the main editor and five menus (each with a submenu open),
four context menus, the palette, Settings and Keyboard Shortcuts, a floating
Inspector (main window and its own OS window), all eight tool tabs and an empty
Inspector. Every capture is taken in dark and light at 1440×900 and 2560×1369
(1x). Output goes to `evidence/ux-v2/{1440,2560}`, with `commands.json`,
`ux-report.json` (viewport share, toolbar buttons without a command) and
`timings.json`. `tests/ux.rs` covers the registry walk, menus and undo labels,
detach/redock and layouts, settings undo and presets, empty states, context menus,
decoration paint order and icon tints.

Release timings on the RTX 4070 SUPER, on a shared host (other worktrees'
builds were not paused; CPU load was 35–62%). The full table and samples are in
[`evidence/ux-v2`](evidence/ux-v2/README.md).

| Size (1x) | Idle CPU p95 | Idle interval p95 | Warm camera → present p50 / p95 |
|---|---:|---:|---:|
| 1440×900 | 0.123 ms | 6.652 ms | 3.322 / 3.899 ms |
| 2560×1369 | 0.117 ms | 6.642 ms | 6.861 / 8.023 ms |

Judged against the UE5/Unity 6 references:

- **Close to both:** structure (one menu row, Unity-style columns), row density,
  context menus and the Settings/Shortcuts pages.
- **Ahead:** the palette shows each command's menu path and id, and disabled
  items explain why.
- **Behind:** viewport share (UE5 is about 73% with its drawer collapsed); there
  is no font weight, so headers are distinguished by size and colour only; the
  gizmo is slimmer than UE5's, with sphere tips instead of arrowheads.

Known gaps:

- Floating windows use the main window's glyph raster scale.
- Splitters inside a floating window are not persisted.
- The Scene viewport stays in the main window.
- Floating windows have no accessibility adapter yet.

## Integration boundaries

- `world::EditWorldView` isolates the temporary document projection from ara-sim.
  Provisional gizmos overlay that projection; committed transforms use `transform.set`.
- Play/Pause/Resume/Step/Stop use the registered `ara-play-host` service and a
  pooled player window. The Game dock still needs the shared GPU texture output.
- `world/gltf.rs` is a temporary source geometry loader; ara-asset owns cooked
  meshes and texture streaming. Editor import jobs use ara-asset's importer/cache
  APIs; FBX scene geometry consumes a published cooked-cache snapshot. Browser
  drag uses stable `@path#id` references (mesh IDs from import sidecars).
- The play host owns C# execution and schema services. The inspector consumes
  schema docs, units, ranges, defaults, enums and references through the host.
- Windows clipboard, OS IME and AccessKit use native adapters. The automated
  route exercises UI helpers; it does not claim a manual screen-reader/IME audit.
