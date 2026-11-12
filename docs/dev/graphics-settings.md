# Runtime graphics settings

`Ara.GraphicsSettings` is the local user's options-menu API. Preferences belong to
the player's host, not to gameplay components, project documents, replication,
replays or simulation snapshots. Changing them does not change the state hash.

The design uses staged preferences and independent scalability groups, studied
from [Unreal's UGameUserSettings](https://dev.epicgames.com/documentation/unreal-engine/API/Runtime/Engine/UGameUserSettings)
and [Scalability](https://dev.epicgames.com/documentation/en-us/unreal-engine/scalability-reference-for-unreal-engine),
and separates render resolution from display modes as in
[Unity QualitySettings](https://docs.unity3d.com/6000.0/Documentation/ScriptReference/QualitySettings.html)
and [Screen.SetResolution](https://docs.unity3d.com/6000.0/Documentation/ScriptReference/Screen.SetResolution.html).
The implementation is original Ara code.

## Game menu API

Use the API in an exclusive script callback (behaviours and commands). Parallel
ECS systems cannot access it. Do not base authoritative gameplay on user preferences.
All public API members have XML documentation, and game modules need no filesystem
or reflection permissions. Serialization uses an AOT-safe generated JSON context.

```csharp
var candidate = GraphicsSettings.Pending;
GraphicsSettings.Pending = candidate with
{
    Preset = GraphicsPreset.High,
    Groups = candidate.Groups with { Shadows = GraphicsTier.Medium },
    ResolutionScale = 0.75f,
    MotionBlur = false,
    FrameRateCap = 144
};
var result = GraphicsSettings.Apply();
if (result.RestartRequired) { /* show the game's restart notice */ }
GraphicsSettings.Save();
```

`Query()` returns immutable snapshots of Pending, Applied, Defaults, Capabilities,
Revision, ActiveRayTracing, RestartRequired and diagnostics. Each group is nullable:
null inherits the overall preset; selecting a preset retains explicit overrides.
Use one snapshot for the menu's widgets. Capability queries coalesce until the next
native frame boundary, including monitor disconnects; managed transfer buffers are pooled.

- `Apply()` validates/clamps Pending and accepts it for the next presentation
  boundary. Unavailable capabilities throw and leave the last accepted settings
  intact. The completed GPU image remains available during a content replacement.
- `Revert()` applies the preceding accepted configuration. A menu can Apply a
  display mode, offer a countdown, then Revert when the user cancels.
- `ResetToDefaults()` stages the project's defaults. Apply and Save stay explicit.
- `Save()` atomically persists **Applied**, including the next-start RT preference.
  Unsaved candidates are never written. IO failures throw with ARA3501.
- `AutoDetect()` stages Auto and clears group, render-scale and upscaler overrides.
  It leaves explicit feature switches/display preferences intact and never enables
  RT. Auto uses the renderer's device-memory policy: Low below 6 GiB or unknown,
  Medium from 6 GiB, High from 8 GiB. Epic requires an explicit selection. This
  entry is device detection, not a claim to have measured a benchmark.
- `Applied` fires for accepted Apply/Revert revisions, including protocol changes
  observed at a subsequent script callback. This means configuration acceptance;
  GPU publication follows at the frame boundary. Subscribe in a live callback and
  unsubscribe in OnDestroy. The host also removes a replaced module's delegates
  on L3 and clears all delegates at shutdown, so subscriptions do not pin old ALCs.
  Register subscriptions in OnReload as well as OnSpawn.

## Mapping and application boundaries

Renderlab draws Ara, and the user's graphics tier reaches it as two numbers: the foliage
density (`ExternalView::foliage_percent`) and the terrain detail distance
(`terrain_detail`). The other preferences are validated, saved and reported by
`play.graphics` like all of them, but they do not change Renderlab's picture yet (the
"Graphics quality settings" row in `crates/ara-renderlab/AGENTS.md` says what that takes).
The display, vsync and frame-cap preferences are the platform's and apply as written.

| Preference | What it sets | Applied today | Boundary |
|---|---|---|---|
| Overall preset | `presets::Plan` and `RasterPlan`: the tiers below derive from it | Through foliage and terrain detail | Next frame |
| View distance | `Quality.terrain_detail` (metres where terrain layers use their detail maps) | Terrain detail distance only | Next frame |
| Shadows | Cascades, local shadows, VSM and horizon map budgets | Saved only | Next frame |
| GI | Probe-relight budget | Saved only | Next frame |
| Reflections | Reflection trace and capture budgets, hybrid query cuts | Saved only | Next frame |
| Post-processing | Bloom, ambient occlusion, depth of field and motion blur budgets | Saved only | Next frame |
| Individual post switches | Mask their group budget; authored volumes still supply lens/exposure parameters | Saved only | Next frame |
| Textures | Compressed texture budget: 256/512/1024/2048 MiB, or explicit cap; bounded to half known VRAM | Saved only | Next native content publication |
| Effects | Emissive-light and volumetric fog budgets | Saved only | Next frame |
| Foliage | Stable 25/50/75/100% subset by immutable instance ID; all parts of an instance stay together | **Yes** | Next native content publication |
| Anti-aliasing | TAAU or TSR group choice, explicit override | Saved only | Next frame |
| Resolution scale | 0.1..1 of physical output dimensions; preset scale when null; UI stays at display resolution | Saved only | Next frame |
| Window mode/resolution/monitor/refresh | SDL windowed, borderless or exclusive display mode; nearest supported exclusive mode | **Yes** (platform) | Native frame boundary |
| Vsync | FIFO, or mailbox/immediate with the existing platform fallback | **Yes** (platform) | Swapchain boundary |
| Frame cap | Presentation cadence only; embedded cadence also respects the owner's cap | **Yes** (platform) | Next frame |
| Hardware RT opt-in | Stored preference vs the immutable startup opt-in; never enabled by Auto/preset. Renderlab picks its own path (`--renderer`) | Saved only | **Save and restart/device reopen** |
| HDR | Capability-gated request | **Unavailable in this player's current SDR scene/UI output path** | n/a |

Texture and foliage changes may rebuild derived content but never rescatter authored
foliage or alter collision/gameplay.

`Capabilities.DeviceKnown` is false before the first GPU is opened in a purely
headless player. Querying preferences does not allocate a GPU. Rendering a capture
populates its physical support; ordinary windowed/embedded games already own a GPU.
`RayTracingSupported` reports physical support independently of logical enablement.

`Capabilities.Modes` lists monitor IDs, physical extents and refresh rates in
millihertz. A monitor ID is not an array index and may disappear on disconnect.
Headless/embedded players report no mutable display and retain their owner's
surface; saved display preferences apply when launched with a native window.
Hidden verification windows never move, resize, or switch desktop display modes.
Native display failures restore the preceding preferences and report ARA3500.
Borderless uses the monitor's desktop extent and refresh rate. Width/height apply
to windowed/exclusive modes; refresh selection applies only to exclusive mode.

The platform can create HDR swapchains, but the player's scene/UI compositor still
produces SDR. It therefore reports `HdrSupported = false` and rejects HDR Apply
instead of presenting incorrectly encoded pixels. No saved flag can force it on.

## Persistence and precedence

The file is `<engine user config>/games/<hash of main scene asset ID>/graphics.json`.
`ARA_CONFIG_DIR` overrides the entire root. Otherwise the root is `%APPDATA%/Ara`
on Windows, `~/Library/Application Support/Ara` on macOS, or `$XDG_CONFIG_HOME/ara`
(`~/.config/ara` fallback) on Linux. The opaque identity prevents path traversal
and remains valid when an exported game moves. Different main-scene asset IDs
currently have separate preference profiles.

Version 1 stores `{ "version": 1, "settings": ... }` using the canonical writer.
The reader rejects duplicate keys, unknown enum/field values, malformed UTF-8,
oversized files (64 KiB) and unsupported versions. Missing files use project
defaults; bad files diagnose and retain defaults. Future files are preserved and
Save is refused for a host that loaded one. Writes sync a temporary file in the
same directory, then atomically replace the destination; failed writes retain the
last good file. Concurrent players use last-successful-save semantics.

Project `rendering.quality` and `rayTracing` in `settings/project.json` (legacy
`ara.project.json` fallback) seed defaults. Saved values
override them; per-launch flags override the saved values without automatically
persisting. Numeric validation clamps resolution to 320..8192 by 180..8192, FPS to
15..1000 (zero uncaps), scale to 0.1..1 and explicit texture budget to 128..32768 MiB.
Nonfinite native scale restores the preset. ARA3500 describes normalization or
unsupported capabilities; ARA3501 describes storage/version errors.

## Agents, CLI and player flags

Engine Protocol: `play.graphics {session, world?, player?, request?}`. No request
means `{operation:"get"}`. Requests use the same typed service as C#:
`set` with a complete `settings` candidate, `apply`, `revert`, `resetToDefaults`,
`save`, `autoDetect`. Changes target one process, never every network player.
MCP exposes it as `ara_play` action `graphics`. Responses also report actual
renderer shadow/GI/LOD/bloom budgets after a rendered frame, for agent verification.

```sh
ara graphics get --world play:1 --json
ara graphics set --world play:1 --settings @candidate.json
ara graphics apply --world play:1
ara graphics revert --world play:1
ara graphics save --world play:1
```

Player launch overrides: `--quality`, `--ray-tracing[=true|false]`, `--size WxH`,
`--graphics-settings FILE` (complete candidate, no version wrapper),
`--resolution-scale`, `--frame-rate-cap`, `--vsync[=true|false]`, `--window-mode
windowed|borderless|fullscreen` and `--monitor ID`. Existing `--windowed` is retained.

Focused evidence: `ara-graphics-settings` tests mapping-independent transactions,
validation and strict/atomic storage; `ara-runtime-render::graphics` tests concrete
quality budgets; `dotnet/tests/Ara.Runtime.Tests/GraphicsSettingsTests.cs` checks
the typed native contract and host isolation; `ara-player --test graphics` runs a
real C# command, checks the renderer's `Quality`, unchanged simulation hash and
save/reload in a fresh headless child. All captures/profiles belong under `target/`.

`cargo run -p ara-runtime-render --example graphics_settings` measures the CPU
control path without opening a GPU or window. Unchanged preferences reuse the
resolved plan; display enumeration occurs only when queried or preferences change.

## Verification

The feature's Windows checkpoint passed:

- Focused `cargo check` and `cargo clippy --all-targets -- -D warnings` for all ten
  changed crates; `ara-dev fmt --check` (111 workspace packages).
- `ara-graphics-settings` (7), `ara-runtime-render --lib` (68), `ara-player --bins`
  (44 passed, 3 existing ignored), and the real C# `ara-player --test graphics`.
- `ara-diag --test coverage`, the GPU/platform/play/host/script suites (including
  generated ABI freshness and hot reload), and all 8 `ara --test cli` tests. The
  reflection snapshot remained current.
- `dotnet test Ara.slnx -c Release`: 150 passed, including the native API contract,
  typed serialization, large mode-list responses, event semantics and host isolation.

The headless test's PNGs and per-run profiles live under `target/player-graphics/`.
Only offscreen output was inspected. Physical monitor switching/exclusive refresh
and macOS/Linux were not exercised at this Windows checkpoint. HDR remains
unavailable until the player gains a complete HDR scene/UI output encoder.
