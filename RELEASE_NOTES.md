# Ara 0.0.1-alpha

Ara is a Rust game engine with a native editor, C# gameplay, and MCP tools for
agents working alongside people. This first public alpha ships for **Windows x64**.

## Install

1. Download `ara-hub-0.0.1-alpha-windows-x64-setup.exe` from
   [GitHub Releases](https://github.com/Snowy7/ara/releases) and install Ara Hub.
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

**Preliminary reading:** the 26 October D26 iteration report `20261026-025148`
used the `perf` profile and predates the latest renderer/startup changes.
It measured Outpost as follows; these are not the final release acceptance results.

| Preset | D26 GPU budget | GPU p50 / p95 | ꟻLIP / RMSE |
| --- | ---: | ---: | ---: |
| Low | 2 ms | 1.73 / 1.92 ms | 0.154 / 0.054 |
| Medium | 3.5 ms | 2.72 / 3.00 ms | 0.161 / 0.057 |
| High | 6 ms | 3.78 / 4.11 ms | 0.158 / 0.056 |
| Epic | 10 ms | 5.26 / 5.62 ms | 0.151 / 0.053 |

GPU readings use settled renderer windows. Quality errors are means across four
Outpost views at 1280×720 against 256-sample path-traced references; lower is better.
High's CPU work p95 was **2.16 ms**. The cold run still recorded one frame over
16 ms after the first frame; the warm run recorded none.

**Final formal D26 reading: pending release validation.**

## Known issues

- Editor frames can still exceed **16.7 ms** while content publishes during first
  imports. Warm interaction improvements do not eliminate every long frame.
- At dusk, shadowed Outpost ground is darker than the path-traced reference.
- Game preview exposure can differ from Play's early frames; particles are not
  composited into the stopped Game preview yet.
- **macOS is not shipped** in this release.
- **D27 record targets are not met yet:** whole-frame GPU targets are
  Low/Medium/High/Epic **1 / 1.75 / 3 / 5 ms** on the same reference GPU and resolution.

Import measurements are recorded in `crates/ara-importers/MEASUREMENTS.md`.
D26/D27 budgets are in `docs/decisions.md`; the current sun-shadow comparison is
`reports/vsm-shadow-proof.html`.
