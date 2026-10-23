# Ara editor v0

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

Release timings on the RTX 4070 SUPER at 1440×900 with the four-entity scene:
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
