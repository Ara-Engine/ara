# ara-export-player

Headless export conformance template, adapted from Ara's own S7 fixture.
Proves pack v2 loading and the S7 C# NativeAOT/CoreCLR ABI on shipping targets.
Sample simulation columns live in bevy_ecs components, with a seeded fixed tick.
This is a template conformance driver, not a replacement for ara-player/ara-sim.
The integration task must prepare real engine runtime pins once the tracks merge.
No SDL window is created by this headless fixture; production platform integration
uses SDL3 (A9). Android currently uses NativeActivity for this test harness only.

Dev/profile launches can embed ara-bridge; release launch config disables it.
Renderer capture, semantic queries and real engine input hooks remain seams.
Run `cargo test -p ara-export-player`, `cargo clippy -p ara-export-player --all-targets`.
Build with CARGO_BUILD_JOBS=2. Cross builds require target linker environment.

Desktop entry locates its sibling `data/` (legacy flat pins remain readable), calls
ara_crash::monitor_from_args before app initialization, then
shipping::start before loading C#. The guard owns and reaps the monitor process.
Crash config is build-scoped; storage uses LOCALAPPDATA/XDG_STATE_HOME, overridable
with ARA_CRASH_DIR for tests. Neither capture nor startup transmits anything.
`--crash-probe native|panic|managed` deliberately faults an explicitly launched
child for export verification. No store-specific runtime state is present.
