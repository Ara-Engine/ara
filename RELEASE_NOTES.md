# Ara 0.0.1-alpha

Ara is a Rust game engine with a native editor, C# gameplay, and MCP tools for
agents working alongside people. This first public alpha ships for **Windows x64**.

## Install

1. Download `ara-hub-0.0.1-alpha-windows-x64-setup.exe` from
   [GitHub Releases](https://github.com/Ara-Engine/ara/releases) and install Ara Hub.
2. Open **Installs**, select **Preview**, and install engine **0.0.1-alpha**.
   Hub and engine releases are separate; the alpha is a prerelease.
3. Create a Starter, Outpost or Material showcase project from **New project**.
   Hub manages installed engines, project version pins and Hub updates.

Alternatively, `ara-0.0.1-alpha-windows-x64-setup.exe` installs the engine and Hub
together. Installations are per user and engine versions can live side by side.

Windows 10/11 x64, a Vulkan 1.3 GPU with a current driver, and 16 GB RAM are the
baseline. Install the x64 .NET 10 SDK/runtime and Microsoft VC++ v14 Redistributable.
NativeAOT builds also need MSVC C++ Build Tools and the Windows SDK. Shader tools
are bundled; running the installed engine does not require Rust or the Vulkan SDK.

**Unsigned alpha:** neither setup has Authenticode signing. Windows SmartScreen
may warn; check the download source before choosing **More info → Run anyway**.
Signed release manifests and SHA-256 verification protect Hub downloads and updates;
they do not remove the Windows warning.

## Highlights

- **Raster rendering on every preset:** probe global illumination, virtual sun
  shadow maps on High/Epic, heightfield horizon shadows for distant terrain, and
  temporal antialiasing/upscaling (TAAU). Hardware ray tracing remains experimental.
- **Finer probe GI at no frame cost:** GI bakes a probe pool, the scene-wide lattice
  plus a finer one over built-up content (Outpost: 32,768 + 16,250 probes), blended
  by coverage without seams. Scenes are ready on the coarse lattice; the fine one
  cross-fades in when its bake lands. The relight touches only its per-frame budget,
  so its cost no longer grows with probe count (Outpost: 0.12–0.22 ms down to
  0.02–0.09 ms). Outpost's ꟻLIP against the path tracer improves on every preset.
- **Faster imports:** Outpost's 115 sources reached assets-ready in about
  **20 seconds** on NVMe (19.83–20.10 s). These were fresh project content caches
  through the real import/publication path; OS and driver caches were not flushed.
  This measures import completion, not the time to the first fully rendered scene.
- **A more responsive editor:** retained UI, asynchronous Scene presentation,
  responsive menus and text editing, Scene/Game views, embedded Play, undoable
  authoring, and material, VFX and animation tools.
- **C# gameplay and shipping:** live code reload with state migration, in-editor
  Build Current Profile, and standalone Windows exports with NativeAOT gameplay.
  Installed-package smoke builds Starter and verifies an exported game's run.
- **MCP agents:** one global connection can discover registered projects and
  inspect, edit, play and capture them through the same engine commands as the
  editor. Sessions stay isolated between projects. Hub provides setup instructions
  with an absolute executable path; Ara does not need to be on the client's PATH.

## Performance bar: D26

D26 sets UE5-class budgets at **2560×1440 on an RTX 4070 SUPER**, with release
builds and validation disabled. Quality is checked against the path-traced reference.
The GPU budgets are Low **2 ms**, Medium **3.5 ms**, High **6 ms**, Epic **10 ms**;
the CPU budget is **4 ms**, with no frame over **16 ms** after the first frame.
Startup targets are a presented loading frame within **2 s** cold and a complete
first scene frame within **3 s** warm.

**Formal reading:** report `20261026-182821` on the release source, `perf` profile.
Every D26 row is within its budget:

| GPU p50 / p95 (ms) | Low (2) | Medium (3.5) | High (6) | Epic (10) |
| --- | ---: | ---: | ---: | ---: |
| Outpost | 1.52 / 1.64 | 3.04 / 3.26 | 3.81 / 4.28 | 5.39 / 5.79 |
| Hangar | 1.26 / 1.52 | 1.88 / 1.98 | 3.14 / 3.48 | 3.96 / 4.21 |
| Showcase | 0.83 / 1.10 | 1.26 / 1.43 | 1.77 / 1.95 | 2.69 / 3.07 |

- **CPU:** Play CPU work p95 is 2.41 ms (Outpost), 1.51 ms (Hangar) and 1.72 ms
  (Showcase); the editor's Scene orbit is 2.07 ms.
- **Frames over 16 ms after the first:** none, cold or warm, in every Play run.
- **Startup:** the player's cold loading frame takes 0.72–0.99 s and its warm first
  scene 1.24–1.91 s. The editor's cold loading frame takes 1.90 s and its warm
  first scene 1.99 s.
- **Quality:** Outpost ꟻLIP / RMSE against 256-sample path-traced references is
  0.129 / 0.043 (Low), 0.130 / 0.043 (Medium), 0.128 / 0.041 (High) and
  0.125 / 0.040 (Epic); lower is better.

GPU readings use settled renderer windows after readiness. The run's foreign-GPU-load
guard recorded no outside load (peak 4.9%, mean 0.2%, no contaminating process).
Its counter sampler then failed with a transient read error in the run's final
seconds, so the tool labels the report invalid. The values were accepted as the
formal reading; the sampler fix ships in the next build.

## Known issues

- The editor still exceeds **16.7 ms** on some frames while content publishes
  during first imports: 40 frames in the formal cold run, with click and drag
  latency p95 of 20.7 ms and 72.6 ms. After imports, 5 frames exceed it.
  A renderer descriptor-heap fix for this is ready for the next build.
- At dusk, shadowed Outpost ground is darker than the path-traced reference.
- Game preview exposure can differ from Play's early frames; particles are not
  composited into the stopped Game preview yet.
- **macOS is not shipped** in this release.
- **D27 record targets are not met yet:** whole-frame GPU targets are
  Low/Medium/High/Epic **1 / 1.75 / 3 / 5 ms** on the same reference GPU and resolution.

Import measurements are recorded in `crates/ara-importers/MEASUREMENTS.md`.
D26/D27 budgets are in `docs/decisions.md`; the current sun-shadow comparison is
`reports/vsm-shadow-proof.html`.
