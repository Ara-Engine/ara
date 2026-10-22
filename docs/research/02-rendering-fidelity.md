# R02 — Rendering, shaders and visual fidelity

Research date: **2026-10-22**. Recommendations are engineering judgments, not benchmark results. The proposed stack assumes a native renderer and a **Windows-first, high-end desktop** product; platform scope remains a product decision.

## TL;DR

- **Unreal-level images are a plausible staged goal; Unreal-wide feature parity is not a credible small-team starting scope.** Lighting, materials, content quality, shadows, exposure, and temporal stability should precede geometry virtualization.
- Start with **NVRHI + D3D12, Slang shaders, a small engine-owned resource graph, clustered deferred opaque rendering, and forward transparency**. Validate Vulkan early; NVRHI does not provide Metal, mobile, or web reach.
- Use **one material representation**: a constrained typed expression language backed by a canonical AST, with graph and text editors over that same structure. Generated shaders are build artifacts. Do not promise reversible conversion of arbitrary shader programs into graphs.
- Start with baked indirect light, reflection probes, and excellent direct lighting. Add **hardware-ray-traced DDGI and selective RT reflections** before attempting a Lumen substitute. Defer a second software-tracing scene representation until hardware requirements demand it.
- Design stable GPU scene IDs, resource binding, motion vectors, material evaluation, and asset processing for GPU-driven rendering immediately; implement meshlet streaming only after measured geometry bottlenecks.
- Current-state corrections: **UE 5.8 shipped; MegaLights is production-ready; Substrate and PCG reached that status in 5.7; new Nanite Foliage remains experimental. Unity is advancing URP and maintaining HDRP. Godot 4.7 includes HDR output and Vulkan RT groundwork. Flax 1.13 shipped yesterday.** Sources and caveats follow.
- Treat shader variants and pipeline creation as a build-system problem: explicit variant budgets, content-derived PSO manifests, asynchronous compilation, driver-specific caches, and atomic hot reload.
- Make frame inspection a product API: deterministic replay inputs, pass/resource dumps, GPU timing distributions, material diagnostics, reference renders, and image comparisons. This serves agents and humans equally.

## How each reference engine does it

### Unreal Engine 5.x: an integrated fidelity system

Epic's current major release is **5.8**, announced June 23, 2026. Its fidelity comes from mutually supporting systems and an enormous content/tooling ecosystem, rather than a single rendering algorithm. The release announcement describes 5.8 as the last *planned* major UE5 release; this is a roadmap statement, not an end to maintenance. [UE 5.8 release](https://www.unrealengine.com/news/unreal-engine-5-8-is-now-available)

**Nanite** preprocesses geometry into compressed hierarchical clusters, selects visible detail on the GPU, and streams it on demand. It reduces manual LOD work and makes dense scanned assets practical. It does not make instance count, materials, bandwidth, or overdraw free. Ordinary mesh shaders alone do not reproduce its asset processing, hierarchy construction, streaming, and rasterization strategy. [Nanite overview](https://dev.epicgames.com/documentation/en-us/unreal-engine/nanite-virtualized-geometry-in-unreal-engine)

The old statement “Nanite supports only static meshes” is obsolete: current documentation includes skeletal meshes, animation LODs, animation-bank instancing, and VSM shadows. However, I could not verify a complete current compatibility matrix for morph targets, cloth, grooms, and every deformation path; do not assume traditional skeletal-renderer parity. Both offline static displacement and runtime **Nanite Tessellation** remain labeled experimental. Ordinary foliage on Nanite is distinct from the new dedicated foliage system. [Working with Nanite content](https://dev.epicgames.com/documentation/en-us/unreal-engine/working-with-naniteenabled-content)

**Nanite Foliage** combines assemblies that reuse small geometry parts, skinning for animation, and voxel representations for distant aggregate detail. This addresses the particular failure of triangle simplification and occlusion on forests full of holes. Its documentation still labels the feature experimental. Steal its diagnosis—foliage needs specialized representation and animation—not its entire implementation agenda for version one. [Nanite Foliage](https://dev.epicgames.com/documentation/unreal-engine/nanite-foliage?lang=en-US)

**Lumen** combines screen traces with software distance-field tracing or hardware triangle tracing, surface/radiance caches, and temporal reconstruction. Hardware RT supports more geometry and better mirror reflections, but acceleration-structure updates and overlapping instances can be costly. Software RT trades hardware reach for an additional geometry representation, restricted content, leakage, and update complexity. Current technical documentation contains inconsistent default-language between sections; explicitly configure and measure the selected mode. [Lumen technical details](https://dev.epicgames.com/documentation/en-us/unreal-engine/lumen-technical-details-in-unreal-engine)

UE 5.8 adds **Lumen Lite**, using an irradiance-field final gather; medium reflection quality substitutes SSR for Lumen reflections on smooth surfaces. Epic's claimed speedup is not a portable performance guarantee. The architectural lesson is that even Epic offers a cheaper probe-oriented solution rather than insisting on one maximum-quality algorithm everywhere. [Lumen performance guide](https://dev.epicgames.com/documentation/unreal-engine/lumen-performance-guide-for-unreal-engine)

**Virtual Shadow Maps** allocate and cache needed pages of very high-resolution shadow maps. They match dense geometry well, but moving/deforming geometry and lights invalidate cached pages. High-quality shadows therefore depend on stable bounds, animation policy, geometry submission, and memory budgeting. Start snengine with a conventional shadow atlas and cascades; add virtual pages when actual scenes justify the cache machinery. [VSM documentation](https://dev.epicgames.com/documentation/en-us/unreal-engine/virtual-shadow-maps-in-unreal-engine)

**MegaLights** samples direct lighting stochastically, traces visibility, and reconstructs the result. It becomes production-ready in 5.8. Its near-constant sample budget shifts failure toward noise and temporal artifacts as lighting complexity grows; it does not eliminate BVH or light-candidate costs. Current docs recommend conventional deferred lighting plus VSM for strong directional lights. This is an attractive later many-light path, not a reason to begin with stochastic lighting. [MegaLights](https://dev.epicgames.com/documentation/en-us/unreal-engine/megalights-in-unreal-engine)

**Substrate** replaces a fixed shading-model menu with composable material scattering behaviors. **PCG** makes convincing density and variation authorable at scale; it is content generation, not a shading technique. Both reached production-ready status in 5.7. Some Substrate documentation landing pages still say beta; the dated release announcement is the stronger status evidence. Keep snengine's material semantics extensible, but initially implement a bounded principled model rather than arbitrary layered closures. [UE 5.7 release](https://www.unrealengine.com/news/unreal-engine-5-7-is-now-available), [Substrate landing page](https://dev.epicgames.com/documentation/en-us/unreal-engine/substrate-materials-in-unreal-engine)

The **Material Editor** translates expression graphs into generated HLSL and then platform shaders. Its HLSL view is read-only; it is not a bidirectional source editor. This distinction matters for snengine: showing generated code helps diagnosis, but only a shared editable representation gives agents and artists equal authoring access. [Essential material concepts](https://dev.epicgames.com/documentation/unreal-engine/essential-unreal-engine-material-concepts)

**TSR** is a vendor-independent temporal upscaler tightly connected to Nanite detail and Lumen's frame budget. **Niagara** combines reusable emitters, ordered modules, typed parameters, data interfaces, and CPU/GPU execution. Its module stack is often a better authoring model than a giant freeform graph. [TSR](https://dev.epicgames.com/documentation/en-us/unreal-engine/temporal-super-resolution-in-unreal-engine), [Niagara](https://dev.epicgames.com/documentation/en-us/unreal-engine/overview-of-niagara-effects-for-unreal-engine)

The expensive lesson is **combinatorics**: material specializations, vertex/deformation paths, passes, platforms, and render states multiply shader and PSO work. Shader compilation and driver pipeline compilation are separate costs. Epic provides both collected PSO caches and automatic asynchronous precaching; neither makes an unbounded feature matrix harmless. Steal precaching and visibility into misses, and avoid creating the same matrix. [PSO precaching](https://dev.epicgames.com/documentation/unreal-engine/pso-precaching-for-unreal-engine)

### O3DE Atom: explicit composition, substantial infrastructure

Atom separates **RHI**, its device-facing abstraction and frame scheduling machinery, from **RPI**, which assembles scenes, views, pipelines, materials, and passes. Feature processors collect and prepare feature-specific scene data. Pass templates are data assets; the inspected `MainPipeline.pass` explicitly connects morphing, skinning, acceleration structures, depth, motion vectors, and lighting. This is excellent evidence that a renderer can be both programmable and inspectable. [Atom developer guide](https://docs.o3de.org/docs/atom-guide/dev-guide/), local evidence **L1–L3** below.

Atom's documented features include clustered forward shading and ray-traced diffuse probe grids. These are strong practical foundations without virtualized geometry. Its `.materialtype` schema describes properties and shader associations; abstract types supply material-specific evaluation plus a lighting-model reference, while builders combine them with material pipelines. Material Canvas emits shader/material files from graph templates. This is a useful separation of surface semantics from render passes. [Atom features](https://www.docs.o3de.org/docs/atom-guide/features/), local evidence **L4–L5**.

**Assessment:** borrow named passes, declared attachments, typed material properties, and explicit feature boundaries. Avoid transplanting the full Gem/component/bus/asset-builder architecture into an empty engine. That framework solves organizational and extensibility problems larger than snengine initially has. The inspected O3DE checkout is newer than the verified 26.05 release and must not be mistaken for a released feature set. [26.05 notes](https://www.docs.o3de.org/docs/release-notes/2605-0-release-notes/)

### Flax: the most relevant scope example

Flax uses a conventional deferred renderer with forward effects, SDF scene data, a global surface atlas, and camera-centered cascaded DDGI. Its probes track visibility and adapt update work; temporal blending stabilizes samples at the cost of slower lighting response. The inspected implementation exposes this machinery directly in a relatively understandable renderer. [Rendering overview](https://docs.flaxengine.com/manual/graphics/overview/index.html), [DDGI documentation](https://docs.flaxengine.com/manual/graphics/lighting/gi/realtime.html), local evidence **L6–L7**.

**Flax 1.13, released October 21, 2026**, adds DDGI quality/performance work, rough specular probe reflections, PCSS soft shadows, reversed Z, and occlusion-culling options. Its release post explicitly identifies GPU-only culling with indirect drawing as future work: HZB culling does not automatically mean a fully GPU-driven submission pipeline. [Flax 1.13](https://flaxengine.com/blog/flax-1-13-released/)

**Assessment:** this is evidence for building a cohesive, good-looking subset. The software tracer still requires SDF generation, representation updates, atlas management, and handling missing thin/deforming geometry. Borrow the staged scope and diagnostics; prefer hardware triangle queries initially if the minimum GPU allows them.

### Godot: readable boundaries and deliberate feature tiers

**Forward+** and **Mobile** use RenderingDevice over modern APIs; **Compatibility** uses the older GL family. Forward+ provides the richer desktop effects, while Mobile prioritizes a smaller rendering workload. These are rendering methods, not synonyms for Vulkan versus OpenGL: current documentation includes Vulkan, D3D12, and Metal as RenderingDevice-capable APIs. [Renderer comparison](https://docs.godotengine.org/en/stable/tutorials/rendering/renderers.html)

Godot offers **LightmapGI**, **VoxelGI**, and **SDFGI**. Lightmaps give strong static-scene quality cheaply at runtime. VoxelGI uses preprocessed scene volumes; SDFGI provides large-world indirect lighting with restrictions on contributing dynamic geometry and detail. VoxelGI/SDFGI are Forward+ features. These approaches illustrate the quality, authoring, memory, and dynamism tradeoffs that a single “GI quality” slider can conceal. [GI comparison](https://docs.godotengine.org/en/stable/tutorials/3d/global_illumination/introduction_to_global_illumination.html)

Godot 4.7 adds HDR display output and rectangular area lights. Its merged Vulkan ray-tracing plumbing and current RenderingDevice declarations provide RT infrastructure; that does **not** establish a production-ready built-in path-traced game renderer. [4.7 release](https://godotengine.org/releases/4.7/), [RT plumbing PR](https://github.com/godotengine/godot/pull/99119), local evidence **L8**.

RenderingDeviceGraph tracks synchronization and command ordering. VisualShader generates textual shader code, while ordinary text shaders expose more capabilities; GPU particles offer an approachable alternative to a Niagara-sized framework. **Assessment:** borrow readable device boundaries, useful shader previews, and particle defaults. Avoid separate graph/text semantics and avoid committing to three rendering methods before identifying real platform requirements. [VisualShader](https://docs.godotengine.org/en/stable/tutorials/shaders/visual_shaders.html), [Particles](https://docs.godotengine.org/en/stable/tutorials/3d/particles/index.html), local evidence **L9–L10**.

### Unity 6.x: useful extensibility, a warning about fragmentation

SRP exposes rendering orchestration to managed code. HDRP supplies advanced lighting/material features; URP emphasizes scalable cross-platform rendering. Shader Graph generates pipeline-targeted shaders; VFX Graph simulates effects on the GPU. The local Unity C# reference contains public/native-facing SRP APIs, **not the complete HDRP/URP implementation**; study Unity's separate Graphics repository for those internals. [Graphics repository](https://github.com/Unity-Technologies/Graphics), [Shader generation](https://docs.unity.com/en-us/engine/6000.0/manual/materials-and-shaders/class-shader-importer), [VFX Graph](https://docs.unity3d.com/Packages/com.unity.visualeffectgraph@17.0/manual/index.html), local evidence **L11**.

Unity 6.3 unified the underlying HDRP/URP Render Graph compiler/API and enabled graph rendering by default for upgraded projects, with compatibility behavior restricted behind a define. The graph can inspect native-pass merging on devices. This is more substantive than simply having a node-based pipeline diagram. [Unity's Render Graph update](https://discussions.unity.com/t/render-graph-updates-in-unity-6-3/1668122)

Unity's **2026 strategy** advances URP, keeps HDRP in maintenance with no new features planned, and begins Built-In deprecation in 6.5. Dynamic GI, SSR, physical units, and other URP improvements are described as investment areas; that strategy page alone does not verify which later 6.x package ships each feature. **Assessment:** borrow extensible graph passes and artist-facing tools; avoid splitting material ecosystems into “easy” and “beautiful” pipelines. [2026 pipeline strategy](https://unity.com/topics/render-pipelines-strategy-for-2026)

## State of the art beyond these engines

### GPU-driven geometry and resource scheduling

An effective architecture has four distinct pieces: persistent GPU scene data; compute culling/LOD and indirect submission; resource lifetime/synchronization scheduling; and material evaluation. **Bindless** resource indexing helps remove per-object binding work, but requires safe descriptor lifetime, explicit capability checks, and visibility into resource use. It is a resource model, not an automatic performance improvement.

A **visibility buffer** stores surface identity and reconstructs attributes for deferred shading. It can reduce geometry-pass bandwidth and separate visibility from materials, but requires correct interpolation/texture derivatives, deformation replay or stored data, masked-material handling, and dedicated transparency. A compact conventional G-buffer is a safer first implementation. The Forge provides serious visibility-buffer examples; these are more relevant than an isolated triangle demo. [The Forge](https://github.com/ConfettiFX/The-Forge)

**Bevy** is evidence that non-Epic virtual geometry is practical. Current meshlet code includes fine-grained culling, near-seamless LOD, and visibility-buffer rasterization, but documents higher base overhead, preprocessing/material restrictions, no MSAA, and Vulkan/Metal-only operation for that path. Bevy 0.19 also improves its Solari realtime path-traced renderer. Do not confuse those native extensions with baseline browser WebGPU capabilities. [Meshlet implementation](https://raw.githubusercontent.com/bevyengine/bevy/main/crates/bevy_pbr/src/meshlet/mod.rs), [Bevy 0.19](https://bevy.org/news/bevy-0-19/)

**Traverse's Breda** combines geometry preparation, mesh shading, visibility buffers, culling, and streaming. Its own explanation warns that “One Trillion Triangles” describes scene scale, not a measured frame rate. **Wicked Engine** exposes mesh-shader/visibility-compute paths and multiple GI techniques in a compact engine. **NVIDIA's Asteroids sample** and **AMD's mesh-shader guidance** demonstrate the hardware building blocks, not an entire Nanite replacement. [Traverse](https://traverse.nl/research/micro-polygon-rendering/), [Wicked Engine](https://github.com/turanszkij/WickedEngine/blob/master/Content/Documentation/WickedEngine-Documentation.md), [NVIDIA mesh shaders](https://developer.nvidia.com/blog/using-turing-mesh-shaders-nvidia-asteroids-demo), [AMD guidance](https://gpuopen.com/learn/mesh_shaders/mesh_shaders-optimization_and_best_practices/)

**Async compute** overlaps independent work; it can also increase contention and synchronization cost. Implement accurate pass dependencies first, then selectively overlap measured workloads. A graph is valuable even on one queue: it can validate access, discard unused work, track transient lifetimes, and expose a useful debug model. A dependency DAG alone is not necessarily an aliasing allocator or automatic queue scheduler.

### Lighting options ranked for snengine

This ranking is **recommended adoption order**, balancing fidelity, runtime cost, and engineering complexity—not a universal quality ordering. Hardware RT is a tracing mechanism used by several options, rather than a competing GI algorithm.

1. **Baked lightmaps + probes + good direct lighting.** Very high static fidelity, low runtime cost, low-to-medium integration effort if baking is reused. Costs: bake iteration, UVs, storage, static assumptions. Use for the first visual proof; it cannot satisfy arbitrary runtime destruction or time-of-day changes alone.
2. **DDGI with hardware triangle tracing.** Strong diffuse bounce lighting with predictable probe/update budgets; medium implementation effort and medium GPU/memory cost. Visibility moments, probe relocation/classification, local invalidation, and hysteresis matter. It is low-frequency lighting: glossy reflections need another solution. Thin walls and rapid changes remain difficult. Prefer one automatic volume initially; add scrolling/cascades when needed. [Original DDGI paper](https://www.jcgt.org/published/0008/02/01/paper-lowres.pdf), [production extensions](https://arxiv.org/abs/2009.10796)
3. **Selective hardware RT shadows/reflections.** High fidelity for contact, offscreen reflections, and mirror cases, with bounded ray budgets and denoising. Medium-to-high integration cost: BLAS/TLAS updates, alpha-tested visibility, material evaluation, and temporal rejection. This augments DDGI. Use a vendor-independent denoiser such as NRD if its input contract fits. [NRD](https://github.com/NVIDIA-RTX/NRD)
4. **ReSTIR DI, then GI.** DI resamples direct-light candidates; GI resamples indirect path samples. High potential fidelity and many-light scalability, but considerable reservoir, visibility, reprojection, bias, and denoising complexity. Rapid disocclusion exposes sample scarcity. RTXDI now includes DI/GI/PT support; these are not interchangeable modes with identical requirements. [RTXDI integration](https://github.com/NVIDIA-RTX/RTXDI/blob/main/Doc/Integration.md), [ReSTIR GI](https://research.nvidia.com/publication/2021-06_restir-gi-path-resampling-real-time-path-tracing)
5. **Surfel GI.** Stores lighting on surface elements, allocating work to scene surfaces rather than empty volume. Attractive for detailed dynamic worlds; high implementation cost around placement, coverage, visibility, updates, and gathering. EA's GIBS work provides production evidence, but does not remove the need for engine-specific engineering. Consider only after probe limitations are demonstrated. [EA's shipping GI presentation](https://www.realtimerendering.com/advances/s2024/content/EA-GIBS2/Apers_Advances-s2024_Shipping-Dynamic-GI.pdf)
6. **SDF/voxel tracing.** Broadens hardware reach and can share scene data with particles or occlusion. Medium-to-high runtime cost and high total systems cost once baking, streaming, animated geometry, thin surfaces, and surface-radiance caches are included. Flax/Godot show both usefulness and limits. Choose it for a required non-RT platform tier, not because “software” sounds simpler.
7. **Radiance cascades.** Allocates spatial and angular resolution by distance to exploit the structure of radiance. Promising, especially for 2D and constrained workloads; 3D representation, memory, updates, and general material behavior still require careful evaluation. The 2026 Split Radiance Cascades paper is relevant new work. I did not verify a reusable general-purpose implementation with snengine's intended feature coverage; keep this as a bounded experiment. [Osborne/Sannikov theory](https://arxiv.org/abs/2408.14425), [Split Radiance Cascades](https://arxiv.org/abs/2607.20384)

Build an **offline/editor reference path tracer** alongside the hybrid renderer, using the same mesh, light, and material semantics. High-sample convergence is a correctness reference; denoised low-sample output is not unquestionable ground truth. Start with supported opaque materials and direct/environment lighting, then expand. A shipping realtime path tracer is a later product choice. Also distinguish SDK generations: current RTXGI emphasizes neural/spatial-hash caches; the older DDGI implementation lives separately. [RTXGI](https://github.com/NVIDIA-RTX/RTXGI), [RTXGI-DDGI](https://github.com/NVIDIAGameWorks/RTXGI-DDGI)

### RHI choices: fidelity per engineering effort

- **Own D3D12/Vulkan/Metal:** maximum control, largest maintenance burden. Synchronization, descriptors, residency, crash diagnostics, and vendor bugs become engine work. Reject three new backends at launch; own a backend only when an observed library limitation warrants it.
- **NVRHI — recommended for the desktop-first plan.** MIT-licensed C++ abstraction over D3D11/D3D12/Vulkan; graphics, compute, ray tracing, mesh pipelines, optional automatic state tracking, multiqueue work, and native escape access. Its SDK usage provides relevant high-end integration evidence. Windows/Linux scope is a deliberate limitation; it supplies neither a render graph nor a finished renderer. [NVRHI](https://github.com/NVIDIA-RTX/NVRHI)
- **Diligent:** broad public D3D/GL/Vulkan/WebGPU coverage, RT/mesh examples, and a self-contained core. A plausible alternative when platform breadth outweighs minimal high-end integration. **Native Metal is available to commercial clients**, not something to assume is included in the open-source backend set. Verify terms and required features before selecting it. [DiligentCore](https://github.com/DiligentGraphics/DiligentCore), [mesh example](https://github.com/DiligentGraphics/DiligentSamples/blob/master/Tutorials/Tutorial20_MeshShader/readme.md)
- **The Forge:** extensive platform/rendering experience, high-end samples, and visibility-buffer research. Its framework and FSL shader workflow add adoption surface; console code/access must be confirmed with platform holders. Excellent reference, possible foundation if its conventions are accepted wholesale. [The Forge](https://github.com/ConfettiFX/The-Forge)
- **SDL_GPU:** approachable graphics/compute abstraction across D3D12/Vulkan/Metal. Its FAQ explicitly puts ray tracing and mesh shaders outside immediate plans. Good for a portable simpler renderer; a poor binding ceiling for this high-end plan. SDL remains usable for windows/input independently. [SDL GPU](https://wiki.libsdl.org/SDL3/CategoryGPU)
- **wgpu / Dawn:** strong cross-platform validation and browser reach. Do not repeat the stale claim that native wgpu has no RT/mesh access: current features include **experimental** native RT queries/pipelines and mesh shaders, with backend restrictions. These are not portable WebGPU guarantees; current ray queries are Vulkan-only. Dawn follows WebGPU and its own extensions. Prefer these if web reach or a Rust-first architecture wins the product tradeoff, accepting a narrower portable tier. [wgpu features](https://docs.rs/wgpu/latest/wgpu/struct.Features.html), [Dawn](https://dawn.googlesource.com/dawn)
- **bgfx:** mature broad portability with its own submission/shader model. The inspected public API/backend documentation does not establish the full RT/mesh feature contract required here. It is not the recommended high-end foundation; absence of documented support is not proof such work can never be added. [bgfx](https://github.com/bkaradzic/bgfx)
- **slang-rhi:** worth a short competing spike because it combines Slang-aware bindings with D3D/Vulkan/Metal and other backends. Its project explicitly guarantees neither source nor ABI compatibility yet. It is a promising option, not a proven lower-risk replacement for NVRHI. [slang-rhi](https://raw.githubusercontent.com/shader-slang/slang-rhi/main/README.md)

### Shaders, reconstruction, and neural rendering

**Slang** adds modules, generics/interfaces, reflection, and capability-aware compilation to familiar HLSL-style programming. Its project is under Khronos open governance, ships in the Vulkan SDK, and documents production use. Its own maturity matrix calls modules/reflection/RT stable but autodiff and several specialization/mesh features experimental. Metal and WGSL targets are still labeled experimental in the repository; cross-compilation does not guarantee feature parity. Pin a compiler version and compile representative shaders on every supported backend. [Slang project](https://raw.githubusercontent.com/shader-slang/slang/master/README.md), [feature maturity](https://shader-slang.org/docs/feature_matureness/), [reflection](https://shader-slang.org/slang/user-guide/reflection.html)

Use **Slang → DXIL/SPIR-V**, with DXC retained for HLSL/vendor-SDK compatibility and compiler diagnostics. Reflection should generate checked native layouts and editor parameter schemas. Modules can reduce textual duplication; they do not abolish specialization counts. Autodiff could eventually support inverse material fitting, but it is unrelated to whether an LLM can write shaders and should not be a launch dependency. [DXC implementation](https://github.com/microsoft/DirectXShaderCompiler)

Design one temporal-upscaler interface with color, depth, motion, jitter, exposure, reset flags, render/display sizes, and optional reactive masks. Integrate a broadly supported temporal fallback, then DLSS/FSR/XeSS. Current products include DLSS 4.5 and XeSS 3 multi-frame generation, while AMD's FSR SDK 2.3 separates hardware-dependent ML features from broader fallbacks. Check **each feature's** GPU/API/platform contract. TSR is an Unreal implementation to study, not an independently licensed drop-in component. [DLSS](https://developer.nvidia.com/rtx/dlss), [XeSS 3](https://www.intel.com/content/www/us/en/developer/topic-technology/gamedev/xess.html), [FSR SDK 2.3](https://gpuopen.com/learn/amd-fsr-sdk-2-3-blog/)

Frame generation needs presentation scheduling, latency integration, and UI handling; generated display frames do not increase simulation responsiveness. Separately, NVIDIA announced DLSS 5 neural rendering that alters apparent lighting/materials, with fall availability planned. **Announcement verified; general SDK availability and snengine integration feasibility were not verified.** Treat such reconstruction as optional art-directed output, never as the renderer's correctness oracle. [DLSS 5 announcement](https://nvidianews.nvidia.com/news/nvidia-dlss-5-delivers-ai-powered-breakthrough-in-visual-fidelity-for-games)

## Recommendations for snengine

### 1. Own a small renderer architecture, reuse device plumbing

Commit provisionally to **NVRHI, D3D12 first, Vulkan second, Slang, and an engine-owned resource graph**. A capability spike must prove bindless indexing, indirect submission, RT queries, external upscaler integration, GPU timestamps, and capture/debug support before treating the choice as final. If macOS is mandatory at launch, reopen this decision immediately.

Use clustered deferred shading for opaque objects and forward shading for transparency in one desktop pipeline. This simplifies many lights, decals, SSR, and hybrid RT at the cost of G-buffer bandwidth and poor fit for MSAA-heavy VR. Keep surface evaluation independent of G-buffer packing; defer a second mobile/VR renderer. Begin with reversed Z, explicit physical units, linear-light shading, and an agreed color-management contract.

Each graph pass declares reads/writes, subresources, history imports, side effects, debug name, and timing scope. Generate barriers and lifetime diagnostics; add aliasing after validation. Decide whether the graph or RHI state tracker owns transitions, avoiding contradictory ownership. Bindless access still needs declared resource sets. Keep a single-queue debug mode; add async overlap through measurements.

Persistent GPU scene tables should use generational object/material/resource handles, stable IDs for picking/debugging, explicit retirement fences, and incremental updates. Preserve previous deformation/transforms for motion vectors. This prepares GPU culling and agents' object queries without demanding virtual geometry now.

**Pillars served:** AI receives inspectable structured state (1); the renderer preserves high-end headroom (2); one pipeline and reliable defaults reduce user decisions (3); pass inspection and picking support the editor (4); the same material/resource contracts support UI and VFX authoring (5).

### 2. Make material authoring one system

Store versioned **typed expression ASTs**, serialized as a readable constrained DSL. Named expressions map naturally to graph nodes; stable semantic IDs preserve graph layout during edits. Layout metadata must not affect shader hashes. Restrict the language to expressions, bounded operations, typed resources, and explicit domains. Arbitrary Slang functions can appear as opaque custom-function nodes; do not promise to reconstruct their internals visually.

For example, proposed syntax—not an implemented language:

```text
material PaintedMetal : Surface {
  param paint : Texture2D<ColorSRGB>;
  param roughness : Float = 0.35;
  let tint = sample(paint, uv0).rgb;
  output principled(baseColor: tint, metallic: 1.0, roughness: roughness);
}
```

The graph edits `sample` and `principled` expressions in this same AST. The texture type requests color decoding; scalar/data textures use different types. Imported opaque function nodes preserve explicit input/output types and diagnostics.

Separate material **definitions**, cheap parameter **instances**, and engine-owned lighting evaluation. Start with metallic/roughness PBR, normal mapping, emissive, opacity masking, and double-sidedness; add clear coat, foliage transmission, then skin/hair only for a demonstrated scene. Shared semantics can lower to raster, RT-hit evaluation, and the reference tracer, with unsupported features reported explicitly. Depth/shadow evaluation must agree with deformation and alpha masking.

Expose texture sample counts, generated source, variant count, compile duration, and measured preview cost. Give UI materials their own constrained unlit domain with clipping, alpha/color rules, and predictable compositing. Avoid charging every UI effect for the 3D lighting feature matrix.

**Pillars served:** agents author meaningful text (1), principled shading scales toward fidelity (2), instances and presets simplify use (3), previews and graph edits improve UX (4), and UI/VFX use the same compiler infrastructure with distinct domain contracts (5).

### 3. Control compilation and VFX scope

Specialize only where generated code materially differs: render domain, alpha/deformation behavior, and a small number of shading families. Use runtime parameters for ordinary artistic variation. Build only reachable variants and publish a per-material variant budget. A shader-module cache and a driver PSO cache are separate artifacts.

Derive PSO descriptions from cooked content plus actual pass/vertex/render-state combinations; warm them asynchronously before scenes become visible. Driver caches require device/driver/compiler/version keys and invalidation. Collect runtime misses without treating playthrough collection as exhaustive. Hot reload compiles away from the render thread, validates layouts, creates replacement pipelines, and swaps at a frame boundary; failures keep the last good version.

For VFX, borrow Niagara's **ordered module stack** and Unity's spawn/update/output separation. Use GPU structure-of-arrays buffers, alive/dead lists, compaction, indirect draw arguments, and deterministic seed inputs. Start with sprite/mesh particles, curves, flipbooks, and optional depth collision. Add ribbons, SDF collisions, and event readback only when needed. Cap particle counts, transparency overdraw, and lights per emitter. Share pure math/compiler/schema components with materials, not an unrestricted universal graph language.

The tradeoff is deliberate: Niagara's data interfaces and mixed CPU/GPU execution offer more extensibility but more synchronization and authoring surface; Unity VFX Graph makes GPU simulation highly programmable; Godot's process-material/shader model is easier to expose as a compact inspector. Adopt the compact preset experience first, with programmable GPU modules underneath. Keep gameplay-critical effects/events CPU-authoritative. [Godot particle authoring contract](https://docs.godotengine.org/en/stable/classes/class_gpuparticles3d.html)

**Pillars served:** actionable diagnostics and bounded execution help agents (1), GPU effects support fidelity (2), templates and stacks are approachable (3), compilation status remains clear in the editor (4), and reusable modules support visual authoring (5).

### 4. Treat image quality and verification as core tools

Ship a coherent post stack: exposure/pre-exposure, a documented tone mapper and grading LUT, restrained bloom, volumetric atmosphere/fog, then measured DOF/motion blur. Prioritize correct normal/roughness filtering and temporal rejection over adding more effects. Internally HDR rendering and **HDR display output are separate features**. Implement tested SDR first, then Windows HDR/scRGB or HDR10 with paper-white/peak controls, correct metadata where applicable, and readable editor/game UI compositing.

Provide capture requests containing scene/content hashes, camera, simulation tick, seed, exposure, resolution, feature tier, history warmup/reset, and compiler/driver metadata. Return pass/resources, object/material IDs, shader/PSO misses, memory peaks, GPU timestamp distributions, and image channels in machine-readable form. Integrate RenderDoc/PIX entry points rather than rebuilding those tools.

Use focused image scenes: furnace/energy conservation, thin-wall leakage, moving emissives, disocclusion, animated alpha foliage, glossy interiors, transparency, and HDR UI. Compare temporal sequences as well as stills; maintain same-device/driver baselines and tolerances. Fixed seeds do not guarantee bit-identical cross-vendor GPU results. Separate performance captures from deliberately serialized diagnostic captures.

AI-authored material expressions should pass schema/type checks, stage restrictions, bounded loops, resource limits, compile checks, NaN diagnostics, and preview validation. Type correctness cannot prove finite execution cost or visual correctness. Compile and preview untrusted custom shaders in a disposable worker/process; resource/watchdog isolation cannot guarantee a GPU driver will never reset. Publish only validated artifacts with an automatic rollback path.

**Pillars served:** agents can inspect and reproduce failures (1), references expose fidelity errors (2), trustworthy defaults reduce tweaking (3), diagnostics are visible editor features (4), and UI/material/VFX previews exercise the actual rendering path (5).

### Phased rendering roadmap

These are **gated capability milestones, not calendar estimates**. Reuse one small art-directed interior, outdoor foliage scene, and material/animation test set throughout. Set a provisional 60 Hz desktop target, always reporting internal resolution, GPU, VRAM, percentile frame time, and reconstruction mode; frame generation must be reported separately.

1. **Phase 1 — “Looks good, simple.”** NVRHI/D3D12 + Slang; graph validation; compact G-buffer/clustered lighting; shadow atlas/cascades; PBR/IBL; baked indirect lighting; reflection probes; basic temporal AA; color/exposure; sprite particles; text materials and parameter previews. **Exit:** a convincing playable camera walkthrough, correct material tests, and no visible PSO hitch on its prepared route. Prove a Vulkan slice before asset contracts harden.
2. **Phase 2 — Dynamic lighting with controlled cost.** Hardware-traced DDGI, selective RT reflections, denoising, local GI invalidation, shadow-quality improvements, proper animated motion vectors, one temporal upscaler/fallback, and the reference tracer. **Exit:** day/night and moving-light tests survive motion, thin walls, and scene edits within a recorded frame/memory budget. Non-RT machines retain baked/probe lighting.
3. **Phase 3 — Scalable scenes and complete authoring.** Persistent GPU scene/culling/indirect draws, HZB, texture streaming, robust shader/PSO cooking, Vulkan validation, material graph over the existing AST, GPU VFX modules, HDR output. **Exit:** representative larger content scales without CPU submission bottlenecks, missing resources, or shader surprises.
4. **Phase 4 — Geometry and many lights where justified.** Offline meshlets, error-bounded hierarchical LOD, residency/streaming, then visibility-buffer evaluation and optional mesh-shader acceleration. Add virtual shadow pages or ReSTIR DI only when captures identify their payoff. **Exit:** better quality/performance than the conventional path on target scenes, including foliage and deformations; retain the conventional escape path.
5. **Phase N — UE-level targeted fidelity.** More sophisticated indirect/specular transport, bounded layered materials, skin/hair, richer volumetrics, advanced foliage/PCG integration, optional realtime path tracing and frame generation. **Exit:** parity on specified reference shots **and motion sequences** at declared hardware/budgets, with authoring and diagnostics usable by both humans and agents. This is selective visual parity, not a promise to reproduce every Unreal subsystem.

**Recommended stack at the roadmap's start:** NVRHI → D3D12/Vulkan; Slang → DXIL/SPIR-V with DXC interoperability; one typed material AST; one resource graph; conventional raster visibility plus selective hardware RT. Borrow fidelity algorithms incrementally rather than inheriting an entire engine framework.

**The five hardest rendering decisions to change later:**

1. **Platform and capability floor:** RT/bindless/mesh expectations versus Metal/mobile/web/VR reach; this shapes almost everything else.
2. **Material semantics and authoring representation:** domains, closure model, type system, deformation, and graph/text identity become content contracts.
3. **GPU scene, binding, and residency model:** stable IDs, handle lifetime, indirect access, and streaming ownership determine scalability and debugging.
4. **Frame/resource/history contract:** dependency ownership, camera/view identity, motion vectors, exposure, coordinate/depth/color conventions, and temporal resets underpin every reconstruction technique.
5. **Cooked asset and compilation contract:** geometry hierarchy, textures, shader specialization, content hashes, compiler versions, and PSO manifests determine whether future features require reprocessing an entire content library.

## Open questions / risks

- **Minimum platform/GPU is unresolved.** The recommendation intentionally trades initial reach for fidelity per engineer. If web/macOS/mobile/VR is mandatory, reevaluate before implementation; a portable API does not make identical high-end features portable.
- **“UE-level” needs a benchmark brief.** Define scenes, animation, camera motion, resolution, frame budget, memory, and art requirements. Without this, geometry or GI research can consume unlimited time without approaching a measurable product goal.
- **Content and technical art are major dependencies.** Great assets, correct import/scale/tangents, lighting composition, and sensible defaults can matter more than another rendering paper. No cited source establishes how many people or months snengine needs.
- **Temporal debt is the main fidelity risk:** GI, reflections, shadows, AA, upscaling, and particles may each look acceptable alone while their combined histories ghost or lag. Budget and test their interactions.
- **Backend/library maturity remains empirical.** No code was built or run and no renderer was benchmarked in this research. Slang target gaps, NVRHI integration, denoiser contracts, proprietary upscalers, and console access need focused spikes.
- **Unverified details:** current Nanite deformation compatibility matrix; universal shipping-quality radiance-cascade integration; DLSS 5 general integration availability; every feature promised for later Unity 6.x URP versions. These should not become architecture assumptions.

## Sources

All linked primary pages above were checked on 2026-10-22. Feature-status claims prefer dated release announcements over contradictory landing-page badges. Performance claims in vendor posts are scene-specific; none is used as snengine's predicted timing. Additional source anchors and reproducible local evidence follow.

### Local reference snapshots, inspected read-only

Completion markers existed before inspection. These are development snapshots, not pinned release checkouts:

- **O3DE:** `2462b90ee389939336931a14e234fc4e3b4cd534` (2026-10-19).
- **Flax:** `394658f3cba5002c6cc16f054b1443a1b6022f05` (2026-10-22).
- **Godot:** `9c2e39242e20846f84d990fe457a5e146b77fa36` (2026-10-22).
- **Unity C# reference:** `bf6028387a4fc2ea6d41aa7d8998e774655aedf6` (2026-10-22; identifies Unity 7000.0.0a12, so it is not evidence of shipped Unity 6 internals).

Local paths are relative to `D:\Coding\projects\desktop\snengine\`:

- **L1:** `.refs/o3de/Gems/Atom/RHI/Code/Include/Atom/RHI/FrameGraph.h` — device-facing frame graph declarations.
- **L2:** `.refs/o3de/Gems/Atom/RPI/Code/Include/Atom/RPI.Public/Pass/PassSystemInterface.h:41` — pass lifecycle/statistics, templates, hierarchy inspection.
- **L3:** `.refs/o3de/Gems/Atom/Feature/Common/Assets/Passes/MainPipeline.pass:24` — explicit pass requests and attachment connections; `.refs/o3de/Gems/Atom/RPI/Code/Include/Atom/RPI.Public/FeatureProcessor.h` — feature processor lifecycle.
- **L4:** `.refs/o3de/Gems/Atom/RPI/Code/Include/Atom/RPI.Edit/Material/MaterialTypeSourceData.h:36` — abstract/direct material-type distinction; `.refs/o3de/Gems/Atom/Feature/Common/Assets/Materials/Types/StandardPBR.materialtype` — versioned properties and imports.
- **L5:** `.refs/o3de/Gems/Atom/Tools/MaterialCanvas/Code/Source/Document/MaterialGraphCompiler.cpp:71` — graph traversal/template generation; line 149 onward exports generated shader and material assets.
- **L6:** `.refs/flax/Source/Engine/Renderer/Renderer.cpp:595` — frame orchestration, G-buffer, shadows, lighting, and effects.
- **L7:** `.refs/flax/Source/Engine/Renderer/GI/DynamicDiffuseGlobalIllumination.h:14` and corresponding `.cpp:42` — probe constants, tracing/update limits, surface-atlas/SDF dependencies; `.refs/flax/Source/Engine/Renderer/Utils/GlobalSignDistanceFieldPass.cpp` — software scene representation.
- **L8:** `.refs/godot/servers/rendering/rendering_device.h:1278` — ray-tracing pipeline/SBT declarations; `.refs/godot/servers/rendering/renderer_rd/environment/gi.cpp` — existing GI implementations.
- **L9:** `.refs/godot/servers/rendering/rendering_device_graph.h:783` and `:961` — barriers and reorder control; `.refs/godot/servers/rendering/renderer_rd/forward_clustered/render_forward_clustered.cpp` and `forward_mobile/render_forward_mobile.cpp` — rendering-method boundaries.
- **L10:** `.refs/godot/modules/visual_shader/visual_shader.cpp:1809` and `:2290` — graph evaluation/code generation and final shader source assignment.
- **L11:** `.refs/unity-cs-reference/Runtime/Export/RenderPipeline/RenderPipeline.cs:10` and `ScriptableRenderContext.cs:139` — managed pipeline entry points, submission, and renderer-list APIs; engine-native backend implementation is outside this reference source.

### Principal external implementation/reference entry points

- Epic: [UE 5.8 release](https://www.unrealengine.com/news/unreal-engine-5-8-is-now-available), [UE 5.7 release](https://www.unrealengine.com/news/unreal-engine-5-7-is-now-available), and feature-specific documentation linked in the Unreal section.
- Unity: [2026 rendering strategy](https://unity.com/topics/render-pipelines-strategy-for-2026), [Render Graph updates](https://discussions.unity.com/t/render-graph-updates-in-unity-6-3/1668122), [Graphics source](https://github.com/Unity-Technologies/Graphics).
- Practical engines: [Flax 1.13](https://flaxengine.com/blog/flax-1-13-released/), [Godot 4.7](https://godotengine.org/releases/4.7/), [O3DE 26.05](https://www.docs.o3de.org/docs/release-notes/2605-0-release-notes/), [Bevy 0.19](https://bevy.org/news/bevy-0-19/).
- Foundation candidates: [NVRHI](https://github.com/NVIDIA-RTX/NVRHI), [Diligent](https://github.com/DiligentGraphics/DiligentCore), [The Forge](https://github.com/ConfettiFX/The-Forge), [SDL GPU](https://wiki.libsdl.org/SDL3/CategoryGPU), [wgpu](https://docs.rs/wgpu/latest/wgpu/struct.Features.html), [Dawn](https://dawn.googlesource.com/dawn), [bgfx](https://github.com/bkaradzic/bgfx), [slang-rhi](https://github.com/shader-slang/slang-rhi).
- Shader/lighting implementations: [Slang](https://github.com/shader-slang/slang), [DDGI](https://github.com/NVIDIAGameWorks/RTXGI-DDGI), [RTXDI](https://github.com/NVIDIA-RTX/RTXDI), [NRD](https://github.com/NVIDIA-RTX/NRD), and the original papers linked in the lighting ranking.
