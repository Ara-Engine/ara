# Ara 0.0.2-alpha.4

Preview 4 updates Ara Engine and Ara Hub from **0.0.2-alpha.3**.

Existing projects need the migration described here before opening in this engine.

## Highlights

- **One place for your game content.** New projects and templates use `content/`
  for scenes, scripts, models, materials, audio and UI, organized however you like.
  Hub and the editor show a move preview before migrating an existing project;
  **Cancel** leaves it unchanged and closed.
- **A rendering and material update.** High/Epic use Ara TSR for the main view;
  lower presets and secondary views retain TAAU. Rough reflections, grazing-angle
  surfaces, ambient occlusion, glass refraction and translucent sun shadows receive
  shading fixes. AgX display transforms and anisotropic material filtering are
  available. High/Epic can use hardware ray queries on capable devices, with a
  raster fallback; the separate ray-traced fidelity tier remains opt-in.
  Unsupported RT requests report `ARA_GPU_RT_FALLBACK` (ARA6008) instead of
  falling back silently.
- **Undoable asset organization.** Import, move, rename, duplicate, safe delete
  and folder creation use attributed transactions. Recognized references and model
  companions are handled together; collisions and unsafe dependency changes are
  diagnosed before committing. An asset batch is one Undo/Redo step, including
  file bytes and empty folders.
- **CLI and MCP workflows that survive reopening.** Offline Undo/Redo now retains
  history between CLI invocations. Entity-only revert selects the caller's eligible
  recent edits without requiring a transaction ID, and accept-all is idempotent.
  `ara why` understands primitive and asset-backed geometry and resolves the new
  content paths. Headless run and offscreen PNG capture work through CLI and MCP,
  alongside the registered Play and test services.
- **C# authoring and editor extensions.** Find entities by name or path, query and
  change hierarchy, and receive deterministic destruction callbacks. Editor-only
  extensions can register commands and menu items, read selection and scene
  snapshots, and submit undoable authoring commands. The SDK ships analyzer fixes
  for ARA0001, ARA0101 and ARA0506; CLI/MCP fixes use undoable source edits and reject
  files that changed underneath them.
- **Hub installation and export workflows.** Updates separate discovery, download,
  verification and explicit restart. Engine installs show their phases; signed
  Repair and cancellable removal are available. Exports produce a game-named
  executable or app with its runtime data, keeping reports, logs and symbols out
  of the player upload folder.
- **Apple silicon source-build support.** Metal 4 rendering, C# playback,
  Retina-aware game UI and macOS app exports are supported. On Apple9 GPUs (M3 and
  later) High/Epic use hardware ray queries by default (`ARA_METAL_RT=0` keeps the
  raster path), virtualized geometry runs, and texture imports use the GPU BC4/BC5/
  BC6H/BC7 encoder. The emitter-related bright speckles in Metal RT captures are
  fixed. Native crashes and panics produce macOS crash reports, and a player killed
  by a signal is reported as a crash. Packaged installers in this release remain
  Windows x64.
- **Reflection probes and lighting debug views.** Place a `ReflectionProbe` to
  capture a room for nearby smooth surfaces. Captures run as cancellable jobs with
  receipts, are cached under `.ara/cache/probes` and restore when the project
  reopens; edits mark them stale instead of changing under you. New **GI diffuse**
  and **Overdraw** views show indirect diffuse light only and per-pixel fragment
  counts on a fixed heat legend.

## Rendering and presentation

Player and editor presentation waits are bounded, and rendering can record the
next frame while the previous one runs. Native geometry uses indexed draws.
Imported glTF IOR and volume settings now reach glass shading, including
refraction and translucent sun shadows.

Sunlight bounced into shadow is restored. Probe GI no longer shadows the surfaces
it was sampled from: in a sun-only bench, shadowed bounce light went from 0.55 to
0.92 of a path-traced reference (0.99 relative to the same scene's sunlit areas),
and thin walls do not leak light. In the opt-in RT tier, rough metals keep their
reflected energy through denoising, so dark panels such as Outpost's shutters read
at the reference brightness again.

## Authoring, scripts and samples

- C# modules use `<name>.module.json` manifests with named dependencies. Nested
  modules compile their own sources; unknown dependencies and cycles are errors.
  An exact lowercase `editor` folder anywhere in a source's ancestry makes it
  editor-only, and those assemblies are excluded from player exports.
- Generated projects, assemblies and component schemas live under `.ara/scripts/`.
  Opening, checking or exporting a fresh project compiles and publishes its C#
  schemas before scene validation, without requiring Play first. Compilation
  failures remain visible instead of becoming unknown-component errors.
- C# diagnostics retain module and source locations, including errors from initial
  script loading and Play startup.
- Saving canonical defaults no longer creates a spurious external-edit transaction
  that clears Redo after an asset Undo. Real external edits are still recorded.
  CLI asset commands wait for prepared work to commit; progressing imports are no
  longer stopped solely because a fixed overall timeout elapsed.
- Outpost's five Kenney packs now have GitHub mirror URLs. Fetch tries the original
  URL and then its mirrors, requiring the same pinned BLAKE3 and size throughout.
  Total failures list every attempted URL and its failure. Credits are unchanged.
- Export preflight identifies missing or mixed runtime resources before compiling
  gameplay and offers installed-engine Repair. VFX exports include the cooked
  effect records and particle atlas required to run outside the checkout.
- Shared menus, searchable selections and dialogs have corrected sizing, focus
  and text wrapping. Hub project creation is a dedicated page with Back navigation.
  Editor interaction changes are listed under **Editor** below.
- Crash monitoring handles long Windows temporary paths that previously prevented
  its startup.

## Updating and migrating a project

Install both **Hub 0.0.2-alpha.4** and **Engine 0.0.2-alpha.4** through the Preview
channel. A preview-3 layout-v3 project can keep using its compatible old engine
until you choose to migrate it.

This engine requires **layout version 4**. Earlier root-level folders and the
preview-3 `game/` layout are migrated into `content/`. Project policy moves into
`settings/project.json`; build presets live in `settings/builds/`. Generated data
stays in `.ara/`, export output in root `builds/`, and root `packages/` is reserved.
References and the main scene are relative to `content/`, without that prefix:
`@props/crate.prefab#pf_crate001` addresses `content/props/crate.prefab`.

Back up the project, including its `.ara/` history. In Hub or the editor, review
the move preview and choose **Migrate** or **Cancel**. Migration is explicit:
Cancel keeps the project closed and unchanged. Destination collisions are reported
before files move. Migration preserves IDs and history, converts legacy C# module
manifests and editor-only placement, and pins the engine performing the migration.
The move is one undoable transaction; undoing it restores the older layout, which
must be migrated again before ordinary work in the new engine.

For the CLI, close the project's editor and daemon first:

```sh
ara --root path/to/project project migrate --dry-run --json
ara --root path/to/project project migrate
ara --root path/to/project check
```

The dry run does not create a lock or cache. **ARA1914** means the old layout needs
migration before open, check, import, Play or export. **ARA1915** means the pinned
engine cannot open the new layout; Hub offers **Use current engine** or
**Install current engine**, with **Cancel** to leave the pin and project unchanged.
If migration reports unsaved journal edits (ARA1905), recover and save them with
the originating engine before retrying. Do not rename the root folders by hand.

See the [project layout and migration guide](https://github.com/Ara-Engine/ara/blob/main/docs/project-layout.md).
Rebuild C# gameplay exports with the matching preview-4 SDK and player rather than
mixing an older compiled gameplay image with the new runtime.

## Install and distribute

Download `ara-hub-0.0.2-alpha.4-windows-x64-setup.exe` from
[GitHub Releases](https://github.com/Ara-Engine/ara/releases), then use
**Installs → Preview** to install the matching engine. The combined
`ara-0.0.2-alpha.4-windows-x64-setup.exe` is also available. Engine versions can
live side by side.

Windows 10/11 x64, a Vulkan 1.3 GPU with a current driver, 16 GB RAM, the x64 .NET 10
SDK/runtime and Microsoft VC++ v14 Redistributable remain prerequisites. NativeAOT
export also needs MSVC C++ Build Tools and the Windows SDK. Installed engine use
does not require Rust or the Vulkan SDK. macOS source builds require Apple silicon,
Metal 4 and macOS 26 or later; macOS app exports are built on a Mac.

**Unsigned preview:** neither Windows setup has Authenticode signing. SmartScreen
may warn. Signed release manifests and download hashes protect Hub installations
and updates; they do not remove the Windows warning.

For a game export, upload only the contents of `build/` (or its package): the
game-named executable or macOS app, required data and notices. Keep sibling
`report/`, `logs/` and `symbols/` folders private. Custom runtime templates must
come from a matching, complete engine build.

## For engine contributors

Source builds include Fidelity Lab for warm scene tracks, shader updates,
captures, reference comparisons and GPU measurements, with a coordinated Mac
mirror. Dev-only Rust body hot-patching covers render passes, ECS systems and
editor panels; incompatible changes require restart. These development tools do
not add hot-patching to shipped games. Showcase comparison tooling includes
adapters for Unity URP, Unreal 5.7 and Cycles; this draft makes no cross-engine
performance claim.

## Editor

- **Drag and drop shows its result before you release.** Hierarchy reorder and
  reparent, asset references, material drops onto sub-meshes and component
  reorder draw an insertion line and a short label naming the action. Invalid
  drops, such as moving an entity inside its own descendants, are refused with
  the reason. A drag is one Undo step, and Escape cancels it.
- **Files dragged in from Explorer import where you drop them.** A banner reports
  how many were created, replaced or skipped. Focus loss, Escape or closing a
  floating panel mid-drag no longer loses the import.
- Drag labels, banners, tooltips and popovers size to their text and wrap at a
  readable width; an automated check fails the build if any overflows.
- After you commit a text field, Ctrl+Z/Ctrl+Y undo and redo the document edit
  instead of being swallowed by the field.
- Renaming an asset, then undoing and redoing, keeps the project model, the file
  and its references in step.
- The editor uses the new engine icon.

A larger editor redesign is planned separately; this preview includes the
interaction fixes above, not new editor panels.

## Scripts in Play

- C# exceptions from gameplay, commands and callbacks carry the full managed
  stack, including inner exceptions and file/line where PDBs are present.
- Edits an agent makes to C# sources while Play is running are attributed to the
  agent even when the file lands late; your own saves in between stay yours.

## Known issues

- **Opt-in RT tier:** glossy surfaces can show grain. This is still present on
  both Vulkan and Metal.
- Automatic placement of reflection captures remains experimental and off by
  default (`ARA_REFLECTION_CAPTURES=1` enables it for look development).
  Probes you place yourself always capture at High and Epic.

## Performance

Measured on this release's source (`d78e59368`) on Windows 11 with an RTX 4070 SUPER
and a Ryzen 7 7800X3D at 2560×1440, using an optimized build without LTO. Every run
held the GPU lease with no foreign GPU or CPU load and passed its pass-coverage check;
High and Epic use the hybrid ray-query path, Low and Medium the raster path.

**Showcase player, D26 protocol** (600 frames after readiness; GPU frame time):

| Preset | p50 | p95 | Max | D26 limit |
|---|---|---|---|---|
| Low | 1.06 ms | 1.51 ms | 1.58 ms | 2.0 ms |
| Medium | 1.27 ms | 1.87 ms | 2.14 ms | 3.5 ms |
| High | 2.58 ms | 3.70 ms | 3.86 ms | 6.0 ms |
| High, cold start | 2.67 ms | 3.26 ms | 3.75 ms | 6.0 ms |
| Epic | 3.59 ms | 4.06 ms | 4.42 ms | 10.0 ms |

CPU work stays at or below 1.12 ms p50 and 2.19 ms p95 in every row. Medium had
two frames over 16 ms (the longest 17.2 ms); no other row had any. These Showcase
rows ran the release player against packs frozen on 2026-10-27, with its main-thread
stack reserve raised to the 8 MB this release ships.

**Showcase materials shot (D29):** High 2.30 ms p50 / 2.36 ms p95; Epic 3.48 ms /
3.61 ms. The sun-bounce correction has no measurable cost, one authored reflection
probe adds about 0.05 ms at High, and the RT-tier bloom change is cost-neutral.

**Editor:** with a warm cache, the first frame appears 1.31 s after launch and the
first scene is fully drawn after 2.38 s.

**Not measured in this release:** Outpost and Hangar frame times, editor
responsiveness during imports and cold import time. Freshly exported performance
packs now embed the C# NativeAOT image, which the timing player cannot run yet, and
older packs carry the previous script ABI that this player correctly refuses.
