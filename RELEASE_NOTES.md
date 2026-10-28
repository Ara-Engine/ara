# Ara 0.0.2-alpha.4

Preview 4 updates Ara Engine and Ara Hub from **0.0.2-alpha.3**.

**Draft:** J7, the sun-bounce fix, the editor follow-up and formal measurements
are still pending below. Existing projects need the migration described here
before opening in this engine.

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
  Retina-aware game UI and macOS app exports are supported. Hardware ray queries
  are verified on the M5 Pro; the emitter-related bright speckles in Metal RT VFX
  captures are fixed. Native crashes and panics produce macOS crash reports.
  Packaged installers in this release remain Windows x64.

## Rendering and presentation

Player and editor presentation waits are bounded, and rendering can record the
next frame while the previous one runs. Native geometry uses indexed draws.
Formal frame-time results are pending below. Imported glTF IOR and volume settings
now reach glass shading, including refraction and translucent sun shadows.

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
  script loading and Play startup. The editor's Problems/Console presentation is
  tracked in the pending editor section below.
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
  Further editor changes are pending below.
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

## Known issues

- **Opt-in RT tier:** glossy surfaces can show grain. This is still present on
  both Vulkan and Metal; the Metal emitter-speckle fix does not remove it.
- **Two-sided panels with RT:** rendering can be incorrect. Use the raster path
  if it affects your scene. **TODO before publication:** retain this issue unless
  the specific RT panel fix has merged and passed its regression check.
- Local reflection-capture bakes remain experimental and disabled by default;
  they can be enabled explicitly for look development.

## TODO — J7

Pending `t3/vq-j7`. Replace this section with the verified user-visible change and
remaining limitations after its merge and checks. No outcome is claimed yet.

## TODO — sun-bounce fix

Pending `t3/vq-sunbounce`. Add the confirmed lighting correction and any remaining
shadowed-ground limitation after its merge and reference comparison.

## TODO — editor follow-up

Pending `t3/ux-editor-p4`. Add only its final merged interaction changes and native
journey results. In particular, confirm how resolved C# errors leave Problems
while Console retains history, using the included compiler-completion support.

## TODO — formal measurements

Insert the final-source performance and quality reading after all rendering and
editor changes land. Include per-scene/per-preset GPU p50/p95, CPU work and frame
outliers, cold/warm startup, editor interaction during imports, import completion,
and reference-image error. Identify hardware, resolution, render path, build and
validation settings. Do not reuse alpha.3's table or intermediate branch speedups;
no preview-4 budget pass or percentage improvement is claimed in this draft.
