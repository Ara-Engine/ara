# R06: Stack, reuse, licensing and strategy

*Research track R06 for snengine. Data gathered 2026-10-22/23. Library versions, licenses and push dates come from the GitHub API on 2026-10-22 unless noted otherwise. Line counts are raw `wc -l` totals (including comments and blanks) over the `.refs/` clones.*

---

## TL;DR

- **Core: C++20, plus the parts of C++23 that MSVC and Clang both support. No modules and no exceptions.** Every library we want to reuse is C or C++: graphics APIs, console SDKs, Jolt, Slang, DXC, V8, HarfBuzz. Rust gives agents no measurable advantage. On Multi-SWE-bench, Rust and C++ resolve rates are similarly low. Rust's engines are still pre-1.0 or only just reached it: Bevy 0.20 has no editor after about six years, and Fyrox took seven years to reach 1.0. Zig (0.16) still breaks between releases, and that churn poisons agents' training priors.
- **Primary scripting: TypeScript on embedded V8.** V8 runs with its JIT on desktop and Android and in jitless mode on iOS and consoles. TS is GitHub's #1 language (Octoverse 2025). It gives millisecond hot reload and real sandboxing (isolates, heap limits, terminate-on-runaway), which matters when agents are writing and running code inside the editor. It also uses the same language as game UI authoring (TSX), editor extensions and our tooling. **Main cost: performance on platforms without a JIT.** Mitigations: hot loops live in C++ native components, the scripting API is batched, and a benchmark gate in month 2 decides whether to stay. **C# (.NET 10) is plan B.** Because bindings are generated from a type registry, adding it later is mechanical.
- **Build: CMake 4.x + Ninja + presets, wrapped in one dev CLI (`snen`, TypeScript on Bun).** Source dependencies are vendored Godot-style with a manifest. A few heavy SDKs (V8, Slang, DXC, D3D12 Agility) come as pinned prebuilt binaries with SHA-256 hashes. No vcpkg or Conan until it actually hurts.
- **Reuse aggressively; write only the integration and differentiation layers.** Jolt, Slang, SDL3, NVRHI (initially), ufbx/fastgltf/meshoptimizer/KTX2, ozz, Recast, miniaudio + Steam Audio, Yoga + HarfBuzz + FreeType + msdfgen, flecs, Tracy, GameNetworkingSockets, OpenXR, zstd. All are permissive and actively maintained in 2026.
- **Licensing rules:** Godot (MIT) and O3DE (Apache-2.0 or MIT) may be copied with attribution. Flax (EULA) and UnityCsReference ("Reference Purposes only": understanding Unity to improve *your Unity projects*) may not be copied. That means no copying code and no pasting their code into agent context to port it. Unreal: no source ever. Avoid LGPL in the runtime (Qt, JavaScriptCore/Bun).
- **Strategy: build our own core from scratch. Don't fork, and don't build a layer on someone else's engine.** Godot now bans contributions from autonomous agents and vibe coding (June 30, 2026), so a fork could never send changes upstream. O3DE is about 3.2M lines of complexity. Unity (Unity AI open beta and MCP server, May 2026) and Unreal (5.8 MCP plugin, June 2026) already ship "AI via MCP", so being AI-native has to be *structural*: headless-first, everything is data plus commands, deterministic, introspectable.
- **Be honest about scope.** In 24 months, 2–4 people plus agents can ship a solid PC engine with a real editor and 3–4 standout Unreal-class rendering features. They cannot reach UE parity. Agents compress implementation time but not design or verification time. Anthropic's 16-agent compiler worked because it had a near-perfect oracle, and an engine needs oracles built on purpose: golden images, deterministic replays, headless tests.
- **License the engine Apache-2.0 OR MIT, with no royalties and no runtime fees.** Earn money from optional services: hosted agents, a build/test farm, asset generation, support, console middleware via a partner. The Unity runtime fee showed that the license itself is the trust product.

---

## Findings

### 1. Core language

#### 1.1 Evidence

| Factor | C++20/23 | Rust | Zig |
|---|---|---|---|
| Reusable ecosystem | Everything in §4 is C/C++. Console SDKs and graphics API headers are C/C++. | wgpu, Rapier and Bevy are good, but C++ libraries need FFI and consoles are a struggle | Uses C libraries through C import, but has a tiny game ecosystem. Mach (Zig engine) last pushed May 2026 with no releases. |
| Agent quality | 90–92% compile success for AI-written C++ vs 98.2% for humans ([arXiv 2607.00107](https://arxiv.org/pdf/2607.00107)). Multi-SWE-bench C++ resolve rates are similar to Rust. | Lowest first-try pass rate of the languages studied (pass@1 0.30 → 0.46 after compiler feedback, [ScienceDirect](https://www.sciencedirect.com/science/article/pii/S0925231225021332)). The compiler is a strong feedback loop but costs extra iterations. Anthropic's agents wrote a 100k-line Rust C compiler, but std-only with a perfect oracle ([Anthropic](https://anthropic.com/engineering/building-c-compiler)). | Pre-1.0 (0.16, Apr 2026) and breaks every release ([JetBrains](https://blog.jetbrains.com/blog/2026/06/05/why-zig-isn-t-1-0-yet/)). Models write stale APIs: the same failure as Godot 3→4 GDScript drift ([summerengine](https://www.summerengine.com/blog/best-ai-for-gdscript)). |
| Iteration speed | Slow only if neglected. Unity builds, PCH, DLL-per-module in dev, and clang-cl + lld keep it fast. | Slow incremental builds and link times. Hot patching (Subsecond) is experimental and Bevy's integration is still a proposal ([Bevy design doc](https://hackmd.io/@bevy/SkdMQIYWle)). | Fast and improving (build system overhaul in 2026) |
| Hot reload | DLL module reload (The Machinery, UE, Flax). Live++ (commercial; powers UE Live Coding). | Experimental | None mature |
| Safety | Weak by default. Mitigate with ASan/UBSan in CI, hardened standard library modes, clang-tidy, a strict subset. | Best in class | Better than C, safety checks optional |
| Agent-friendly engine patterns (graphs, callbacks, hot reload, ECS with aliasing) | Natural | Frequent fights with the borrow checker; ECS frameworks hide this | Natural |

The people who backed Rust hardest for AAA didn't bet their games on it: Embark, the biggest AAA Rust champion, shipped *The Finals* and *Arc Raiders* on Unreal Engine 5. *(Widely known; not re-verified this session.)*

**C++ standard status (Oct 2026).** C++26 static reflection (P2996) is implemented only in **GCC 16** (experimental, `-freflection`). Upstream Clang doesn't have it (only Bloomberg's fork), and MSVC has no public ETA ([JetBrains ReSharper C++ 2026.2](https://blog.jetbrains.com/rscpp/2026/07/22/resharper-cpp-2026-2-cpp-26-reflection-ispc-language-support-and-more/), [Wikipedia C++26](https://en.wikipedia.org/wiki/C%2B%2B26)). Reflection is therefore **not usable on Windows yet**, so use registration code and codegen for now (§7). `import std` in CMake is still behind an experimental gate, works only with Ninja, and IDE completion is unreliable ([cmake-cxxmodules](https://www.mankier.com/7/cmake-cxxmodules)). **Skip modules.**

#### 1.2 How the reference engines did it

| Engine | Language | Size (raw lines) | Who builds it |
|---|---|---|---|
| Godot 4.7.2 | C++ (+GDScript, C#) | ~1.65M engine + 2.85M vendored third-party (`.refs/godot`) | Hundreds of contributors, about 8.7k commits in the last 52 weeks |
| O3DE 26.05 | C++ (+Lua, Python, Qt) | ~3.2M across `Code/`+`Gems/` (`.refs/o3de`) | Linux Foundation. Downloads +88% in 2025 ([O3DE](https://o3de.org/o3de-2025-in-review-and-a-look-into-2026/)), but few shipped games |
| Flax 1.13 | C++ + C# | ~0.72M Engine+Editor (`.refs/flax`) | Led by one person: top contributor 10,892 commits vs 904 for the next |
| Wicked Engine 0.72 | C++ | — | One person: 4,620 commits vs 163 for the next |
| Bevy 0.20 (Oct 8, 2026) | Rust | — | Distributed (227 contributors per release), still no editor ([Bevy 0.20](https://bevy.org/news/bevy-0-20/)) |
| Fyrox 1.0 (Mar 2026) | Rust | — | One person (7,795 commits), 7 years to 1.0 ([Fyrox](https://fyrox.rs/blog/post/fyrox-game-engine-1-0-0/)) |
| The Machinery (2018–22) | C (plugin API, hot reload) | — | Small company, shut down (§6) |

**Conclusion:** C++ is the only language where the ecosystem, consoles, agents' competence and hiring all point the same way. The C++ problems (safety, build speed) are engineering problems with known fixes.

### 2. Gameplay scripting language

#### 2.1 Comparison (A = best)

| | C# (.NET 10) | **TypeScript (V8)** | Luau | Lua/LuaJIT | Python | AngelScript | Verse | Rust + hot patch | Wasm runtime |
|---|---|---|---|---|---|---|---|---|---|
| Agents write it well | A (huge Unity gameplay corpus) | **A+** (#1 on GitHub, [Octoverse 2025](https://github.blog/news-insights/octoverse/octoverse-a-new-developer-joins-github-every-second-as-ai-leads-typescript-to-1/)) | B | B (Lua 5.1 dialect) | A+ | C (tiny corpus) | D (tiny corpus, proprietary) | B (more loops) | depends on guest language |
| Human ergonomics (our audience) | A for Unity devs | **A** for web devs; Unity devs adapt quickly (same designer) | B | B− | B | B (C++-like) | C | C | — |
| Hot reload | B (collectible ALC reload ~1–3 s; s&box gets ms with custom hotload, [sbox docs](https://sbox.game/dev/doc/getting-started/)) | **A** (ms module swap) | A | A | A− | A (Hazelight reloads even mid-PIE, [Hazelight](https://angelscript.hazelight.se/)) | ? | C (experimental) | B (re-instantiate) |
| Perf, desktop | A (tiered JIT) | **B+** (JIT; interop cost) | B (fast interpreter + x64/arm64 codegen) | A− | D | B, or about native when transpiled | ? | A+ | A− (AOT) |
| Perf, no-JIT platforms (iOS, consoles) | A (NativeAOT / Mono AOT; Flax.Build has ILC and Mono AOT modes, `.refs/flax/Source/Tools/Flax.Build/Build/DotNet/DotNetAOT.cs:15`) | **C** (jitless V8: Speedometer −40%, Web Tooling −80% vs JIT, [v8.dev](https://v8.dev/blog/jitless)) | B | C | D | B | ? | A+ | B+ |
| Sandboxing (UGC, agent-written code) | D (no AppDomains; build your own IL whitelist like [SS14](https://docs.spacestation14.com/en/robust-toolbox/sandboxing.html); [s&box editor unsandboxed](https://sbox.game/f4industries/secbox)) | **A** (isolates, heap caps, `TerminateExecution`) | A (designed for it) | B | D | B | A | D | A+ |
| Debugging | A (VS/Rider/VS Code) | **A−** (V8 inspector → VS Code/Chrome) | B | C | A | B (VS Code extension) | B | A | C |
| Static types | A | **A−** (strict mode; types erased) | B (gradual) | D | C | B | A | A+ | — |
| Embedding effort | B (hostfxr + AOT pipeline) | **C** (V8 is heavy to build: use pinned prebuilts) | A | A | C | A | n/a | B | B |
| License | MIT | BSD-3 (V8) | MIT | MIT | PSF | zlib | proprietary | MIT/Apache | Apache-2.0 + LLVM exception |

#### 2.2 Evidence for each option

- **C# is proven at scale:** Unity, Godot, Flax, Stride, s&box. s&box launched Apr 28, 2026, with its C# layer MIT-licensed on Source 2 ([Wikipedia](https://en.wikipedia.org/wiki/S%26box), [GameFromScratch](https://gamefromscratch.com/sbox-game-engine-open-sourced/)). Unity is moving to CoreCLR/.NET 10 in 6.8, and domain reloads go away by 7.0 ([Unity Discussions](https://discussions.unity.com/t/coreclr-scripting-and-serialization-update-june-2026/1723299)). Godot 4 C# still can't export to web, and mobile is experimental ([Godot docs](https://docs.godotengine.org/en/stable/tutorials/scripting/c_sharp/index.html)). Sandboxing is do-it-yourself.
- **TypeScript:** TS 7.0 (Go-native compiler) shipped July 2026 with roughly 10× faster type checking ([InfoQ](https://infoq.com/news/2026/08/typescript-7-released/)). That speeds up the agent's typecheck loop. Puerts (BSD-3) shows TS on Unity/Unreal with V8 or QuickJS backends and generated declarations ([Puerts](https://github.com/Tencent/puerts)). I could not verify any shipped Puerts titles. Static Hermes, which would compile typed JS to native code, **has not shipped**. Hermes V1 is an engine upgrade without AOT-to-native ([Software Mansion](https://swmansion.com/blog/welcoming-the-next-generation-of-hermes-67ab5679e184)). QuickJS-ng (MIT, 0.17) is easy to embed but much slower and has weak debugging (Puerts notes no QuickJS debugging). Bun/JavaScriptCore **statically links LGPL-2 WebKit**, which is a non-starter for a runtime shipped on consoles (`oven-sh/bun` LICENSE.md).
- **Luau** (MIT, 0.743): an excellent sandbox with a fast interpreter and gradual types, proven by Roblox. It loses here because of the smaller corpus and weaker types, and because it has no synergy with UI authoring.
- **AngelScript:** *Split Fiction* has about 1.7M lines of it, shipping on PS5 and Xbox ([Hazelight](https://angelscript.hazelight.se/)). It is strong proof that a custom script layer beats Blueprint/C++ for iteration, but agents barely know the language.
- **Verse:** Epic only. It becomes UE6's primary gameplay language, with UE6 early access at the end of 2027 ([Strafe](https://www.strafe.com/news/read/state-of-unreal-2026-all-the-key-takeaways-from-the-epic-keynote/)). Not available to us.
- **Wasm as the primary runtime:** the game-specific ecosystem is thin. Wasvy is 0.0.x ([docs.rs](https://docs.rs/crate/wasvy/0.0.4)), and Ambient (Rust + Wasm engine) has paused indefinitely. Keep it as a possible future sandbox for user mods with native-speed code, not as the primary runtime.
- **Agent quality depends on API stability:** LLMs regularly write Godot 3 code for Godot 4. **Whatever we pick, the API must be stable, versioned, and served to agents as machine-readable typings.**

#### 2.3 Recommendation: TypeScript as the one high-level language

The engine is meant to be driven by agents and to share one language across gameplay, UI, editor and tools. That weighting favors TS on most criteria: agent fluency, hot reload, sandboxing, and synergy with UI/editor. C# wins only on raw performance and Unity familiarity. Design rules that limit the performance risk:
1. Persistent gameplay state lives in engine-side components (serializable and inspectable), not in script heap objects. Hot reload, save/load and agent introspection then all use the same mechanism.
2. The script API is batched (queries over components, typed arrays) and uses V8 Fast API calls, so we never make per-entity, per-frame calls into C++.
3. C++ native components and systems, hot-reloadable as DLLs, are the official escape hatch for heavy loops.
4. **Month-2 benchmark gate:** 10k entities with script-driven update and events, run in V8 JIT and jitless. If the jitless p95 exceeds about 2 ms per frame on typical gameplay, adopt plan B: C# through the same binding generator.

### 3. Build system and tooling

| Engine | Build | Dependencies | Notes |
|---|---|---|---|
| Godot | SCons (Python), optional single-compilation-unit builds (`.refs/godot/SConstruct:311`) | Vendored in `thirdparty/` with a README listing upstream, version and license (`.refs/godot/thirdparty/README.md`) | Simple and deterministic; agents can grep dependency sources |
| Flax | Flax.Build (C#). Parses `API_CLASS`/`API_FUNCTION` macros to generate C# bindings (`.refs/flax/Source/Tools/Flax.Build/Bindings/`, `.refs/flax/Source/Engine/Physics/Actors/RigidBody.h:17`) | Vendored in `Source/ThirdParty` | Ships an `AGENTS.md` (`.refs/flax/AGENTS.md`). Console code is NDA and kept out of the repo. |
| O3DE | CMake + presets, 3p package system | Prebuilt 3p packages | Powerful but heavy |
| Unreal | UBT (C#) | Vendored | Unreal-only |
| Bevy | cargo | crates.io | Excellent dependency management, but Rust only |

Current tool versions: CMake 4.4.4, xmake 3.1.2, Premake 5.0.0 (finally stable, Oct 2026), vcpkg 2026.10.16, Conan 2.33, CPM.cmake 0.43.2.

**Recommendation:**
- **CMake + Ninja + `CMakePresets.json`.** Agents know CMake best, and every library already supports it. xmake and Premake are nicer but have much less training data, which means more agent mistakes.
- **Dependencies:** vendor sources Godot-style in `third_party/<lib>/` with a `third_party/MANIFEST.toml` (upstream URL, commit, license SPDX, local patches). Agents can read, grep and debug them offline. Heavy SDKs that are painful to build (V8, Slang, DXC, D3D12 Agility SDK, optionally PhysX GPU) come as pinned prebuilt archives in a `deps.lock.json` with SHA-256 hashes. **No vcpkg or Conan** (YAGNI); add them only if the dependency count explodes.
- **Speed:**
  - Dev configuration builds each module as a shared library, which gives fast relinks and DLL hot reload. Ship builds are monolithic static with LTO.
  - Unity builds for third-party code, PCH per module, clang-cl + lld-link on Windows, and sccache locally and in CI.
  - Targets: incremental rebuild of one module plus relaunch of the headless engine in ≤10 s; clean build ≤10 min on 16 cores.
- **One command:** `snen setup && snen build && snen test`. `snen` is a TypeScript CLI on Bun (1.4.x). It is the single entry point for humans and agents: setup, build, test, run headless, golden tests, codegen, format, package. It is dev-only and never shipped, so Bun's LGPL JSC doesn't matter.

### 4. Reuse instead of build

Health data is as of 2026-10-22 (latest release / last push). ✅ = recommend, ◐ = optional or later, ✗ = avoid.

| Area | Library | License | Health | Verdict |
|---|---|---|---|---|
| Physics 3D | **Jolt** | MIT | v5.6.0 (Jul 2026); used in *Horizon Forbidden West* and *Death Stranding 2*; Godot module (`.refs/godot/modules/jolt_physics`) | ✅ default |
| Physics 3D | PhysX 5 | BSD-3, including GPU source since 5.6 ([CG Channel](https://www.cgchannel.com/2025/04/nvidia-open-sources-physxs-gpu-simulation-code)) | Active (Oct 2026) | ◐ only for GPU cloth/particles (CUDA = NVIDIA only) |
| Physics 2D | **Box2D v3** | MIT | v3.2.0 (Oct 2026) | ✅ |
| Physics | Rapier | Apache-2.0 | Rust only | ✗ (FFI overhead) |
| Shaders | **Slang** | Apache-2.0 + LLVM exception | v2026.20; governed by Khronos ([Khronos](https://www.khronos.org/news/press/khronos-group-launches-slang-initiative-hosting-open-source-compiler-contributed-by-nvidia)) | ✅ shader and material-graph target language (modules, generics, autodiff) |
| Shaders | DXC | NCSA (LLVM) | v1.9.2609 (Sep 2026) | ✅ DXIL backend |
| Shaders | SPIRV-Cross | Apache-2.0 | Active, no recent tags | ◐ reflection/Metal fallback |
| RHI | **NVRHI** | MIT | Active (Oct 2026), no tags. D3D11/12 + Vulkan 1.3; RT and meshlet pipelines; used in NVIDIA RTX SDKs | ✅ Phase 1 under our render graph; no Metal or consoles |
| RHI | Diligent | Apache-2.0 | v2.5.6 (2024) + active. Metal on iOS/tvOS was historically under a commercial license | ◐ |
| RHI | The Forge | Apache-2.0 | v1.63 (2025) | ◐ reference; console ports are commercial |
| RHI | wgpu / Dawn | MIT/Apache / BSD-3 | v30 / daily. RT and mesh shaders still experimental | ◐ for a future web target only |
| RHI | SDL_GPU | zlib | Says itself it is not for cutting-edge features ([SDL wiki](https://wiki.libsdl.org/SDL3/CategoryGPU)) | ✗ for high fidelity |
| GPU memory | D3D12MA / VMA | MIT | v3.2 / v3.4 (Jun 2026) | ✅ |
| Upscalers | FidelityFX SDK; Streamline (DLSS) | MIT (verify); MIT wrapper + NVIDIA binary terms | v2.3 / v2.14 | ✅ check redistribution terms |
| Windowing/input | **SDL3** | zlib | 3.4.18 (Oct 2026) | ✅ (gamepads, mobile, dialogs) |
| Windowing | GLFW | zlib | 3.5.1 | ✗ (no gamepad DB or mobile) |
| Import | **ufbx** | MIT / public domain | v0.23.1 | ✅ FBX |
| Import | **fastgltf** (or cgltf) | MIT | v0.9.1 (Sep 2026) / v1.15 | ✅ glTF |
| Import | Assimp | BSD-3 (contrib parts vary) | v6.0.5 | ◐ exotic formats only, in an out-of-process importer |
| Import | OpenUSD | TOST-1.0 (Apache-2.0 with a modified trademark clause) | v26.11.1 | ◐ importer from Phase 3, not the runtime format |
| Mesh | **meshoptimizer** | MIT | v1.3 (Sep 2026) | ✅ (meshlets, LOD, compression) |
| Textures | **basis_universal + KTX-Software** | Apache-2.0 | v2.50 / v4.4.2 | ✅ |
| Animation | **ozz-animation** | MIT | 0.17.0 (Aug 2026) | ✅ runtime sampling and blending |
| Animation | ACL | MIT | v2.1.0 (2023); commits through Nov 2025 | ◐ compression; slow-moving |
| Animation | Esoterica | MIT | Active | ◐ reference for animation graphs |
| Navigation | **Recast/Detour** | zlib | v1.6.0 (2023), pushes in 2026; vendored by Godot | ✅ (stable and slow-moving) |
| Audio | **miniaudio** | public domain / MIT-0 | 0.11.25 | ✅ core |
| Audio | **Steam Audio** | Apache-2.0 | v4.8.1 (Feb 2026) | ✅ spatial/HRTF |
| Audio | SoLoud | zlib | last release 2020 | ✗ stale |
| Audio | FMOD / Wwise | Commercial (indie tiers: Wwise <$250k budget, [Audiokinetic](https://www.audiokinetic.com/en/blog/free-wwise-indie-license)) | — | ◐ user-licensed plugins only |
| UI layout | **Yoga** | MIT | v3.2.1 (2024), active | ✅ flexbox in C++ |
| UI layout | Taffy / Clay | MIT / zlib | v0.15 / v0.14 | ✗ (Rust) / ◐ debug UI |
| Text | **HarfBuzz** | Old MIT | 15.0.0 (Oct 22, 2026) | ✅ |
| Text | **FreeType** | FTL (requires credit) or GPLv2 | Active | ✅ choose FTL and credit it |
| Text | **msdfgen** | MIT | v1.13 | ✅ |
| Debug UI | Dear ImGui | MIT | v1.93 (Oct 2026) | ✅ debug overlays only |
| ECS | **flecs** | MIT | v4.1.6 | ✅ built-in reflection, JSON, REST explorer (introspectable by agents) |
| ECS | EnTT | MIT | v4.0.0 (Jul 2026) | ◐ leaner, but we'd build more ourselves |
| Profiling | **Tracy** | BSD-3 | v0.14.1 | ✅ built in |
| Profiling | Superluminal | Commercial | — | ◐ developer tool, not shipped |
| Networking | **GameNetworkingSockets** | BSD-3 | v1.6.1 (Oct 2026) | ✅ transport |
| Networking | yojimbo / ENet | BSD-3 / MIT | v1.14 (Oct 2026) / 1.3.18 | ◐ |
| XR | **OpenXR SDK** | Apache-2.0 | 1.1.65 (Oct 2026) | ◐ Phase 3+ |
| Compression | **zstd** (+LZ4) | BSD (dual with GPLv2: choose BSD) | v1.5.7 | ✅ |
| Compression | Oodle | Commercial (Epic/RAD) | Free only inside UE ([Epic](https://www.unrealengine.com/en-US/blog/oodle-now-free-to-use-in-unreal-engine-via-github)) | ◐ optional plugin |
| Scripting | **V8** | BSD-3 | Active | ✅ |
| Scripting | QuickJS-ng / Hermes / Luau | MIT | 0.17 / active / 0.743 | ◐ fallback runtimes |
| Allocator | mimalloc | MIT | v3.5.3 | ✅ |
| Crash reporting | sentry-native | MIT | 0.17.4 | ✅ |

**License flags for a commercial engine:**
- **Qt:** LGPL-3, as used by O3DE (`.refs/o3de/LICENSE.txt`). Avoid it for the editor.
- **JavaScriptCore/Bun:** LGPL-2 with static linking. Never put it in the runtime.
- **FreeType:** the FTL license requires a credit line.
- **zstd:** pick the BSD side of the dual license.
- **Assimp contrib/ code:** audit it before use.
- **Commercial middleware (FMOD, Wwise, Oodle, DLSS binaries, Superluminal, Live++):** offer them only as optional integrations that users license themselves. Never bundle them in the open-source repo.
- **Console SDKs:** NDA. They belong in private platform repos, as Flax does.

### 5. Licensing of the reference engines

| Engine | License (verified) | Can we copy code? | Rule for the team and agents |
|---|---|---|---|
| Godot | MIT (`.refs/godot/LICENSE.txt`) | **Yes**, with the copyright and license notice | Keep the original header plus an entry in `THIRD_PARTY_NOTICES.md`. Vendored libraries inside Godot keep their own licenses (check `thirdparty/README.md`). |
| O3DE | Apache-2.0 by default, or MIT at your option. Some components are copyleft, e.g. Qt LGPL (`.refs/o3de/LICENSE.txt`). | **Yes**: choose MIT, or Apache with NOTICE and change markings | Never copy anything that touches Qt or GPL third-party code |
| Flax | Proprietary EULA; "Use of the… source code… is governed by the Flax EULA" (`.refs/flax/LICENSE.md`). 4% royalty above $250k per quarter per game ([Flax](https://flaxengine.com/licensing)) | **No** (I didn't review the EULA PDF; assume reuse is allowed only in Flax-based products) | Read for ideas only. Never paste Flax code into an agent prompt to port it. |
| UnityCsReference | Unity Reference-Only License v1.1: "Reference Purposes" means "the sole purpose of inspecting functionality to understand or improve performance of your [Unity] games"; "no other distribution or modification" ([Unity](https://unity.com/legal/licenses/unity-reference-only-license)) | **No.** Using it to design a competing engine is arguably outside the license's purpose. | Use public Unity *docs* for API-shape familiarity, written in our own words. Don't let agents read this clone for implementation tasks. |
| Unreal | EULA; source only for licensees | **No** | No UE source in the repo or in agent context. Public papers and talks (Nanite, Lumen at SIGGRAPH) are fine for ideas. |

**Practical rules:**
1. Every file copied from Godot or O3DE keeps its header, and CI checks the notices file.
2. Run license and similarity scanning in CI (e.g. ScanCode). GameLogicBench saw agents **copying public repository code when network access was open** ([arXiv 2609.21562](https://arxiv.org/abs/2609.21562)).
3. Implementation agents run without network access by default and get only permissive references.
4. Record ideas taken from proprietary engines as ADRs with citations to public talks, never to source.

### 6. Strategy: from scratch, fork, or layer

#### 6.1 Projects that stalled vs projects that succeeded

| Project | Outcome | Lesson |
|---|---|---|
| Lumberyard → O3DE | Lumberyard required AWS for cloud services ([The Register](https://www.theregister.com/2021/07/07/open_3d_engine)), was donated to the Linux Foundation in 2021 and is no longer offered. O3DE: downloads +88% in 2025, but a 3.2M-line codebase and few shipped games. | A corporate strategic engine dies when the strategy changes. Size and complexity kill onboarding. |
| The Machinery (2018–22) | Shut down. Licenses were terminated after 14 days and users were asked to delete source and binaries ([GameWorldObserver](https://gameworldobserver.com/2022/08/01/our-machinery-terminates-machinery-game-engine-changes-eula)) | A proprietary license from a tiny company is an existential risk for users. A permissive license is the insurance. |
| Stride (Xenko, open-sourced 2018) | Alive, but slow: 4.4.x with .NET 10, small team | Open-sourcing an engine doesn't create a community |
| Ambient (Rust + Wasm) | Paused indefinitely | A novel runtime bet plus venture funding is fragile |
| Bevy | Huge community (49k stars) and still pre-1.0 | A clean modular core attracts contributors, but **the editor is the hardest part** |
| Godot | 4.7 (Jun 2026). Steam releases went from 56 (2019) to about 1,229 (2025), roughly 5% of Steam releases in 2024 ([wnhub](https://wnhub.io/news/analytics/item-50801)). Community doubled in two years ([Godot](https://godotengine.org/article/godot-growth-stats-2026/)) | Permissive license + ease of use + a trust shock at a competitor. Growth took 10+ years. |
| Flax / Wicked / Fyrox | One lead developer each, high quality, 7–10 years | One focused person can build an engine. Agents should compress the timeline, but verification doesn't compress. |
| s&box | Excellent C# and hot-reload experience layered on Source 2 | Layering works if you have a top-tier runtime license, which we don't |

#### 6.2 Options for snengine

- **Fork Godot.** You get breadth on day 1. But Godot's **June 30, 2026 policy bans autonomous agents and substantial AI-generated code** ([Game Developer](https://www.gamedeveloper.com/business/godot-to-ban-almost-all-ai-coding-contributions)), so an agent-built fork could never upstream anything. It would drift away from about 8.7k upstream commits a year, and the renderer and editor architecture would cap fidelity and UX. Bevy also restricts LLM-made pull requests ([Bevy](https://bevy.org/learn/contribute/policies/ai/)). **No.**
- **Fork O3DE:** 3.2M lines and Qt, the opposite of "complex things built simply". **No.**
- **Bevy, Stride or Wicked as the base:** the wrong language, or churn, or one developer's architecture. **Use Wicked as an MIT reference** for a lean bindless renderer. **No** to using any of them as the base.
- **Layered: AI editor/tooling on someone else's runtime.** Unity AI (open beta and MCP, May 2026; [Unity](https://unity.com/en/blog/unity-ai-mcp-how-to-get-started)) and UE 5.8's first-party MCP plugin ([Strafe](https://www.strafe.com/news/read/state-of-unreal-2026-all-the-key-takeaways-from-the-epic-keynote/)) already occupy that space, and a layer can't reach pillars 2 to 4. **No.**
- **Our own core, assembled from reused libraries: recommended.** We write the kernel (type registry, serialization, jobs), the scene/ECS integration, the asset database and pipeline, the render graph and renderer features, the editor, the script bindings and the AI layer. Everything in §4 is reused.

#### 6.3 What 2–4 people plus agents can realistically build

- **6 months:** headless runtime, a solid baseline 3D renderer, physics, import, TS hot reload, CLI/MCP, and an editor v0 shell. One small 3D game built end to end by an agent.
- **12 months:** editor v1, material graph, UI authoring, animation, audio, nav, Windows/Linux packaging, public alpha.
- **24 months:** a PC 1.0 with 3–4 standout high-fidelity features (§Roadmap). **Not realistic in 24 months:** UE-wide parity (Nanite + Lumen + MetaHuman + World Partition-scale streaming), consoles in-house, or mobile parity.

The constraint is the oracle. Anthropic's compiler worked because GCC and its torture tests gave a near-perfect verifier. Without one, "Claude will solve the wrong problem", and the code is "nowhere near the quality of what an expert Rust programmer might produce" ([Anthropic](https://anthropic.com/engineering/building-c-compiler)). Engines need their verifiers built on purpose.

### 7. Making the engine's own codebase AI-friendly

Evidence: Anthropic's harness lessons (near-perfect tests, terse output with grep-able `ERROR` lines, a `--fast` sampled test mode, lock files for task claiming, progress and failed-attempts docs, strict CI). Flax's `AGENTS.md`, which names exact build and test commands and tells agents to build the narrowest target. Godot's `--dump-extension-api`, which emits the whole engine API as JSON (`.refs/godot/main/main.cpp:672`, `.refs/godot/core/extension/extension_api_dump.cpp`).

Conventions:
1. **Repo layout**
   ```
   engine/<module>/{include,src,tests,AGENTS.md}
   editor/
   runtime-ts/
   tools/snen/
   third_party/ (+MANIFEST.toml)
   tests/golden/
   docs/{adr,api}/
   ```
   The root `AGENTS.md` (with a CLAUDE.md pointer) holds only commands, invariants and where things live. Each module's `AGENTS.md` is ≤150 lines: purpose, public API, how to test, gotchas.
2. **One source of truth for types.** C++ registers types, components and functions in a type registry (Godot ClassDB style; flecs reflection can back it). `snen gen` dumps `api.json` and generates from it:
   - TS `.d.ts` files and bindings
   - the serialization schema
   - editor inspector metadata
   - MCP/JSON-Schema tool definitions
   - API docs

   Nothing is maintained by hand twice, and C++26 reflection can replace the registration boilerplate once MSVC supports it.
3. **Fast, terse, deterministic feedback.**
   - `snen test <module>` runs in under 30 s.
   - The headless engine boots in under 1 s.
   - Golden-image tests compare with perceptual diff (FLIP).
   - Deterministic simulation replays act as physics and gameplay oracles.
   - Importers are fuzzed.
   - ASan/UBSan run in CI.
   - Output is a one-line summary plus log files.
4. **Prefer boring, well-known APIs.** Use `std::` containers with custom allocators instead of a homegrown STL, because every bespoke API is something agents must learn from our docs. On the script side, follow Unity's naming where it fits, to exploit what models already know, but make typings the authority and expose a docs-lookup MCP tool so agents don't hallucinate Unity APIs.
5. **Size limits:** files under about 800 lines, modules under about 40k lines behind one public header set, no clever template metaprogramming in public APIs, macros only for registration. Use clang-format, a clang-tidy profile and `compile_commands.json` for clangd.
6. **Process:**
   - ADRs for every architectural decision.
   - Task files with claim locks for parallel agents.
   - A strict-CI merge gate.
   - Specialized review agents (duplication, performance, API consistency).
   - A human owns architecture and public APIs.
   - An explicit **AI contribution policy** in the opposite spirit to Godot's: AI-written code welcome, but it must pass the oracles and a human is accountable for it.

### 8. Business model and trust (brief)

- **Unity:** the runtime fee announced in September 2023 was fully cancelled on September 12, 2024, returning to seat pricing ([AlternativeTo](https://alternativeto.net/news/2024/9/unity-entirely-scraps-controversial-runtime-fee-returns-to-seat-based-subscription-model)). Trust damage outlasted the reversal, and it accelerated Godot's growth.
- **Unreal:** 5% royalty above $1M, or 3.5% under "Launch Everywhere with Epic" from 2025 ([80.lv](https://80.lv/articles/unreal-engine-developers-can-pay-epic-less-with-new-program)). Predictable, so tolerated.
- **Flax:** 4% royalty above $250k per quarter.
- **Godot:** donations, plus third parties selling console ports (W4 Games, [Godot docs](https://docs.godotengine.org/en/4.5/tutorials/platform/consoles.html)).
- **Lesson:** developers accept a known, stable price, and they punish retroactive changes. A permissive license is the strongest possible promise, because a fork is always possible.
- **Options for snengine:**
  - Apache-2.0 OR MIT engine, free forever.
  - Revenue from:
    - Hosted agent and inference credits. BYO-key must stay free, since Unity's AI Gateway already lets users bring Claude, Codex or Gemini at no Unity cost.
    - A cloud build and test farm (golden-image CI as a service).
    - Asset generation.
    - Multiplayer hosting.
    - Enterprise support.
    - A marketplace cut.
    - Console middleware through a partner.
  - Avoid royalties, so we never become "the next runtime fee".
- **Caveat:** US copyright requires human authorship (cited in [Bevy's policy](https://bevy.org/learn/contribute/policies/ai/)). Heavily AI-written code may be weakly copyrightable. That matters little under a permissive license but would undermine an open-core or proprietary model.

---

## Recommendations for snengine

### Recommended stack

| Layer | Choice | Pillars served |
|---|---|---|
| Core language | **C++20, plus C++23 features that both MSVC and Clang support.** No modules, no exceptions (`std::expected`), RTTI off. Sanitizers and a hardened standard library in CI. | 2 (direct GPU/ecosystem access), 1 (agents are competent, huge corpus) |
| Compilers | MSVC + clang-cl on Windows; Clang on Linux; Apple Clang later | — |
| Build | **CMake + Ninja + presets**, vendored `third_party/` + pinned prebuilt SDKs, sccache, DLL-per-module dev builds | 1, 3 (one-command setup, fast agent loops) |
| Dev CLI | **`snen` (TypeScript/Bun)**: setup, build, test, gen, golden, run-headless, package | 1 (single entry point for agents), 3 |
| Scripting | **TypeScript on V8** (JIT on desktop/Android, jitless on iOS/consoles), checked by TS 7, bindings and `.d.ts` generated from the type registry, hot reload with state in components. Plan B: C# via the same generator. | 1, 3, 5 |
| Type registry/codegen | C++ registration → `api.json` → TS bindings, serialization, inspector, MCP schemas, docs | 1, 3, 4, 5 |
| ECS/scene | flecs (MIT) underneath our scene API | 1 (introspection), 3 |
| RHI | **NVRHI** (MIT) behind our render graph in Phase 1. D3D12 (Agility SDK) + Vulkan 1.3. Decide on adding Metal to NVRHI vs our own RHI at the M12 review. Wicked Engine as a reference. | 2 |
| Shaders/materials | **Slang** → DXIL/SPIR-V/Metal; the material graph generates Slang | 2, 5 |
| Platform | **SDL3** | 3 |
| Physics | **Jolt** (3D), **Box2D v3** (2D) | 2, 3 |
| Assets | ufbx, fastgltf, meshoptimizer, basis_universal/KTX2, zstd; OpenUSD import later | 2, 3 |
| Animation / nav | ozz-animation (+ACL later), Recast/Detour | 2, 3 |
| Audio | miniaudio + Steam Audio; FMOD/Wwise as user-licensed plugins | 2, 3 |
| UI/text | Game UI authored in **TSX** → Yoga layout → native renderer; HarfBuzz + FreeType + msdfgen (+ICU subset); Dear ImGui for debug only | 4, 5 |
| Tooling | Tracy, sentry-native, RenderDoc/PIX/Nsight, Superluminal (optional) | 3 |
| Networking / XR | GameNetworkingSockets; OpenXR (Phase 3+) | — |
| Editor shell | Coordinate with the editor research track. A TS/React shell fits the TS-everywhere choice. **Avoid Qt** (LGPL, weight). | 4 |
| License | **Apache-2.0 OR MIT** | trust |

### 24-month roadmap

**M0–M2: Foundations and decision gates**
- Repo skeleton, `snen`, CMake presets, CI with sanitizers, and `AGENTS.md` files.
- Type registry plus `api.json` codegen.
- SDL3 window, NVRHI triangle on D3D12 and Vulkan, Slang pipeline.
- V8 embed with a hot-reload prototype.
- **Gate 1: TS vs C# benchmark.** Gate 2: flecs vs our own scene store.
- Ships: internal "hello world" plus a headless test harness with golden images.

**M3–M6: v0.1 "agent can make a small 3D game"**
- Scene, prefabs, serialization (text, diff-friendly).
- Asset database and pipeline CLI (glTF/FBX → meshoptimizer → KTX2).
- Clustered forward PBR renderer with cascaded shadows, IBL and TAA.
- Jolt, SDL3 input, miniaudio, TS gameplay with ms hot reload.
- MCP server plus CLI over the same command bus.
- Editor v0: viewport, gizmos, hierarchy, inspector generated from the registry, undo built on the command log, play-in-editor.
- Windows only.
- Ships: a private preview plus one dogfood game built largely by agents.

**M7–M12: v0.5 public alpha**
- Render graph, bindless, GPU-driven culling, meshlets.
- First GI (probe-based DDGI) and upscalers (FSR/DLSS/XeSS).
- Material graph v1 → Slang.
- UI authoring v1 (TSX + Yoga + text stack, live preview, an agent-editable layout format).
- Animation (ozz state machine and blend trees), Recast nav, Steam Audio.
- Linux; packaging and export.
- Docs site generated from `api.json`; project templates.
- **M12 review:** RHI future (NVRHI + Metal vs our own) and scripting performance in practice.

**M13–M18: v0.8 beta "fidelity push"**
- Pick **3–4 standout features** and do them very well. Candidates:
  - virtual shadow maps
  - cluster-LOD virtualized geometry with streaming
  - hardware RT reflections and GI (ReSTIR-class) with a fallback
  - volumetrics and atmosphere
  - GPU VFX graph
- Terrain, world streaming v1, networking (GNS + replication v1).
- Begin the macOS/Metal port.
- Ships: a public beta plus a vertical-slice demo that is visually comparable to UE in those features.

**M19–M24: v1.0 PC**
- API freeze and semantic versioning: critical for agents, see the GDScript drift problem.
- Performance and stability, crash reporting.
- Asset/package registry.
- A Unity project importer (scenes, prefabs, materials, best-effort C#→TS translation by agents) as the growth driver.
- XR (OpenXR) if demand exists.
- Console middleware through a partner after 1.0.

---

## Open questions / risks

| # | Risk | Mitigation |
|---|---|---|
| 1 | **TS performance on no-JIT platforms** (jitless V8 is 40–80% slower than JIT on web benchmarks) | State in components, batched APIs, V8 Fast API calls, C++ native-component escape hatch, Gate 1 benchmark, C# plan B via the same generator. Watch Static Hermes (unshipped) only as an option. |
| 2 | **Chasing fidelity and scope creep** | Development driven by the dogfood game; 3–4 standout features; written non-goals (no in-house consoles or mobile before 1.0) |
| 3 | **Code quality and regressions from agents** ("fixes often broke existing functionality") | Oracles first (golden images, replays, fuzzing), strict CI, review agents, human-owned architecture and ADRs, module size limits |
| 4 | **License contamination** (agents copying UE, Flax or Unity code) | No proprietary source in agent context, network-off implementation agents, ScanCode and similarity checks in CI, notices tracking |
| 5 | **V8 embedding and build burden** | Pinned prebuilt V8 per platform; thin binding layer; QuickJS-ng as a fallback for footprint or tests. *Unverified:* prebuilt console V8 availability. |
| 6 | **Differentiation vs Unity AI, UE 5.8 MCP and UE6 (EA end of 2027)** | Make AI-native structural: headless-first, every action is a typed, undoable command; deterministic sim; full introspection; one TS language for everything; agent-runnable test oracles |
| 7 | **NVRHI dead end** (no Metal, no consoles) | Render graph façade isolates it; decision at M12; MIT means we can fork and add a backend |
| 8 | **OSS community friction over AI** (Godot and Bevy restrict it) | Publish a clear pro-AI, oracle-gated contribution policy from day 1 |
| 9 | **Copyright weakness of AI-written code** | A permissive license makes it mostly moot; record human design authorship (ADRs); avoid business models that depend on enforcing code copyright |
| 10 | **Bus factor** (one-lead pattern of Flax, Wicked, Fyrox) | Exhaustive docs and ADRs; agents as institutional memory; 2+ humans with review rights on core |

**Open questions for Islam:**
- Team size and funding?
- Is 2D a first-class target?
- Which genre is the dogfood game?
- Is the editor UI web-based or native? (Coordinate with the editor track.)
- Where are networking and mobile on the 1.0 priority list?
- Is Unity-project import a strategic goal? If yes, C# plan B becomes more attractive.

---

## Sources

**Local clones (read-only)**
- `.refs/godot/LICENSE.txt`
- `.refs/godot/thirdparty/README.md`
- `.refs/godot/modules/jolt_physics/`
- `.refs/godot/SConstruct:311`
- `.refs/godot/main/main.cpp:672`
- `.refs/godot/core/extension/extension_api_dump.cpp`
- `.refs/o3de/LICENSE.txt`
- `.refs/o3de/Code/Framework/AzCore/AzCore/RTTI/BehaviorContext.cpp`
- `.refs/flax/LICENSE.md`
- `.refs/flax/AGENTS.md`
- `.refs/flax/Source/Tools/Flax.Build/Bindings/`
- `.refs/flax/Source/Tools/Flax.Build/Build/DotNet/DotNetAOT.cs`
- `.refs/flax/Source/Engine/Physics/Actors/RigidBody.h`
- `.refs/unity-cs-reference/LICENSE.md`

**GitHub API (2026-10-22):** release, license and push data for all repos in §4 and §1.2, e.g. `gh api repos/jrouwe/JoltPhysics`, `…/releases/latest`, `…/contributors`.

**Web**
- Octoverse 2025: https://github.blog/news-insights/octoverse/octoverse-a-new-developer-joins-github-every-second-as-ai-leads-typescript-to-1/
- Multi-SWE-bench: https://arxiv.org/pdf/2504.02605
- AI vs human C++ verification: https://arxiv.org/pdf/2607.00107
- Rust compiler-feedback study: https://www.sciencedirect.com/science/article/pii/S0925231225021332
- Anthropic, Building a C compiler: https://anthropic.com/engineering/building-c-compiler
- GameLogicBench: https://arxiv.org/abs/2609.21562
- Zig 1.0 interview: https://blog.jetbrains.com/blog/2026/06/05/why-zig-isn-t-1-0-yet/
- C++26: https://en.wikipedia.org/wiki/C%2B%2B26
- ReSharper C++ 2026.2 (reflection): https://blog.jetbrains.com/rscpp/2026/07/22/resharper-cpp-2026-2-cpp-26-reflection-ispc-language-support-and-more/
- CMake C++ modules: https://www.mankier.com/7/cmake-cxxmodules
- Bevy 0.20: https://bevy.org/news/bevy-0-20/
- Bevy hot patching: https://hackmd.io/@bevy/SkdMQIYWle
- Bevy AI policy: https://bevy.org/learn/contribute/policies/ai/
- Fyrox 1.0: https://fyrox.rs/blog/post/fyrox-game-engine-1-0-0/
- O3DE 2025 review: https://o3de.org/o3de-2025-in-review-and-a-look-into-2026/
- O3DE 26.05: https://www.cgchannel.com/2026/06/open-3d-foundation-releases-o3de-2026-5/
- Godot growth stats: https://godotengine.org/article/godot-growth-stats-2026/
- Godot Steam analytics: https://wnhub.io/news/analytics/item-50801
- Godot AI policy: https://www.gamedeveloper.com/business/godot-to-ban-almost-all-ai-coding-contributions
- Godot C# docs: https://docs.godotengine.org/en/stable/tutorials/scripting/c_sharp/index.html
- Godot console docs: https://docs.godotengine.org/en/4.5/tutorials/platform/consoles.html
- GDScript and AI: https://www.summerengine.com/blog/best-ai-for-gdscript
- Unity CoreCLR update: https://discussions.unity.com/t/coreclr-scripting-and-serialization-update-june-2026/1723299
- Unity AI and MCP: https://unity.com/en/blog/unity-ai-mcp-how-to-get-started
- Unity Reference-Only License: https://unity.com/legal/licenses/unity-reference-only-license
- Unity fee cancellation: https://alternativeto.net/news/2024/9/unity-entirely-scraps-controversial-runtime-fee-returns-to-seat-based-subscription-model
- State of Unreal 2026: https://www.strafe.com/news/read/state-of-unreal-2026-all-the-key-takeaways-from-the-epic-keynote/
- UE royalty (Launch Everywhere): https://80.lv/articles/unreal-engine-developers-can-pay-epic-less-with-new-program
- Oodle in UE: https://www.unrealengine.com/en-US/blog/oodle-now-free-to-use-in-unreal-engine-via-github
- Flax licensing: https://flaxengine.com/licensing
- The Machinery shutdown: https://gameworldobserver.com/2022/08/01/our-machinery-terminates-machinery-game-engine-changes-eula
- Lumberyard → O3DE: https://www.theregister.com/2021/07/07/open_3d_engine
- s&box: https://en.wikipedia.org/wiki/S%26box
- s&box open source: https://gamefromscratch.com/sbox-game-engine-open-sourced/
- s&box docs: https://sbox.game/dev/doc/getting-started/
- secbox (s&box editor sandboxing): https://sbox.game/f4industries/secbox
- SS14 sandboxing: https://docs.spacestation14.com/en/robust-toolbox/sandboxing.html
- Hazelight Angelscript: https://angelscript.hazelight.se/
- V8 jitless: https://v8.dev/blog/jitless
- Hermes V1: https://swmansion.com/blog/welcoming-the-next-generation-of-hermes-67ab5679e184
- TypeScript 7: https://infoq.com/news/2026/08/typescript-7-released/
- Puerts: https://github.com/Tencent/puerts
- Wasvy: https://docs.rs/crate/wasvy/0.0.4
- PhysX GPU open source: https://www.cgchannel.com/2025/04/nvidia-open-sources-physxs-gpu-simulation-code
- Slang at Khronos: https://www.khronos.org/news/press/khronos-group-launches-slang-initiative-hosting-open-source-compiler-contributed-by-nvidia
- SDL GPU: https://wiki.libsdl.org/SDL3/CategoryGPU
- Wwise indie license: https://www.audiokinetic.com/en/blog/free-wwise-indie-license
- AGENTS.md / AAIF: https://openai.com/index/agentic-ai-foundation/

**Unverified or flagged in text:**
- Embark's engine choice
- Puerts shipped titles
- Prebuilt console V8 availability
- Flax EULA reuse terms (PDF not reviewed)
- FidelityFX SDK license (GitHub reports none; AMD states MIT)
- Diligent Metal licensing today
