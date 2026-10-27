# Linux build and test node: m6u

`ssh m6u` reaches Islam's always-on Ubuntu 26.04 x86-64 mini PC over Tailscale.
It has a Ryzen 5 7640HS (6 cores / 12 threads), 24 GiB RAM, and a Radeon 760M
using Mesa RADV. Use it for Linux compilation and CPU tests. Windows remains the
canonical shader cook host; use the committed cooked artifacts unchanged.

## Setup

Toolchains live in the user's home directory. No shell profiles, services,
timers, Docker containers, Tailscale proxies, or desktop services are changed.
GCC/G++, build-essential, zlib development files, the Vulkan loader, and Mesa
26.0.8 were already installed. SDL's desktop/audio headers are vendored.

- Rust 1.99.1, rustfmt and clippy: [rustup](https://rust-lang.org/tools/install/)
  in `~/.cargo` / `~/.rustup`, installed
  with `--profile minimal --no-modify-path` and the repository's pinned version.
- .NET SDK 10.0.403: Microsoft's [dotnet-install.sh](https://learn.microsoft.com/en-us/dotnet/core/tools/dotnet-install-script) with `--channel 10.0
  --install-dir "$HOME/.dotnet" --no-path`.
- Vulkan SDK 1.4.363.0: the official Linux x86-64 tarball under `~/VulkanSDK`,
  needed for debug GPU validation (not shader cooking). Verify the archive with
  LunarG's published SHA-256 before extracting it. System Mesa supplies RADV.
- Git transport: bare repository `~/ara.git`; checkout `~/ara`, whose `origin`
  points at that bare repository. The PC's `m6u` remote is `ssh://m6u/~/ara.git`.

Noninteractive SSH does not load toolchain paths. From `~/ara`, use:

```sh
export PATH="$HOME/.cargo/bin:$HOME/.dotnet:$PATH"
export DOTNET_ROOT="$HOME/.dotnet" CARGO_BUILD_JOBS=10
export DOTNET_CLI_HOME="$PWD/target/linux-node/dotnet-home"
export DOTNET_CLI_TELEMETRY_OPTOUT=1 DOTNET_GENERATE_ASPNET_CERTIFICATE=false
mkdir -p target/linux-node/profiles
profile=$(mktemp -d "$PWD/target/linux-node/profiles/run-XXXXXX")
export ARA_CONFIG_DIR="$profile/config" ARA_CRASH_DIR="$profile/crashes"
```

For GPU tests, additionally set:

```sh
export VULKAN_SDK="$HOME/VulkanSDK/1.4.363.0/x86_64"
export PATH="$VULKAN_SDK/bin:$PATH"
export LD_LIBRARY_PATH="$VULKAN_SDK/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export VK_LAYER_PATH="$VULKAN_SDK/share/vulkan/explicit_layer.d"
export VK_DRIVER_FILES=/usr/share/vulkan/icd.d/radeon_icd.json
```

Hub captures need no display. Editor verification uses never-shown SDL windows;
on this node set `XDG_RUNTIME_DIR=/run/user/1000`, `WAYLAND_DISPLAY=wayland-0`,
and `SDL_VIDEODRIVER=wayland`. X11 also works using the session's `DISPLAY` and
`XAUTHORITY` from `systemctl --user show-environment` (read only those variables).

## Two-command sync

Commit the desired work locally first. From any PC worktree with the `m6u`
remote, push the branch, then check out its exact SHA on the node:

```sh
git push m6u main HEAD:refs/heads/t3/linux-node
ssh m6u 'cd ~/ara && git fetch origin && git checkout --detach <sha>'
```

Replace the destination branch when syncing another agent. Never use
`--force`, overwrite a dirty checkout, switch revisions during a running build,
or push to GitHub as part of node sync. The single `~/ara/target` is a shared
warm build cache: coordinate users so they run one build/test batch at a time.
Linux binaries run on Linux; this node does not produce Windows editor builds.

## Checks and evidence

Run `cargo check --workspace --all-targets` first, then focused
`cargo test -p <crate>` commands. Build the editor and its matching player through
`cargo run -p ara-dev -- cargo -- build -p ara-editor -p ara-hub -p ara -p ara-import-worker`.
The import worker is required for real project assets. Use fresh profiles under `target/` for every
batch. Offscreen verification must stay windowless; never launch desktop apps
from SSH. Stop only child processes started by the batch.

Logs, timings, and captures are under `~/ara/target/linux-node/`.

Passwordless sudo is unavailable. If a required system package is missing,
stop and give Islam the exact `sudo apt install ...` command; do not change
system configuration or try passwords.

## Bring-up result (2026-10-27)

Base: `81c0b8e9`, with the portability fixes on `t3/linux-node`.

- The cold all-target workspace check passed in **170 s**. Editor + matching
  player + Hub + CLI built in **251 s** including queue time (**186 s** in Cargo).
- Strict Clippy for the changed crates, workspace formatting, the .NET solution
  build and all **114 .NET tests** passed. The NativeAOT sample published with GCC.
- Default tests: **87 of 90 crate suites passed** (1,309 passing test cases,
  three audio golden failures). [Full results and per-crate matrix](linux-node-results.md).
- A fresh-target `cargo test -p ara-schema` took **18.46 s** here (10 jobs) versus
  **44.09 s** on the PC (2 jobs). Download caches were warm; other work was active.
  This compares the intended offload settings, not isolated CPU performance.
- RADV: all **32 Hub captures** passed; editor `--verify` passed on Wayland
  (**7 s**) and X11 (**9 s**), with the dark/light captures visually inspected.
- Fixed: Vulkan rejected every integrated GPU; Wayland verification required
  compositor-denied window placement; Hub had a Linux-only Clippy warning.

Known gaps: audio goldens in `ara-audio-graph` differ in PCM hash and tiny metrics;
`ara-acoustics` and `ara-music` differ only in tiny metrics (PCM hashes unchanged). Their current
same-platform golden contract needs an audio-owner decision. The Outpost journey
exposes Windows-only editor export (`ara-player.exe`) and embedded Play's opaque
NT handles; the asset-complete run also failed Game preview readiness and reached
its 300 s bound. Native docking passes on X11 but fails on Wayland's absolute
placement; the native dark golden also exceeds its SSIM threshold. Evidence,
reproduction commands and owners are in the linked results. Full journeys are not green.
