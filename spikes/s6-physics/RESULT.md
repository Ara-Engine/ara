# S6: Physics, Jolt (own binding) vs Rapier

**PASS, 2026-10-23.** All five scenarios ran on both engines with recorded numbers, all three determinism tests ran on both, and the recommendation is clear: **Jolt 5.6, through a thin C shim we own, built from vendored source with `cc`**.

Jolt passed every character-controller lane (Rapier failed both moving-platform lanes). It drove the vehicle on its defaults. It stayed within ±15% of Rapier at 5k and 20k bodies with tighter tail latency, and its rollback snapshots were 7–9× smaller and at least 20× faster to restore. Rapier's strengths are real (pure Rust, stands a tall stack on defaults, a little faster at 5k bodies multi-threaded) but they don't outweigh those gaps.

## Criteria

| Criterion | Result | Evidence |
|---|---|---|
| (a) Character: 35° slope, 0.3 m stairs, moving platform, wall | **Jolt 7/7 lanes. Rapier 5/7**: both moving-platform lanes fail | [Character](#a-character-controller) |
| (b) Stacking: 10×10 pyramid + 20-box tower, drift at 10 s, sleep time | **Both measured.** Rapier stands on defaults (sleeps 0.82 s). Jolt's tower topples on defaults (10 velocity steps) and stands with 15 (sleeps 0.83 s) | [Stacking](#b-stacking) |
| (c) Vehicle: 4 wheels, drive and turn | **Both pass.** Jolt on defaults; Rapier after lowering tyre grip from its 10.5 default, which rolls the car | [Vehicle](#c-vehicle) |
| (d) Scale: 5k / 20k bodies, p50/p95, single- and multi-threaded | **Measured on both**, 1/4/8 threads, four build flavours | [Scale](#d-scale) |
| (e) Determinism: same process, cross process, snapshot/restore | **Both bit-exact in all three**, plus 1/4/8-thread invariance and a negative control, in fast and deterministic builds | [Determinism](#e-determinism) |
| Integration notes: ergonomics, ECS sync, queries, FFI cost, build time, binary size, platforms, maintenance | **Recorded** | [Integration](#integration-notes) |
| Recommendation with reasoning, and the binding approach for Jolt | **Jolt via an own C shim** | [Recommendation](#recommendation) |
| `ara-physics` crate boundary sketch | **Included** | [Boundary](#proposed-ara-physics-crate-boundary) |

## What was built

A standalone Cargo workspace (`spikes/s6-physics`, not a repo workspace member) with two parts:

- **`jolt-sys/`: Ara's Jolt binding.**
  - `shim/ara_jolt.{h,cpp}`: a 30-function C ABI over Jolt in about 490 lines of C++.
    - World creation with either a single-threaded job system or a thread pool.
    - Batch body creation; bulk state read.
    - Kinematic targets.
    - Narrow-phase ray and sphere casts.
    - Snapshot save/restore as bytes (PhysicsSystem state, including vehicle constraints, plus every CharacterVirtual).
    - CharacterVirtual (`ExtendedUpdate` with stair walking and stick-to-floor).
    - A wheeled VehicleConstraint: engine, gearbox, rear differential, anti-roll bars.
  - `build.rs` compiles the vendored Jolt sources and the shim with the `cc` crate (no CMake). `src/lib.rs` holds the hand-written `extern "C"` declarations; a test checks every struct size against the C compiler.
- **`src/`: the harness.**
  - An `Engine` trait holding the primitives the scenarios need, implemented once per engine: `jolt.rs` over the shim, and `rapier.rs` over `rapier3d` 0.36.1 used directly.
  - The scenarios are written once against that trait (`scenarios.rs`, `scene.rs`), so both engines see identical geometry, inputs and metrics.
  - Every run is fixed-step at dt = 1/60.
- **Scripts and probes:** `run.ps1` (all flavours), `ab-bench.ps1` (interleaved timing), `measure-build.ps1` with `probes/{empty,jolt,rapier}` (build time, binary size), `check-android.py`, and `summarize.py`.

Each engine is used the way its own documentation and samples use it:
- **Jolt:** the `CharacterVirtualTest` velocity loop; `VehicleConstraintTest` setup; `MoveKinematic`; `SaveState(All)`.
- **Rapier:** `KinematicCharacterController::move_shape`; `DynamicRayCastVehicleController`; serde snapshots of `PhysicsWorld`.

Four build flavours, each in its own target directory:

| Flavour | Jolt | Rapier | Used for |
|---|---|---|---|
| `fast-st` | `/fp:fast`, AVX2 + FMA | default (4-lane SIMD, baseline x86-64), no rayon | scenarios (a)–(c), queries, single-threaded scale |
| `fast-par` | same | `parallel` (rayon) | multi-threaded scale, thread-count determinism |
| `fast-par-simd8-avx2` | same | `parallel,simd8`, `RUSTFLAGS=-C target-cpu=x86-64-v3` | Rapier on equal SIMD footing (Jolt's C++ is always AVX2) |
| `det-par` | `JPH_CROSS_PLATFORM_DETERMINISTIC`, `/fp:precise`, no FMA | `enhanced-determinism,parallel` | determinism mode, and its cost |

## Exact commands

PowerShell 5.1, from the repository root. Rust 1.96.0 (repo toolchain); MSVC 14.50.35717 (VS 18 Build Tools), which `cc` finds on its own. Python 3 (standard library only) is used for the summary and the Android compile check.

```powershell
Set-Location spikes/s6-physics
$env:CARGO_BUILD_JOBS = '4'
./setup.ps1                 # shallow-fetches Jolt e77f1755 (tag v5.6.0) into vendor/ (gitignored)
./run.ps1                   # builds 4 flavours into target/<flavour>/ and runs every scenario (~30 min)
./run.ps1 -Only determinism # re-runs determinism only (used after adding the negative control)
./ab-bench.ps1 -Reps 5      # interleaved A/B timing across builds (~15 min)
./measure-build.ps1         # clean build time and binary size of the three probes
python check-android.py D:/Android/Sdk/ndk/28.2.13676358   # compile-only, aarch64 Android
python summarize.py         # evidence/*/*.json -> evidence/summary.md
cargo test --release --workspace   # shim layout, snapshot smoke, bit-exact replay for both engines
cargo clippy --release --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

A single scenario: `target/fast-st/release/s6.exe character --engine both` (also `stacking`, `vehicle`, `queries`, `determinism --threads 1,4,8`, `scale --count 5000,20000 --threads 1,4,8 --steps 600`). JSON goes to `evidence/<flavour>/`; [`evidence/summary.md`](evidence/summary.md) collects every table.

## Machine and conditions

- **Hardware:** Ryzen 7 7800X3D (8 cores / 16 threads), 31.1 GiB RAM.
- **OS:** Windows 11 Pro 10.0.26300 ([machine.json](evidence/machine.json)).
- **Background load:** other agents shared the PC throughout. Total CPU use sampled between runs ranged from **27% to 87%** (`evidence/load-*.json`).
- **What that means for the timings:** two flavours whose Jolt code is byte-identical (`fast-st` and `fast-par`) differ by up to 20% in Jolt's single-threaded timings. Treat single-run differences under about 20% as noise. Build-to-build costs come from the interleaved A/B runs, not from comparing tables.
- **Defaults kept unless stated:**
  - Jolt: 10 velocity and 2 position steps.
  - Rapier: 4 solver iterations.
  - Both: friction 0.5–0.6, density 1000 kg/m³, and each engine's own sleep thresholds.

## Results

### (a) Character controller

Both characters are a capsule (radius 0.3 m, height 1.8 m) with a 45° slope limit, 0.35 m step height and 0.5 m stick-to-floor. Each lane runs 6 s with its own character, all in one world. Platforms move ±3 m along Z at up to 4.7 m/s, and the second one also bobs ±0.5 m vertically.

| Lane | Jolt CharacterVirtual | Rapier KinematicCharacterController |
|---|---|---|
| 35° slope, 8 m, walk to the top | PASS, y = 4.589 (landing 4.589) | PASS, y = 4.608 |
| Stop half-way up 35° and stand 4 s | PASS, slid 0.0000 m | PASS, slid 0.0097 m |
| 50° slope (control, must block) | PASS, max y 0.000 | PASS, max y 0.026 |
| Stairs, 6 × 0.3 m rise, 0.4 m treads | PASS, y = 1.800 | PASS, y = 1.820 |
| Moving platform, horizontal | **PASS, drift 0.000 m** | **FAIL**, drifts 3.6 m and falls off |
| Moving platform, horizontal + vertical | **PASS, drift 0.117 m, gap ≤ 0.017 m** | **FAIL**, drifts 5.8 m and falls off |
| Walk 45° into a wall for 3 s | PASS, 0 penetration, slides 6.36 m | PASS, 0 penetration, slides 6.26 m |
| Update cost per character (p50 / p95) | **1.43 / 2.93 µs** | 10.2 / 23.5 µs |

Identical in the `det-par` build (Jolt 1.87 µs p50, Rapier 12.7 µs).

- **Jolt:** the documented loop works unchanged on moving platforms.
  - `MoveKinematic` sets the platform's velocity immediately, so `UpdateGroundVelocity` sees this tick's motion.
  - CharacterVirtual's solver treats contacts with their velocities.
  - One addition was needed: the sample's `OnContactSolve` policy ("don't slide down walkable slopes while idle"). It lives in the shim as a C++ `CharacterContactListener`, so no callback crosses the FFI boundary.
- **Rapier:** `move_shape` has built-in "kinematic friction" for platforms, but it applies only on iterations where the sweep **hits** the platform. A rider standing at the KCC offset distance registers a hit on some ticks and not others. As a result:
  - riders slip on a horizontally moving platform;
  - on a descending platform the sweep never reaches it, so friction never applies.
- **Rapier workarounds tried** in this spike (none committed except the velocity-based platform):
  - feeding ground velocity manually: double-applies with the built-in friction;
  - a post-step carry: double-applies too;
  - running characters after the step;
  - a velocity-based kinematic platform (kept: it fixes a one-tick velocity lag of position-based kinematics);
  - a downward "stick" velocity: breaks the slope and stairs lanes.

  None passes without changing Rapier's controller itself.

### (b) Stacking

A 10×10-base square pyramid (385 unit boxes), placed exactly touching, plus a 20-box tower of unit boxes, on a static slab. Lean is the largest horizontal displacement after 10 s.

| | Pyramid lean | Tower lean | Tower top y (placed 19.5) | Standing at 10 s | All asleep at | Awake step mean / p95 |
|---|---|---|---|---|---|---|
| Jolt, defaults (10 velocity steps) | 0.62 mm | **14.8 m (toppled at ~9 s)** | 9.54 | **no** | 13.2 s (after the collapse) | 1.10 / 2.54 ms |
| Jolt, 15 velocity steps | 0.35 mm | 17.3 mm | 19.458 | yes | **0.83 s** | 1.44 / 3.23 ms |
| Rapier, defaults | 0.53 mm | 9.6 mm | 19.395 | yes | **0.82 s** | 0.83 / 1.87 ms |

- The tower leans slowly with Jolt's defaults and never falls asleep (its points stay above the 0.03 m/s sleep threshold).
- The velocity-step sweep was sharp: 11 and 12 steps topple at about 9 s, 13 and 14 topple later, and 15 or more stand.
- Rapier's soft contacts sag more (0.105 m over 20 boxes, against Jolt's 0.042 m) but hold.
- **Rule for Ara:** run Jolt with 15 velocity steps by default, or expose the setting per scene. The pyramid is solid either way.

### (c) Vehicle

A 1500 kg car (1.8 × 0.4 × 4 m), 0.35 m wheels, 1.5 Hz / 0.5-damping suspension, 30° front steering, rear-wheel drive. It settles for 1 s, drives straight for 5 s at full throttle, then turns right for 5 s at half throttle.

| | Speed at 5 s | Distance | Lateral drift | Turn | Radius | Min up·Y | Wheels down | Step p50 |
|---|---|---|---|---|---|---|---|---|
| Jolt VehicleConstraint | 17.3 m/s | 48.3 m | 0.014 m | 96.6° right | 48.6 m | 1.000 | 4.0 | 4.4 µs |
| Rapier DynamicRayCastVehicleController | 20.0 m/s | 50.2 m | 0.000 m | 133.4° right | 38.3 m | 1.000 | 4.0 | 3.9 µs |

- **Jolt** uses its defaults: engine (500 N·m max torque, RPM curve), automatic gearbox, differential, tyre friction curves and anti-roll bars.
- **Rapier** has no drivetrain. Throttle maps to a constant 3000 N per rear wheel, which is why speed rises linearly to exactly 20 m/s.
- **Rapier's default grip rolls the car.** `friction_slip` defaults to 10.5 (Bullet's value, about 10 g of grip), and the car **rolled over in the turn** (min up·Y = −1, 2.3 wheels down on average). With μ = 1.2 it passes. `roll_influence` is private.
- Rapier's suspension stiffness and damping were converted from Jolt's frequency and damping ratio, so the springs match.

### (d) Scale

The bowl is a 25 m radius, 12 m deep paraboloid triangle mesh (1,153 vertices, 2,256 triangles). Bodies are dropped in 50 × 50 layers at 0.6 m spacing, alternating 0.5 m boxes and 0.5 m spheres with pseudo-random orientations. Each run is 600 steps (falling, impact, pile). Numbers are ms/step over all 600 steps; no body escaped the bowl in any run.

| Bodies | Threads | Jolt p50 / p95 | Rapier p50 / p95 | Rapier (AVX2 + simd8) p50 / p95 |
|---|---|---|---|---|
| 5,000 | 1 | 16.4 / 23.9 (fast-par), 19.7 / 29.2 (fast-st) | 19.1 / 37.2 (fast-par), 16.7 / 35.8 (fast-st) | 11.2 / 27.9 |
| 5,000 | 4 | 6.3 / 8.9 | 5.1 / 10.8 | 3.1 / 9.1 |
| 5,000 | 8 | 4.4 / 5.7 | 3.4 / 7.9 | 2.5 / 6.3 |
| 20,000 | 1 | 116.6 / 168.0 (fast-par), 131.6 / 177.9 (fast-st) | 129.3 / 233.6 (fast-par), 119.1 / 188.3 (fast-st) | 93.7 / 171.1 |
| 20,000 | 4 | 40.8 / 58.6 | 36.0 / 60.9 | 27.4 / 54.0 |
| 20,000 | 8 | 30.9 / 52.1 (25.1 / 31.7 in the AVX2 build's run) | 29.2 / 71.0 | 20.3 / 41.1 |

- **Throughput is a wash.** Rapier's p50 is a little better at 5k bodies multi-threaded.
- **Jolt's tail is consistently tighter:** p95/p50 is 1.3–1.7, against Rapier's 1.7–2.4. Rapier spikes at impact.
- **The AVX2 column's apparent ~1.4× gain is load noise**; the interleaved A/B below shows no consistent gain.
- **Scaling:** 1 → 8 threads is 3.8× (Jolt) to 4.4× (Rapier) at 20k bodies, despite the shared machine.
- **Single-threaded at 20k:** about 120 ms for both. This is a dense 12 m-deep pile, about 6× the cost of 5k bodies.
- **No Jolt capacity overflows:** across all 30 A/B runs the summed `EPhysicsUpdateError` bits are 0. The world is sized at 8× bodies for both pairs and contacts.

#### Interleaved A/B

Each repetition runs every (engine, build) pair back to back and rotates the build order between repetitions. The table is the median of 5 per-run p50s (`ab-bench.ps1`, [evidence/ab/](evidence/ab/)). Load after each repetition was 69, 66, 74, 90 and 41% CPU, so run-to-run spread is ±7–25%. Read only the larger differences.

| Bodies, threads | Engine | fast-par | det-par (cross-platform deterministic) | fast-par-simd8-avx2 |
|---|---|---|---|---|
| 5,000, 1 | Jolt | 17.18 | 18.32 (**+6.6%**) | 18.50 (same Jolt code: noise) |
| 5,000, 1 | Rapier | 18.26 | 19.96 (**+9.3%**) | 17.15 (−6%) |
| 20,000, 8 | Jolt | 31.35 | 35.22 (**+12%**) | 32.94 (same Jolt code: noise) |
| 20,000, 8 | Rapier | 27.57 | 30.56 (**+11%**) | 28.64 (+4%) |

- **Determinism mode costs 7–12% on both engines**, with the same sign in all four comparisons. That matches Jolt's documented ~8%.
- **AVX2 + `simd8` gives Rapier no measurable gain** on this workload.
- **The same-build engine gap is small:** Jolt 6% faster at 5k single-threaded, Rapier 14% faster at 20k on 8 threads.

### (e) Determinism

The scene is built to be chaotic:
- 800 mixed bodies falling into an 8 m bowl;
- a 55-box pyramid;
- a car driven with sinusoidal steering;
- a walking character, and a character riding a moving platform.

All inputs are pure functions of the tick number. The per-tick hash is FNV-1a over the raw bits of every tracked body's position, rotation, linear and angular velocity and sleep flag, plus both characters' state. Runs are 300 ticks.

| Test | Jolt | Rapier |
|---|---|---|
| Same process, two runs, every tick's hash | identical (fast and det builds; 1, 4, 8 threads) | identical (same) |
| Cross process (two child processes vs in-process) | identical | identical |
| Snapshot at tick 100 → run to 200 → restore → re-run to 200 | identical, and equal to the uninterrupted run | identical, and equal |
| Same tick-100 snapshot restored into a **freshly built world** | identical | identical |
| Thread-count invariance (1 vs 4 vs 8 threads) | identical | identical (rayon pool of 1/4/8) |
| Negative control: one body's start x moved by one float step | hash changes (detected) | hash changes (detected) |
| Snapshot size (scene above: ~860 dynamic bodies, 2 characters, 1 car) | **455 KB** | 4.19 MB |
| Save / restore | **0.8–1.4 ms / 0.26–0.52 ms** | 2.2–3.6 ms / 6.4–18 ms |
| Snapshot at scale: 5k / 20k bodies | **3.1 / 13.9 MB**; restore 1.5–3 / 9–18 ms | 23 / 96.5 MB; restore 38–60 / 193–263 ms |

Final hashes at tick 300:

| Engine | fast builds | det build |
|---|---|---|
| Jolt | `a271e625…` | `733d83dc…` |
| Rapier | `b4d3a4a3…` (baseline target), `e2022dc5…` (AVX2 build) | `80d4bed2…` |

What the hashes show:
- **Jolt's fast-build hash** is the same across `fast-st`, `fast-par` and the AVX2 Rust build, because the C++ is compiled identically in all three.
- **Rapier's non-deterministic-mode result changes with the Rust target CPU flags** (FMA and SIMD width). That is fine within one binary, but a server and its clients built with different `target-cpu` would diverge.
- **Both deterministic builds** produce different hashes from the fast builds, as expected (different floating-point modes).
- **Cross-platform equality was not tested.** Only one platform ran here, and the Android build below is compile-only. Jolt states its cross-platform mode is verified upstream on x86/ARM/RISC-V with MSVC, clang and gcc. Rapier claims the same for `enhanced-determinism` with IEEE-754 targets.

**Determinism cost:** 7–12% for both `JPH_CROSS_PLATFORM_DETERMINISTIC` and Rapier `enhanced-determinism`, from the [interleaved A/B](#interleaved-ab). Jolt documents about 8%. Behaviour in the deterministic builds is unchanged: the same lanes pass and fail, and the stacking and vehicle results match.

## Jolt binding: options evaluated

All three options were downloaded and read: crate sources from crates.io, repository data from the GitHub API on 2026-10-23.

| | `rolt` 0.3.1 / `joltc-sys` 0.3.1 | `oxijolt` 1.2.0 / `oxijolt-sys` | **Own thin C shim (this spike)** |
|---|---|---|---|
| Jolt version | 5.0.0 (published crates; repo pushes continue) | 5.6.0 | 5.6.0 (`e77f175`, pinned; trivially bumpable) |
| C layer | SecondHalfGames' JoltC, 218 functions | amerkoleci/joltc (MIT, 218 stars, also behind JoltPhysicsSharp) + its own `joltc_ext` | 30 functions, ~490 lines of C++, written for Ara |
| CharacterVirtual / vehicles / state save-restore | **none of the three** | all three, plus ragdolls and soft bodies | all three (what S6 needs) |
| Can run S6 scenarios (a), (c), (e)? | no | yes | yes |
| Build | bindgen + CMake | **downloads prebuilt native archives with curl by default** (sha256-pinned; x86-64 Windows/Linux only); otherwise CMake ≥ 3.20 | `cc` from `build.rs`; no CMake, no downloads; clean build 37–56 s at 4 jobs |
| Rust API shape | thin, safe-ish wrappers | 45k lines of safe Rust, 1:1 with Jolt objects | ECS-shaped: batch add, batch read, snapshot as bytes; policies stay in C++ |
| Maturity | 103 stars, 5 contributors, 2.5 years | **repo created 2026-10-02**, 13 stars, 6 contributors, 3 releases in 3 weeks | ours |
| Licence | MIT / Apache-2.0 | MIT / Apache-2.0 | ours; Jolt is MIT |

**Production recommendation: own the binding.**
- **`rolt`** cannot run half the scenarios.
- **`oxijolt`** is current and impressively complete, but it:
  - is three weeks old, with one main author;
  - downloads prebuilt binaries during the build by default. That is a supply-chain and reproducibility problem, and the archives only cover x86-64 Windows/Linux (no macOS, iOS or Android);
  - binds Jolt object by object. That means per-body FFI traffic, and Jolt's object model leaking into Ara's API.
- **The shim** took one session and compiled first time. Its shape is what `ara-physics` needs:
  - batch commands in, batch state out;
  - opaque snapshot bytes;
  - contact policies kept in C++.

When the shim needs breadth (more shapes, constraints, ragdolls), copy from `joltc` (MIT) with attribution. Keep `oxijolt` as prior art.

**How the shim is built** (`jolt-sys/build.rs`):
- It globs `vendor/JoltPhysics/Jolt/**/*.cpp` and drops the 8 ObjectStream-only files (Godot's vendored copy of the same commit drops them too).
- It compiles them together with the shim in one `cc::Build`, so the shim and Jolt share every ABI-affecting define.
- Flags mirror Jolt's CMake:
  - always: `/arch:AVX2`, SSE4.1/4.2, LZCNT, TZCNT, F16C;
  - fast builds: `/fp:fast` and FMA;
  - the `cross-platform-deterministic` feature: `/fp:precise`, no FMA, and `JPH_CROSS_PLATFORM_DETERMINISTIC`.
- `cc`'s parallel mode honours Cargo's jobserver, so `CARGO_BUILD_JOBS=4` caps the C++ build too.
- In production the source would be a git submodule or a vendored copy inside `crates/ara-jolt-sys`, rather than `setup.ps1`.

## Integration notes

**API ergonomics from Rust.**
- **Rapier** is idiomatic Rust: glam types since 0.36, a `PhysicsWorld` convenience struct, serde everything.
- **Rapier's friction points:**
  - borrow-splitting: the vehicle needs `QueryPipelineMut` with `&mut` bodies and colliders, built from the broad phase;
  - a stateless KCC whose state game code must own;
  - realistic tuning takes digging (`friction_slip`).
- **The Jolt side** is one 330-line `unsafe` adapter (`src/jolt.rs`), where every call has a `// SAFETY:` comment. Nothing else is unsafe:
  - u32 BodyIDs as handles;
  - no lifetimes or callbacks across the boundary;
  - no Rust panics reachable from C++, since the shim never calls back.
- **Jolt costs:** C++ compile errors surface through `build.rs`, and debugging spans two languages (the MSVC debugger handles mixed frames).

**ECS sync.** The harness already has the shape `ara-physics` would use:
- apply commands at the tick boundary (kinematic targets, character inputs, vehicle input);
- step;
- bulk-read state back.

Bulk extraction of all bodies, including conversion to glam, every 10th tick:
- 20k bodies: Jolt 0.42–0.62 ms, Rapier 0.14–0.24 ms;
- 5k bodies: 0.06–0.09 ms against 0.04 ms.

Production would write straight into ECS columns and only for awake bodies.

**FFI overhead.**
- An empty call through the shim is **1.1 ns**.
- A single body position read is 8.8 ns for Jolt (BodyID → body lookup) against 3.6 ns for Rapier (arena index).
- A bulk read costs 10.8 against 6.5 ns per body.
- Per-call cost is negligible next to the work. Jolt's lookup could be removed by iterating the body array in C++.

**Queries** (5,000 settled bodies, 10,000 downward casts from random points; identical hit counts and mean distances on both engines):

| | Jolt | Rapier |
|---|---|---|
| Raycast (closest hit + normal) | **0.55 µs** (0.46 µs in the AVX2 run) | 0.83 µs (0.97 µs AVX2 run) |
| Sphere cast r = 0.2 (closest) | **1.76 µs** (1.41 µs) | 7.27 µs (6.70 µs) |

Jolt narrow-phase queries return consistent hits, but multi-hit order can vary, and broad-phase queries are not deterministic. Sort results before simulation code uses them.

**Build time and binary size** (each probe is a minimal "box falls on ground" executable; release build, 4 jobs, crates already in the registry cache, under 31–45% background load):

| | Clean build | Touch rebuild | Exe size | Added over empty |
|---|---|---|---|---|
| Empty probe | 2.4 s | 0.9 s | 148 KB | — |
| Jolt shim (all of Jolt minus ObjectStream; linker drops unused code) | **55.5 s** | 0.7 s | 1.07 MB | **+0.92 MB** |
| Rapier 0.36.1 | 66.3 s | 1.5 s | 2.44 MB | +2.29 MB |

The Jolt C++ compiles once per profile and feature set and is cached afterwards. Agents' inner loop (`cargo check`) runs the build script once and never recompiles Jolt.

**Platform reach.**

| Target (D3) | Jolt via shim | Rapier |
|---|---|---|
| Windows x64 | **tested here** (MSVC 14.50) | tested here |
| Linux x64 / arm64 | upstream-supported; `build.rs` has the GCC/Clang flag path (untested) | pure Rust |
| macOS / iOS (Apple silicon) | upstream-supported (NEON path, no special flags); needs Xcode clang through `cc` (untested here) | pure Rust |
| Android arm64 | **compile-checked here**: all 145 Jolt sources plus the shim compile for `aarch64-linux-android24` with NDK r28 clang in deterministic mode, in 67 s at 4 jobs ([check-android.py](check-android.py), [evidence](evidence/android-compile.txt)). Not linked or run, since no Rust Android target is installed. | pure Rust |
| WASM (not a target) | upstream supports emscripten | pure Rust |

Jolt requires a C++17 toolchain for every target, and `cc` supplies it the way it already does for the other native dependencies in A11 (ufbx, meshopt).

**Maintenance health** (GitHub, 2026-10-23):

| | Jolt | Rapier |
|---|---|---|
| Stars / contributors / open issues | 11.8k / 93 / 12 | 5.9k / 78 / 80 |
| Last push | 2026-10-22 | 2026-10-10 |
| Releases | 5.4 (2025-09), 5.5 (2025-12), 5.6 (2026-07) | 0.x; minors break APIs (0.36 moved to glam, added PhysicsWorld) |
| Production use | *Horizon Forbidden West*, *Death Stranding 2*; Godot's built-in module pins this exact commit | Bevy ecosystem; Dimforge |
| Defects hit in this spike | none in Jolt | `PhysicsWorld::detect_collisions()` called before the first `step()` leaves every dynamic body frozen permanently; called after any step, it is fine. Repro: `cargo run --release --manifest-path probes/rapier-freeze/Cargo.toml` |

## Findings and gotchas

1. **Rapier 0.36.1 has a first-step freeze.** Calling `detect_collisions` before the first `step` freezes the simulation. Without that call, scene queries see an empty world until tick 1, so the character controller can't find the ground on tick 0.
2. **Rapier's KCC platform support is contact-gated.** It reports no ground body or ground velocity. The built-in kinematic friction applies only on sweep hits, so riders slip off. A position-based kinematic platform also exposes its velocity one tick late.
3. **Rapier's vehicle defaults roll a car.** There is no drivetrain model, `friction_slip` defaults to 10.5, and `roll_influence` is private.
4. **Jolt's default solver topples a 20-box tower.** Use 15 velocity steps; Rapier's defaults hold it.
5. **Jolt idle-on-slope behaviour is a `CharacterContactListener` policy.** Without it the character slides 0.215 m down a 35° slope in 4 s. Policies like this belong in the shim (C++), not across FFI.
6. **Jolt's `RestoreState` needs the identical body set and creation order.** Restoring into a freshly built world works when the world is built in the same order. Structural changes inside a rollback window must be replayed as commands. Characters are not in `PhysicsSystem::SaveState` and are saved separately (the shim does both).
7. **Some Jolt outputs are not deterministic:** broad-phase queries, `GetActiveBodies` order, and callback thread order (Jolt documents all three). Sort before use.
8. **Determinism depends on build flags.** In fast mode, MSVC `/fp:fast` vs `/fp:precise`, and Rust `target-cpu` (for Rapier), change results. Networked peers must run identically flagged builds, or both engines' deterministic modes.
9. **Scene construction is part of determinism.** This spike builds orientations with std trigonometry through glam. For cross-platform determinism, scene and asset setup must avoid platform libm too, by baking values or using libm/Jolt's `Sin`/`Cos`. This applies to both engines.
10. **Rapier snapshots are complete for free but heavy.** Serde of the whole `PhysicsWorld` is 7–9× Jolt's size, and restore is 20× or more slower. Jolt's `SaveState` also supports a filter (save only predicted islands), which suits R09's selective prediction.

## Recommendation

**Choose Jolt 5.6 for Ara's 3D physics, through Ara's own thin C shim (`ara-jolt-sys`).**

How to build it:
- Vendor Jolt at a pinned release commit.
- Compile it with the `cc` crate from `build.rs` (no CMake).
- Put all defines in one place, with a struct-layout test.
- Design the API around ECS batches and snapshot bytes.

Reasoning, in priority order for Outpost and R09:
1. **Character controller.** CharacterVirtual passed every lane, including both moving platforms, at 7× lower cost per character (1.4 µs against 10 µs). The predicted kinematic character is R09's launch-critical prediction target. Rapier's KCC loses riders on moving platforms with its documented API.
2. **Rollback contract.** Both engines are bit-exact for same-process, cross-process, snapshot/restore and thread-count invariance. Jolt's snapshots are 7–9× smaller and restore at least 20× faster, and they can be filtered per body. That makes per-tick save practical for predicted islands.
3. **Gameplay systems.** Out of the box, Jolt has a drivetrain vehicle, character-vs-character collision, ragdolls, soft bodies and a cross-platform deterministic mode. That fits "Unreal-level fidelity, Unity-level ease" without Ara building those layers on top.
4. **Performance parity.** Within ±15% of Rapier on throughput (interleaved A/B), with a tighter p95. Faster queries (1.5× rays, 4× sphere casts). Smaller binary (+0.9 MB against +2.3 MB). Comparable clean build (55 s against 66 s).
5. **Health.** Large contributor base, regular releases, AAA shipping titles, and Godot's built-in module on the exact commit tested here.

What Ara pays:
- a C++ toolchain per target;
- about 800 lines of FFI (C++ shim plus Rust declarations) to own;
- the 15-velocity-step setting;
- the "same body set" rule for restore.

Rapier remains the reference for pure-Rust behaviour. It is not part of the plan.

## Proposed `ara-physics` crate boundary

Three crates, split along the real dependency boundary (native code vs the engine contract):

```
crates/ara-jolt-sys      vendored Jolt (pinned rev) + Ara's C shim, built by build.rs with `cc`
crates/ara-physics       engine contract: components, CommandBatch, PhysicsBackend, ECS systems
crates/ara-physics-jolt  `impl PhysicsBackend for JoltBackend`; the only crate that touches -sys
```

Gameplay, the editor, netcode and C# bindings depend on `ara-physics` only.

- **The vocabulary is Ara's own schema.** It follows Jolt, the chosen backend, rather than an intersection of engines.
- **A backend that lacks a feature rejects the component at load with a diagnostic** (`supports(Feature)`). So a second backend never shrinks the API: it either implements the feature or says it doesn't.
- The seam is the integration contract (sync, step, read-back, queries, snapshot), not a re-export of a physics library.

```rust
//! ara-physics: Ara's physics contract. Simulation state lives in the backend and is mirrored
//! into ECS components at tick boundaries; every authoring change is a command (A4), applied
//! in a defined order so BodyIDs and results are deterministic.

/// How a body moves. Kinematic bodies follow targets set by gameplay; dynamic bodies simulate.
#[derive(Component, Reflect)]
pub enum RigidBody { Static, Kinematic, Dynamic { mass_kg: Option<f32> } }

/// Collision shape, physics material and layer. Mesh shapes reference cooked assets (`@path#id`).
#[derive(Component, Reflect)]
pub struct Collider { pub shape: ShapeRef, pub material: MaterialRef, pub layer: CollisionLayer }

/// Linear (m/s) and angular (rad/s) velocity; written back every tick for dynamic bodies.
#[derive(Component, Reflect)]
pub struct Velocity { pub linear: Vec3, pub angular: Vec3 }

/// Kinematic capsule character (Jolt CharacterVirtual): slopes, stairs, moving platforms.
#[derive(Component, Reflect)]
pub struct CharacterController { pub radius: f32, pub height: f32, pub max_slope_deg: f32, pub step_height: f32 }

/// Per-tick character intent. A network input, never an edit (A4, R09).
#[derive(Component, Reflect)]
pub struct CharacterInput { pub desired_velocity: Vec3, pub jump: bool }

/// Wheeled vehicle: wheels, engine, transmission, differentials, anti-roll bars.
#[derive(Component, Reflect)]
pub struct WheeledVehicle { pub wheels: SmallVec<[WheelSettings; 4]>, pub engine: EngineSettings /* , ... */ }

/// Runtime handle into the backend world. Not reflected, not serialized, not replicated.
#[derive(Component, Clone, Copy)]
pub struct BodyHandle(u32);

/// One tick of changes, built by `sync_in` from ECS change detection and sorted by entity id,
/// so creation order (and therefore BodyIDs) is identical on every peer and every replay.
#[derive(Default)]
pub struct CommandBatch {
    pub create: Vec<(Entity, BodyDesc)>,
    pub destroy: Vec<BodyHandle>,
    pub kinematic_targets: Vec<(BodyHandle, Pose)>,
    pub velocities: Vec<(BodyHandle, Velocity)>,
    pub impulses: Vec<(BodyHandle, Vec3, Vec3)>,
    pub character_inputs: Vec<(BodyHandle, CharacterInput)>,
    pub vehicle_inputs: Vec<(BodyHandle, VehicleInput)>,
}

/// The backend seam. Static dispatch: `PhysicsPlugin::<JoltBackend>` picks it when the app is
/// built. Batch-shaped, so a native backend costs one FFI call per batch, not per body.
pub trait PhysicsBackend: Send + 'static {
    fn new(config: &PhysicsConfig) -> Self;
    /// Authoring features this backend implements; unsupported components fail validation.
    fn supports(&self, feature: Feature) -> bool;
    /// Applies a tick's commands in order; returns handles for `create` in the same order.
    fn apply(&mut self, commands: &CommandBatch, created: &mut Vec<BodyHandle>);
    /// Advances one fixed tick (characters and vehicles included).
    fn step(&mut self, dt: f32);
    /// Bulk read for ECS write-back: poses, velocities and sleep state of `bodies`, in order.
    fn read_bodies(&self, bodies: &[BodyHandle], out: &mut BodyStates);
    fn read_characters(&self, characters: &[BodyHandle], out: &mut Vec<CharacterState>);
    /// Contact begin/end events of the last step, sorted by (body_a, body_b).
    fn contacts(&self, out: &mut Vec<ContactEvent>);
    /// Narrow-phase scene queries; multi-hit results sorted by (distance, body).
    fn cast_ray(&self, ray: Ray, filter: &QueryFilter) -> Option<RayHit>;
    fn cast_shape(&self, shape: &QueryShape, from: Pose, motion: Vec3, filter: &QueryFilter) -> Option<ShapeHit>;
    fn overlap(&self, shape: &QueryShape, at: Pose, filter: &QueryFilter, out: &mut Vec<BodyHandle>);
    /// Rollback / late-join contract: all state `step` reads that `apply` does not set
    /// (bodies, contacts, constraints, characters, vehicles). Requires the same structure.
    fn save(&self, filter: SnapshotFilter, out: &mut Vec<u8>);
    fn restore(&mut self, bytes: &[u8]) -> Result<(), RestoreError>;
}

/// FixedUpdate: sync_in::<B> (commands) -> step::<B> -> sync_out::<B> (poses, velocities,
/// character state into components) -> contact events. Rendering interpolates between the
/// last two ticks outside this crate.
pub struct PhysicsPlugin<B: PhysicsBackend>(PhantomData<B>);
```

Rules this boundary encodes:
- **ECS sync.** `sync_in` turns `Added`/`Changed`/`Removed` component changes into one sorted `CommandBatch`. `sync_out` reads awake bodies in one `read_bodies` call and writes `Transform` and `Velocity`. Gameplay never holds a backend pointer; it reads components and sends commands.
- **Determinism.**
  - Inputs arrive only at tick boundaries.
  - Contact events and query results are sorted.
  - Simulation code never uses broad-phase queries.
  - Active-body lists are sorted before use.
- **Rollback.** Predicted entities (the owner's character first, per R09) are restored from `save` bytes plus their ECS components. Structural changes inside the rollback window are replayed as commands.
- **Callbacks stay native.** Contact and character policies run inside the shim in C++. No per-contact callback crosses FFI.
- **C# sees components and commands**, never backend handles. The generated ABI (S2) wraps `ara-physics` types only.

## Proposed decision

For `docs/decisions.md`, Spike results table:

| Spike | Result | Decision | Evidence |
|---|---|---|---|
| S6 | **PASS** (2026-10-23) | **Adopt Jolt 5.6 through Ara's own thin C shim (`ara-jolt-sys`)**: vendored at a pinned release commit, built with `cc` from `build.rs` (no CMake, no prebuilt downloads), behind `ara-physics`' batch-shaped `PhysicsBackend`. Rapier is not adopted.<br>• Character: CharacterVirtual passes all 7 lanes, including moving platforms, at 1.4 µs per character. Rapier's KCC fails both platform lanes (10 µs).<br>• Vehicle: Jolt's VehicleConstraint (engine, gearbox, differential) passes on defaults.<br>• Scale: within ±15% of Rapier at 5k/20k bodies, 1–8 threads (interleaved A/B), with tighter p95. Queries are 1.5–4× faster. The binary is +0.9 MB.<br>• Determinism: same process, cross process, snapshot/restore and 1/4/8-thread invariance are all bit-exact. Snapshots are 455 KB against 4.2 MB, and restore takes 0.3 ms against 7 ms.<br>• Rules: 15 velocity steps (the default 10 topples a 20-box tower); contact and character policies live in C++ in the shim; commands are applied sorted at tick boundaries; `JPH_CROSS_PLATFORM_DETERMINISTIC` for networked and replay builds (measured cost 7–12%); no broad-phase queries in simulation code.<br>Still open: running on Linux, macOS, iOS and Android (Android arm64 is compile-checked only), and cross-platform hash equality (needs the MacBook). | [`spikes/s6-physics/RESULT.md`](../spikes/s6-physics/RESULT.md) |

The open item "Physics: Jolt (via a binding we own) vs Rapier. Lean: Jolt (S6)" would then be struck through as decided. R07 §8's default to Rapier is superseded by this result. A11's reuse list would gain Jolt.
