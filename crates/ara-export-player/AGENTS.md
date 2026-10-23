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
