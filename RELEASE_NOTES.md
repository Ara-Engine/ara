# Ara 0.0.2-alpha.5

Preview 5 updates Ara Engine and Ara Hub from **0.0.2-alpha.4**.

The project layout is unchanged (layout version 4): preview-4 projects open in this
engine without migration. The editor, the renderer and the licence all changed, so
read **Highlights**, **Known issues** and **Known gaps** before updating.

## Highlights

- **A new editor.** The editor window was rebuilt and the previous one deleted. One
  dock holds Hierarchy, Scene, Inspector and Assets, with Console, Problems, Agents
  and History beside them. It has a Create browser, a command palette, context menus,
  tab dragging and splitting, panels that tear out into their own OS windows,
  box selection, a Rect tool, curve editors, Mesh Paint, and docked Game, Material
  graph, VFX and Animator pages. It is the default editor on Windows, macOS and Linux;
  `--legacy-ui` no longer exists. See **Editor**.
- **Renderlab is the only renderer.** Ara's previous renderer, `ara-render`, is
  deleted on every platform and nothing falls back to it. Play, `ara capture`,
  exported games and the editor all draw with Renderlab: hardware ray tracing where
  the device has ray queries, its non-RT path elsewhere. `--renderer ara` and
  `ARA_RENDERER=ara` are refused with an error. Where Renderlab cannot start, the
  player stops with the reason and the editor shows it in the Scene panel; neither
  draws with anything else. **Renderlab takes much longer per frame than the removed
  renderer did, and does not yet draw everything it drew.** See **Known issues**,
  **Known gaps** and **Performance**.
- **The editor's Scene view draws through Renderlab.** What you edit in is what Play
  draws, with the same renderer, lighting and exposure. Click selection, selection and
  hover outlines, the grid, gizmos and icons work on it, on object ids that Renderlab
  writes with each frame, so an outline follows the object's real silhouette. Scene
  captures (`--screenshot`, `--headless`, agent captures) are read back from Renderlab.
  Only the Lit view exists; see **Known gaps**.
- **Imports are visible when a project opens.** The editor holds a card over its
  window while the project's imports settle: how many are done, the last source to
  finish and the elapsed time. Failed imports are listed with their errors, and the
  card waits for **Open project** on a failure. **Open now** appears after three
  seconds. Failed imports are also in the Problems panel. A project whose imports
  finish within a moment never shows the card.
- **Faster C# edit-to-run.** On a Mac, an added member (L2) takes 47 ms from save to
  running instead of 134 ms, and a rebuild (L3) 143 ms instead of 237 ms; launch to
  first tick fell from about 2.3 s to about 0.7 s across several changes (a
  ReadyToRun compile server, the first compile queued before the file watchers
  start, Roslyn warmed on machines with eight or more cores). Each change and its
  numbers are in its pull request. These were measured on a Mac and have not been
  repeated on Windows for this release.
- **A licence.** Ara is available under **MIT OR Apache-2.0**, at your option
  (decision D38). Vendored code keeps its own licence files.
- **Graphics settings from C#.** Scripts can read and change graphics settings, with
  user scalability groups and display controls. Renderlab keeps its own quality: the
  Low, Medium, High and Epic presets and settings such as shadow resolution do not
  change how it draws yet, although foliage density and the terrain detail distance
  still follow the setting.

## Rendering and presentation

Renderlab draws the extracted frame through a bridge in `ara-renderlab`. It traces
reflections itself, so placed `ReflectionProbe` components do not change its picture.
Automatic exposure, the colour grade, tone curves and a look's colour table follow
Ara's settings. Terrain draws up to 192 resident tiles (Outpost's 144 all draw);
entity draws merge by material past 256 groups. Each feature Renderlab does not draw
is reported once per run in the player log as `ARA_RENDERLAB_UNSUPPORTED` (a limit as
`ARA_RENDERLAB_LIMIT`), so a missing effect is named rather than silent. The player
prints one line saying which Renderlab path and adapter it uses.

**On macOS** Renderlab renders through Ara's own GPU layer (Metal) on the presenter's
device, on its hardware-ray path only, so nothing crosses between devices. The colour
grade, the colour table, volumetric fog, object ids and empty scenes work there. It is
slow: see **Performance**.

## Authoring, scripts and samples

- Compilation is deterministic: sources compile in file order, so identical
  sources produce identical images in every process.
- A live edit that touches a type with a primary constructor now rebuilds fully
  instead of applying an unsafe delta. Moving a documentation comment off a
  reflected field during a hot edit is rejected instead of bypassing the
  documentation checks.
- On macOS, editors that save by writing a temporary file and renaming it are
  noticed through directory watches; a body edit takes 25 ms from save to running
  instead of 35 ms.
- C# preparation allocates less during large edits and project preparation.
- Curves are a value type. VFX Size over Life can be a curve (existing effects
  save as before; a shaped key adds its own fields), and Animator transitions have a
  blend curve. Both are edited inline in the editor.
- Bounded editor runs write under their project, not the checkout. A bounded run that
  cannot count a frame for 60 seconds now exits with an error naming what it waited
  for, instead of waiting forever (it used to hang when the Game tab was in front).
- The Outpost sample's sun moved (10 degrees up, further west) and its dusk exposure
  is half a stop lower. Renderlab casts the dunes' shadow where the removed renderer
  did not, which had left the whole compound in shade.
- `ara capture --video` takes `--video-cut-at F` with `--video-cut-move` and
  `--video-cut-yaw`, for a clip with one camera cut. `ara capture --reference` (the
  path-traced reference of the removed renderer) no longer exists.

## Updating

Install both **Hub 0.0.2-alpha.5** and **Engine 0.0.2-alpha.5** through the Preview
channel. Projects made with preview 4 keep layout version 4 and do not need
migration. Effects and Animator controllers that use the new curve fields should be
saved and opened with this engine. Back up the project, including `.ara/`, before
updating, as for any preview.

Rebuild C# gameplay exports with the matching preview-5 SDK and player rather than
mixing an older compiled gameplay image with the new runtime.

Scripts, build presets or tools that pass `--renderer ara`, or set `ARA_RENDERER=ara`,
must stop doing so: the player refuses the value and names the ones it takes
(`renderlab`, `renderlab-rt`, `renderlab-nonrt`).

## Install and distribute

Download `ara-hub-0.0.2-alpha.5-windows-x64-setup.exe` from
[GitHub Releases](https://github.com/Ara-Engine/ara/releases), then use
**Installs → Preview** to install the matching engine. The combined
`ara-0.0.2-alpha.5-windows-x64-setup.exe` is also available. Engine versions can
live side by side.

Windows 10/11 x64, a Vulkan 1.3 GPU with a current driver, 16 GB RAM, the x64 .NET 10
SDK/runtime and Microsoft VC++ v14 Redistributable remain prerequisites. A GPU with
hardware ray queries lets Renderlab use its ray-traced path; other Vulkan devices use
its non-RT path. A device on which Renderlab cannot start cannot draw Ara's 3D at
all. NativeAOT export also needs MSVC C++ Build Tools and the Windows SDK. Installed
engine use does not require Rust or the Vulkan SDK. macOS source builds require Apple
silicon with hardware ray tracing, Metal 4 and macOS 26 or later. Linux source builds
of the editor need a Wayland session whose compositor offers subsurfaces (GNOME and
KDE do); otherwise it stops at start and points at `--headless`. Packaged installers
in this release remain Windows x64.

**Unsigned preview:** neither Windows setup has Authenticode signing. SmartScreen
may warn. Signed release manifests and download hashes protect Hub installations
and updates; they do not remove the Windows warning.

For a game export, upload only the contents of `build/` (or its package): the
game-named executable or macOS app, required data and notices. Keep sibling
`report/`, `logs/` and `symbols/` folders private. Custom runtime templates must
come from a matching, complete engine build. Exported games draw with Renderlab.

## For engine contributors

- `ara-render` is deleted (decision D40). With it went `ara-render-geo`,
  `ara-anim-capture`, `ara-pathtrace`, `ara-vfx-gpu`, `ara-landscape`, the root
  `shaders/` folder, the fidelity renderer's ray-traced view inside
  `ara-render-fidelity`, and the raster view and bootstrap renderer inside
  `ara-runtime-render`: about 1.35 million lines and 205 MB of cooked shaders that
  the installed engine no longer ships. The CPU frame records live in the new
  `ara-frame`; `ara-render-fidelity` keeps the look code (frame environment, tone
  map, exposure, presets); `ara-render2d` owns the render graph. The editor, the
  player and the presenter reach Renderlab only through `ara-renderlab`, whose
  `AGENTS.md` holds the table of what it does not yet draw.
- `ara-runtime-render`'s presenter is the GPU device, the installed
  `ExternalRenderer` and its published image. Renderlab serves more than one view
  per engine and writes a packed object-id image per view; the editor's overlays
  read it through `IdSource`.
- `ara-gpu` gained the capabilities Renderlab needs across Metal and Vulkan
  (formats, samplers, dynamic acceleration structures, barriers, compute queues
  that may share the graphics family) and enables image robustness and workgroup
  zeroing on Vulkan devices that offer them. On macOS the bridge renders through
  Renderlab's `ara-gpu` backend on the presenter's own Metal device (`bridge_metal.rs`),
  with the scene mapping shared with the Vulkan bridge.
- `ara-editor` went from 205,732 lines to well under half with the old window and
  the raster Scene view deleted. Bounded runs take `--frames N` (the window never
  shown; `--frames 1000000` is a protocol server for export), and
  `--screenshot [--frames N] [--size WxH] --output DIR` saves `editor.png` and
  `editor.layout.json`. `ARA_SCENE_DUMP=<file.png>` writes the composed Scene frame
  with its overlays, which a window capture never contains.
  `ARA_RENDERLAB_FAIL_START=1` makes Renderlab refuse to start, to see what a host
  shows. `--dpi` is not supported. Capture and agent tooling gained a `window`
  capture target and an `editor.imports` method that reports import progress and
  failures and can open the import card. The dev-only hot-patch host builds again; no
  hot-patch session has been run against the new editor.
- The release smoke installs into a short folder of fixed length and refuses, before
  installing, a package whose longest installed path would reach 260 characters.
- The release tool's notices step needs the GPL-2.0-only text for a new dependency
  with an `Apache-2.0 OR GPL-2.0-only` licence; it is now in `release/licenses/spdx`.

## Editor

- **Layout.** Tabs drag to another area or onto an edge to split; every tab has a
  menu (Add Tab, Move To, Maximize, Close); Window › Maximize (Shift+Space); layouts
  can be saved and restored. Panels torn out of the dock are real OS windows that
  reopen where you left them. Hierarchy, Inspector, Scene and Game have no tab close
  button; Window › Hierarchy, Inspector and Game show and hide them. The left column
  is narrower by default, tree rows have less left padding, and windows under 1100
  pixels wide draw the interface one zoom step larger; the title bar keeps its
  minimize, maximize and close buttons at every size.
- **Scene.** Overlays that drag to any edge and collapse, box selection, saved views,
  Drop to Surface, Pilot Selected Camera, a Rect tool (T) that works in the object's
  own frame, Mesh Paint, and a Game view Scale slider (1x to 5x). The Draw Mode menu's
  view modes and effect switches were the removed renderer's and do nothing yet.
- **Inspector.** Fields that differ across a multi-selection can be edited; prefab
  instances show their source and an Overrides menu with per-field Apply and Revert;
  a Debug mode shows the resolved record; lists edit in place. Vector fields show
  whole numbers in narrow panels, at the cost of an ellipsized long label.
- **Hierarchy and Assets.** Drag assets onto the Hierarchy to place them and onto an
  Inspector field to assign them. Hierarchy names use the full row; the indent step is
  smaller. Assets has one row of category filters, a path bar that starts at the
  project's own name, and tiles that step down to a list when the panel is too short
  to show a tile's name.
- **Title bar and status.** The title bar is one surface with no theme button (Ara Dark
  and Light are in Settings). The status bar names your platform's launcher chord
  (Ctrl K or ⌘K). The frame-time readout in the Scene corner appears only with
  View › Scene statistics on.
- **Console and profiling.** C# faults show every exception and frame with a link
  to the file and line; the Profiler has a call tree and a Memory module.

## Scripts in Play

- C# exceptions from gameplay, commands and callbacks carry the full managed
  stack, including inner exceptions and file/line where PDBs are present.
- Edits an agent makes to C# sources while Play is running are attributed to the
  agent even when the file lands late; your own saves in between stay yours.

## Known issues

- **Renderlab is slow.** See **Performance**. There is no faster renderer to switch
  to.
- **The editor waits for each Scene frame.** While the Scene redraws, the editor's
  input lag is the Scene's frame time: small on a fast GPU with a light scene, a fifth
  of a second or more on Outpost on an integrated GPU.
- **Camera cuts cost extra frames.** After a cut, one frame takes about three times
  as long on the ray-traced path, and about six frames are slow on the non-RT path
  while its lighting cache refills. The picture stays clean.
- **First frame.** Renderlab compiles its pipelines on the first frame: about 0.3 s
  on Showcase and 0.6 s on Outpost on an RTX 4070 SUPER, 1.3 s cold on a Radeon 760M.
  The pipelines are cached in the player's GPU cache folder.
- **The Game tab on Linux.** The editor's Game tab cannot show the running game on
  Linux (`ARA_GPU_EXTERNAL`: the shared surface it needs exists on Windows only). Play
  in its own window works.
- **Integrated GPUs with ray queries.** Renderlab chooses its ray-traced path
  wherever the device offers ray queries, including integrated GPUs where its non-RT
  path is faster. On a Radeon 760M, Outpost on the ray-traced path ran at about
  1.5 frames a second and lost the device when the player moved.
- **A notice on GPUs without hardware ray queries.** Where Renderlab uses its non-RT
  path, the player prints `ARA_RENDERLAB_NONRT_PATENT` (ARA6123) once at start.
- **Large levels on the non-RT path.** Foliage keeps at most two million triangles
  there, and the rest is reported (`ARA_RENDERLAB_LIMIT`), because that path's
  shadows slow sharply with caster triangles.
- **Terrain and groups.** Terrain takes at most 192 of the 256 mesh-material groups;
  entity draws and foliage share the other 64, merging by material past it, and what
  cannot merge (skinned, see-through, moving batches) is left out and reported once.
- **The new editor does not yet have** the Scene view's Collider shapes and Mesh
  bounds overlays (the toggles exist and draw nothing), a draw-call and triangle
  statistics overlay, the colour picker's eyedropper, a per-type gizmo visibility
  menu, importer cook presets, or prefab thumbnails in Assets. A `--screenshot` taken
  with the Game tab in front shows the Game area empty, because the game's picture is
  a native layer outside the interface snapshot.

## Known gaps

Things the removed renderer drew or provided and Renderlab does not yet. Each is
reported as `ARA_RENDERLAB_UNSUPPORTED` where a frame uses it; the full table is
`crates/ara-renderlab/AGENTS.md`.

- **Not drawn, or drawn differently:** fog volumes, clouds and the sky atmosphere's
  artistic factors; decal emission and ambient occlusion; ambient occlusion maps, the
  second UV set, vertex paint and material-graph materials (they draw with their base
  factors and maps); emissive maps on see-through materials; alpha masks on skinned
  meshes and transmission that varies over a surface. Rect, disc and tube lights draw
  as a wide spot or sphere with no IES profile. Every material is double-sided,
  skinning uses the four strongest influences, and textures decode to full RGBA8 mip
  chains (about five times the memory of BC7). Depth of field blurs about half as much
  as the removed renderer on the one scene compared.
- **VFX graphs:** mesh-surface particle spawns, depth collisions, ribbons, mesh
  particles, events, material flipbook atlases, smoke lighting gradients, the velocity
  shutter, and more than 2,048 particles per emitter are not drawn.
- **Graphics quality settings** (the presets, shadow resolution and the rest of the
  plan) do not reach Renderlab.
- **Debug and capture features:** the path-traced reference, buffer views
  (`ara capture --view`) and the rendering debug switches were the removed renderer's
  and are gone. The editor's captures carry entity marks and a legend again, from
  Renderlab's object ids; the player's labelled capture has no marks yet.
- **Scene view and previews:** view modes and rendering debug views in the Scene view,
  render probes, asset previews and thumbnails.
- **macOS:** on the hardware-ray path only, with no non-RT path. The backend does not
  draw particles or textured transparency; a click on glass selects what is behind it.

## Performance

Measured with an RTX 4070 SUPER at 1920×1080 in the player, GPU time per frame with a
moving camera. Showcase: about 34 ms on the ray-traced path and 35 ms on the non-RT
path. Outpost: about 38 ms on the ray-traced path and about 100 ms on the non-RT path.
In the editor, a settled Scene frame at the default panel size takes about 10 to 13 ms
on Showcase and 26 to 31 ms on Outpost. The renderer removed in this release took 2 to
3 ms on the same GPU; the previous release's frame-time table was for it and does not
describe this release.

On an Apple M5 Pro, Showcase takes about 65 ms per frame in the player and about
67 ms in the editor's Scene view.

**Not measured in this release:** a full performance table at the presets, first-frame
time with a warm and a cold pipeline cache, editor responsiveness during imports on
Windows, and cold import time.
