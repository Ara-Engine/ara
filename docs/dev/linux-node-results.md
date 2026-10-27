# Linux bring-up results: m6u

2026-10-27; base `81c0b8e9` plus `t3/linux-node` portability fixes.
All commands ran on Ubuntu 26.04 / Linux 7.0, Rust 1.99.1, GCC 15.2, .NET SDK 10.0.403, Mesa RADV 26.0.8 on the Radeon 760M. `CARGO_BUILD_JOBS=10`.

## Gates and timings

| Command / route | Result | Wall time |
|---|---|---:|
| `cargo check --workspace --all-targets`, cold target | PASS: 104 packages | 170 s |
| Same check after fixes | PASS | 94 s |
| Editor + matching player + Hub + CLI through `ara-dev cargo` | PASS | 251 s including queue; Cargo 186 s |
| Standalone `ara-import-worker` build | PASS | Cargo 110 s |
| Strict Clippy: `ara-gpu`, `ara-editor`, `ara-hub`, all targets | PASS | 93 s |
| `ara-dev fmt --check` | PASS | 105 s |
| `dotnet build dotnet/Ara.slnx -c Release` | PASS, no warnings | 17 s |
| .NET solution tests | PASS: 114 tests | 4 s |
| NativeAOT sample, `linux-x64` | PASS, GCC fallback | 8 s |
| Default `cargo test` suites | 87 PASS / 3 FAIL across 90 crates | See matrix |
| Hub GPU capture, 1440x900 | PASS: 32 dark/light captures | 1 s |
| Editor `--verify`, fresh project, Wayland | PASS | 7 s |
| Editor `--verify`, fresh project, X11 | PASS | 9 s |
| Editor opt-in native suite, Wayland | 1 PASS / 2 FAIL | 48 s including compile; tests 13.86 s |
| Native docking alone, X11 | PASS | 7 s |
| Asset-complete Outpost `--verify-journey` | INCOMPLETE; bounded stop | 301 s |

Build/test wall times include Cargo lock waits: several batches shared this one checkout. They are run records, not isolated per-crate benchmarks. Default suites retain their declared ignored tests; the native suite above was run explicitly. No checked-in shaders or goldens were changed.

## PC comparison

The same `cargo test -p ara-schema` used fresh target directories, warm download caches, and the pinned toolchain on both machines: **m6u 18.46 s (10 jobs)** versus **Windows 44.09 s (2 jobs)**. Other work was active on both hosts. This compares the practical offload configurations, not isolated CPU performance. No heavy Windows editor build was started.

## Linux failures and handoffs

| Area / owner | Reproduction and evidence | Diagnosis / disposition |
|---|---|---|
| GPU device selection | First Hub capture: `No discrete GPU` | Fixed: discrete preferred, integrated/virtual/CPU fallbacks allowed, existing capability checks retained. RADV GPU tests and captures pass. |
| Editor verification startup | Wayland `--verify`: `position verification window` | Fixed: Wayland placement is compositor-owned; fixed pixel extent still checked. Fresh Wayland/X11 routes pass. |
| Hub warning | Linux unused `shown` in `native/os.rs` | Fixed: construct the Explorer/Finder string only on those platforms. Strict Clippy passes. |
| Audio graph / audio owner | `cargo test -p ara-audio-graph --test render golden_example_captures` | Repeated captures are deterministic within Linux; footsteps WAV hash differs from the checked golden, with metric drift up to 2.1e-7 dB. Needs cross-platform DSP/golden policy. |
| Acoustics / audio owner | `cargo test -p ara-acoustics --test render golden_two_room_capture_is_deterministic` | Only two peak dB fields differ by 3.6e-15; WAV hash and all other values match. Same-platform golden contract needs Linux evidence or a reviewed metric comparison. |
| Music / audio owner | `cargo test -p ara-music --test music golden_music_session` | Same WAV hash; spectrum metrics differ by up to 5.5e-6 dB, channel RMS by 7.6e-12. No golden was rewritten. |
| Floating windows / editor UX owner | Wayland: `cargo test -p ara-editor --test native native_docking_drags_match_drop_previews -- --ignored --exact` | `SDL position: wayland cannot position non-popup windows`. `app/windows.rs` still sets absolute floating-window positions. X11 passes the identical test. Needs Wayland placement/coordinate handling beyond the startup fix. |
| Native golden / UI + renderer owners | `cargo test -p ara-editor --test native native_interaction_route_matches_reviewed_dark_and_light_goldens -- --ignored --exact` | `ara-dark` SSIM loss 0.00788129 exceeds 0.005. Actual and diff retained under `target/ara-editor-native/golden-failure/`; needs reviewed platform comparison, not a blind update. |
| Editor Build / export owner | Outpost journey step 08, diagnostic ARA4505 | `ara-player.exe is not next to the editor`. `shell/build.rs` defaults/pins/runs a Windows template even on Linux. The native `ara-player` exists and its companion probe passes. |
| Game preview / renderer owner | Asset-complete journey step 13 | Failed after 12,961 frames / about 99 s waiting for Game preview readiness; the run then hit its 300 s bound. No import-worker errors in this run. Preview/readiness needs isolation. |
| Embedded Play / GPU + play owners | Baseline journey step 14 waits for a Game-view frame; inspect `ara-gpu/src/vulkan/external.rs` | Linux shared surfaces are explicitly unsupported (`cfg!(windows)`, opaque NT handles). Need Linux external-memory FD support. Windowed Play/start-cancel steps passed in the journey. |

The first journey attempt omitted the standalone import-worker binary; that setup error was corrected before the asset-complete retry. A reused sample fixture also caused an image-change assertion failure; fresh fixtures passed on both display backends. Neither is presented as an unresolved engine defect.

## Evidence and reproduction

Setup and environment: [linux-node.md](linux-node.md). All raw logs and timings live in `~/ara/target/linux-node/`; copy them with SCP when investigating. Key files:

- `workspace-check.log`, `final-gates.tsv`, `test-<crate>.log`, and the three `*-tests.tsv` batch records.
- `audio-golden-diff.json`, `acoustics-golden-diff.json`, `music-golden-diff.json`: field-level differences from temporary probes; all probe source edits were removed.
- `hub-captures/1440x900/` (all 32 inspected as a contact sheet), `editor-verify-wayland/` (dark/light images inspected), `editor-verify-x11-fresh/`.
- `editor-native.log`, `editor-native-x11.log`, `editor-journey-ready/journey.json`, `editor-journey-ready.log`, and `journey-ready/samples/outpost/.ara/diagnostics.jsonl`.

The PC worktree also holds the key logs/diffs in `target/linux-node/evidence/`,
the Hub contact sheet at `target/linux-node/hub-contact-sheet.png`, and the
inspected editor images in `target/linux-node/editor-verify-wayland/`.
All test/editor/player/import-worker processes exited; the .NET build servers
were shut down. Docker, Tailscale, nginx, console, websockify and T3 services
remained active. No service, timer, watcher, shell profile or system package was added.

With the environment from the setup guide, the asset-complete journey command was:

```sh
target/debug/ara-editor target/linux-node/journey-ready/samples/outpost \
  --verify-journey --frames 200000 --size 1280x720 --dpi 1 \
  --output target/linux-node/editor-journey-ready
```

It ran under `timeout -k 15s 300s`; status 124 means the bound was reached, not a passing journey. The scratch Outpost copy was populated by `ara-samples fetch outpost` and `kits outpost`, using the lock and assets under `target/`.

## Per-crate matrix

`check` is the all-target workspace type check, not a claim that every standalone binary was built. `tests` means the default crate suite; rendering/shader suites left outside the requested CPU sweep are explicit NOT RUN rows. Passed/failed/ignored count harness results, including doctests. Times include compilation and queue waits.

| Crate | Check | Tests | Passed | Failed | Ignored | Seconds | Notes |
|---|---|---|---:|---:|---:|---:|---|
| ara | PASS | PASS | 36 | 0 | 0 | 146 |  |
| ara-2d | PASS | PASS | 5 | 0 | 0 | 12 |  |
| ara-acoustics | PASS | FAIL | 14 | 1 | 1 | 75 | two-room golden: 3.6e-15 dB, same WAV hash |
| ara-ai | PASS | PASS | 16 | 0 | 0 | 24 |  |
| ara-anim | PASS | PASS | 37 | 0 | 0 | 16 |  |
| ara-anim-capture | PASS | NOT RUN | - | - | - | - | Renderer/shader-specific suite; player binary built separately where applicable |
| ara-anim-physics | PASS | PASS | 1 | 0 | 0 | 14 |  |
| ara-asset | PASS | PASS | 23 | 0 | 1 | 10 |  |
| ara-asset-format | PASS | PASS | 4 | 0 | 0 | 2 |  |
| ara-audio | PASS | PASS | 35 | 0 | 1 | 20 |  |
| ara-audio-graph | PASS | FAIL | 14 | 1 | 0 | 14 | footsteps golden: PCM hash + tiny metric drift |
| ara-audio-mix | PASS | PASS | 6 | 0 | 0 | 13 |  |
| ara-audio-why | PASS | PASS | 5 | 0 | 0 | 3 |  |
| ara-bench | PASS | PASS | 1 | 0 | 0 | 49 |  |
| ara-brand | PASS | PASS | 5 | 0 | 0 | 16 |  |
| ara-bridge | PASS | PASS | 4 | 0 | 0 | 17 |  |
| ara-cmd | PASS | PASS | 12 | 0 | 0 | 5 |  |
| ara-collab | PASS | PASS | 14 | 0 | 2 | 28 |  |
| ara-color | PASS | PASS | 2 | 0 | 0 | 1 |  |
| ara-components | PASS | PASS | 8 | 0 | 0 | 1 |  |
| ara-crash | PASS | PASS | 2 | 0 | 0 | 20 |  |
| ara-crowd | PASS | PASS | 12 | 0 | 0 | 0 |  |
| ara-dev | PASS | PASS | 106 | 0 | 1 | 121 |  |
| ara-diag | PASS | PASS | 30 | 0 | 1 | 2 |  |
| ara-doc | PASS | PASS | 21 | 0 | 0 | 21 |  |
| ara-editor | PASS | PASS | 160 | 0 | 9 | 132 | Opt-in native: 1/3 pass on Wayland; X11 docking passes; see failures above |
| ara-editor-gpu | PASS | PASS | 1 | 0 | 1 | 45 |  |
| ara-export | PASS | PASS | 19 | 0 | 3 | 156 |  |
| ara-export-installer | PASS | PASS | 3 | 0 | 0 | 14 |  |
| ara-export-player | PASS | PASS | 0 | 0 | 0 | 16 |  |
| ara-foliage | PASS | PASS | 3 | 0 | 0 | 158 |  |
| ara-geo | PASS | PASS | 6 | 0 | 1 | 8 |  |
| ara-gpu | PASS | PASS | 42 | 0 | 1 | 27 |  |
| ara-graph | PASS | PASS | 3 | 0 | 0 | 1 |  |
| ara-host | PASS | PASS | 40 | 0 | 1 | 111 |  |
| ara-hub | PASS | PASS | 30 | 0 | 1 | 54 |  |
| ara-hub-service | PASS | PASS | 22 | 0 | 0 | 53 |  |
| ara-import-worker | PASS | PASS | 11 | 0 | 2 | 8 |  |
| ara-import2d | PASS | PASS | 3 | 0 | 0 | 15 |  |
| ara-importers | PASS | PASS | 44 | 0 | 0 | 96 |  |
| ara-input | PASS | PASS | 1 | 0 | 0 | 2 |  |
| ara-jolt-sys | PASS | PASS | 1 | 0 | 0 | 1 |  |
| ara-json | PASS | PASS | 9 | 0 | 0 | 6 |  |
| ara-l10n | PASS | PASS | 13 | 0 | 0 | 20 |  |
| ara-landscape | PASS | PASS | 5 | 0 | 0 | 25 |  |
| ara-material | PASS | NOT RUN | - | - | - | - | Renderer/shader-specific suite; player binary built separately where applicable |
| ara-mcp | PASS | PASS | 7 | 0 | 0 | 193 |  |
| ara-merge | PASS | PASS | 7 | 0 | 0 | 0 |  |
| ara-meshopt-sys | PASS | PASS | 4 | 0 | 0 | 2 |  |
| ara-music | PASS | FAIL | 10 | 1 | 0 | 2 | session golden: up to 5.5e-6 dB, same WAV hash |
| ara-nav | PASS | PASS | 26 | 0 | 0 | 12 |  |
| ara-nav-sys | PASS | PASS | 0 | 0 | 0 | 0 |  |
| ara-net | PASS | PASS | 19 | 0 | 4 | 91 |  |
| ara-ops | PASS | PASS | 15 | 0 | 0 | 4 |  |
| ara-opus-sys | PASS | PASS | 2 | 0 | 0 | 12 |  |
| ara-pack | PASS | PASS | 7 | 0 | 0 | 83 |  |
| ara-pathtrace | PASS | NOT RUN | - | - | - | - | Renderer/shader-specific suite; player binary built separately where applicable |
| ara-perception | PASS | PASS | 3 | 0 | 0 | 66 |  |
| ara-physics | PASS | PASS | 1 | 0 | 0 | 60 |  |
| ara-physics-jolt | PASS | PASS | 30 | 0 | 1 | 33 |  |
| ara-physics2d | PASS | PASS | 12 | 0 | 0 | 54 |  |
| ara-platform | PASS | PASS | 4 | 0 | 4 | 14 |  |
| ara-play | PASS | PASS | 4 | 0 | 0 | 78 |  |
| ara-play-host | PASS | PASS | 17 | 0 | 0 | 63 |  |
| ara-player | PASS | NOT RUN | - | - | - | - | Renderer/shader-specific suite; player binary built separately where applicable |
| ara-profile | PASS | PASS | 14 | 0 | 0 | 27 |  |
| ara-proto | PASS | PASS | 3 | 0 | 0 | 1 |  |
| ara-release | PASS | PASS | 13 | 0 | 1 | 9 |  |
| ara-render | PASS | NOT RUN | - | - | - | - | Renderer/shader-specific suite; player binary built separately where applicable |
| ara-render-fidelity | PASS | NOT RUN | - | - | - | - | Renderer/shader-specific suite; player binary built separately where applicable |
| ara-render-geo | PASS | NOT RUN | - | - | - | - | Renderer/shader-specific suite; player binary built separately where applicable |
| ara-render2d | PASS | NOT RUN | - | - | - | - | Renderer/shader-specific suite; player binary built separately where applicable |
| ara-rpc | PASS | PASS | 4 | 0 | 0 | 19 |  |
| ara-runtime | PASS | PASS | 43 | 0 | 1 | 93 |  |
| ara-runtime-audio | PASS | PASS | 0 | 0 | 0 | 22 |  |
| ara-runtime-net | PASS | PASS | 5 | 0 | 1 | 165 |  |
| ara-runtime-render | PASS | NOT RUN | - | - | - | - | Renderer/shader-specific suite; player binary built separately where applicable |
| ara-runtime-script | PASS | PASS | 0 | 0 | 0 | 24 |  |
| ara-runtime-world | PASS | PASS | 4 | 0 | 0 | 4 |  |
| ara-samples | PASS | NOT RUN | - | - | - | - | Renderer/shader-specific suite; player binary built separately where applicable |
| ara-save | PASS | PASS | 10 | 0 | 0 | 47 |  |
| ara-schema | PASS | PASS | 18 | 0 | 1 | 10 |  |
| ara-schema-derive | PASS | PASS | 0 | 0 | 0 | 0 |  |
| ara-script | PASS | PASS | 38 | 0 | 1 | 173 |  |
| ara-sdl3-src | PASS | PASS | 0 | 0 | 0 | 0 |  |
| ara-setup | PASS | PASS | 0 | 0 | 0 | 4 |  |
| ara-shader-cook | PASS | NOT RUN | - | - | - | - | Renderer/shader-specific suite; player binary built separately where applicable |
| ara-sim | PASS | PASS | 24 | 0 | 0 | 13 |  |
| ara-terrain | PASS | PASS | 7 | 0 | 0 | 120 |  |
| ara-texture-gpu | PASS | NOT RUN | - | - | - | - | Renderer/shader-specific suite; player binary built separately where applicable |
| ara-texture-stream | PASS | PASS | 18 | 0 | 2 | 9 |  |
| ara-ui | PASS | PASS | 28 | 0 | 0 | 41 |  |
| ara-ui-document | PASS | PASS | 6 | 0 | 0 | 11 |  |
| ara-ui-render | PASS | PASS | 9 | 0 | 0 | 22 |  |
| ara-ui-stories | PASS | PASS | 4 | 0 | 0 | 7 |  |
| ara-ui-test | PASS | NOT RUN | - | - | - | - | Renderer/shader-specific suite; player binary built separately where applicable |
| ara-ui-text | PASS | PASS | 5 | 0 | 0 | 28 |  |
| ara-update | PASS | PASS | 14 | 0 | 0 | 40 |  |
| ara-vfx | PASS | PASS | 9 | 0 | 0 | 82 |  |
| ara-vfx-gpu | PASS | NOT RUN | - | - | - | - | Renderer/shader-specific suite; player binary built separately where applicable |
| ara-vsm | PASS | PASS | 1 | 0 | 0 | 83 |  |
| ara-why | PASS | PASS | 17 | 0 | 0 | 99 |  |
| ara-world | PASS | PASS | 12 | 0 | 0 | 10 |  |
| arabench | PASS | PASS | 6 | 0 | 0 | 33 |  |
