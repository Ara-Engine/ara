# Known issues: v0.0.1-alpha

This is an honest list of what doesn't work yet, or doesn't meet its own target. Ara follows [no scope cuts](decisions.md) (D7): everything here is **build order**, not dropped scope. The locked decisions are in [`decisions.md`](decisions.md); the build order is in [`plan/milestones.md`](plan/milestones.md).

## Platforms

| Platform | State |
|---|---|
| **Windows 10/11 x64 + Vulkan 1.3 GPU** | **Supported for the alpha.** Developed and measured on an RTX 4070 SUPER. Other GPU vendors are untested. |
| Linux | The workspace compiles (`cargo check`), but the native window, audio and editor have never been run on Linux. The Linux editor is an M3 item. |
| macOS / iOS | Not available. The Metal 4 backend (`ara-gpu`) is a stub. The Metal half of the S1 spike is paused, and Apple builds of SDL are still a TODO (D15). |
| Android | NativeAOT game export builds APK/AAB files that run on an API 37 emulator. Real arm64 devices are untested, and there is no editor on Android. |

## Install and Hub

- **Ara Hub and the global `ara mcp` (D25) are not built yet.** They are planned for 0.0.1-alpha. Until they land:
  - you build from source;
  - you run `ara init` per project;
  - you point agents at the per-project `.mcp.json`.
- **There is no engine installer.** `ara-export-installer` builds self-extracting installers for *exported games*, not for the engine.
- **The `ara-player` executable carries the Ara icon.** Exported games inherit it until per-game icons land in export presets.

## Editor

- **Play opens a separate player window.** The in-editor Game viewport (a shared GPU texture, D19) is not wired yet.
- **Floating panels have limits:**
  - the Scene viewport stays in the main window;
  - splitter positions inside floating windows aren't saved;
  - floating windows have no accessibility tree yet.
- **OS IME and screen readers** have not been verified by hand. The AccessKit tree exists.
- **The menu-bar brand tile still shows a text "a"** instead of the [Ara mark](design/brand.md).
- **The default layout gives the viewport 57% of the window** (2560×1369). UE5 gives about 73%. Density work continues in Editor UX v2.

## Agents and MCP

- **Some MCP tools are placeholders:** `ara_run`, `ara_capture`, `ara_play` and `ara_test` return `ARA1806`.
- **Some CLI commands are out of date:** `ara review`, `ara accept` and `ara revert` still exist from the pre-D24 review flow. Under D24, agent changes simply apply, with undo per agent turn.

## Rendering and fidelity

These measurements are on a shared RTX 4070 SUPER; the source files are linked.

- **No scene passes "UE5 polish" in the renderer's own verdict.** The numeric error gates pass, but they are not visual parity ([`FIDELITY_ROUND3.md`](../crates/ara-pathtrace/FIDELITY_ROUND3.md)).
- **The night-interior timing gate fails.** Its upscaled p95 is +1.29 ms over budget, against a +1 ms limit.
- **The 1000-light stress scene** has 17.4% surface error. It is a stress test, not a quality claim.
- **Virtual geometry misses its 4 ms target.** The photogrammetry field (1.09 B triangles) takes 4.6–5.0 ms at native 1440p. The cold-cache first frame takes 97–126 ms, and only simple preview lighting is used.
- **HDR output has never been checked on a real HDR display.**
- **Windowed runtime rendering has gaps:** foliage, particles and 2D show up only in offline captures so far, not in the windowed runtime renderer ([`ara-runtime-render`](../crates/ara-runtime-render/AGENTS.md)).
- **VFX** is not yet at Niagara showcase quality ([`ara-vfx-gpu`](../crates/ara-vfx-gpu/README.md)).

## Simulation and physics

- **Physics misses all three of its Jolt targets** ([`benches/RESULTS.md`](../crates/ara-physics-jolt/benches/RESULTS.md)):

| Workload | Measured p95 | Target |
|---|---|---|
| 5k-body bowl | 6.0 ms | 4 ms |
| 20k-body bowl | 30 ms | 15 ms |
| 1,000 characters + props | 7.0 ms | 3 ms |

- **Boids are still slower than Unity DOTS:** 3.5% slower at 50k and 28% slower at 20k. Transforms, particles and churn are faster ([`ara-sim/BENCH.md`](../crates/ara-sim/BENCH.md)).
- **Some guarantees are not yet established:**
  - netcode authority migration (S4);
  - physics hash equality across platforms (S6).
- **Full-HRTF audio does not scale yet.** All 1,000 voices on HRTF takes 4.67 ms per 2.67 ms block, so the default caps real HRTF voices at 128.

## Content

- **The samples are gray-box.** Proper-art *Outpost* scenes are in progress.
- **The Valley integration level** proves the whole stack runs: terrain, foliage, streaming, networking and C#. Its art is placeholder.

## Project

- **No license yet:** all rights reserved, and the repository is private (D14).
- **No CI (D9).** Gates run locally.
- **Every performance number is from one shared development machine:**
  - Ryzen 7 7800X3D;
  - RTX 4070 SUPER;
  - Windows 11;
  - other builds running at the same time.

  Treat them as upper bounds, not quiet-machine results.
