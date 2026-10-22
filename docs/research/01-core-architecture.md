# R01: Core engine architecture

*Research track R01 for snengine. Written 2026-10-22. Clone evidence comes from `.refs/` (Godot master = 4.8-beta per `godot/version.py`, Flax master = 1.13 per `flax/Flax.flaxproj`, O3DE `development`, Unity C# reference = `7000.0.0a12` per `unity-cs-reference/README.md`). Claims marked **[unverified]** could not be confirmed against a primary source.*

---

## TL;DR

1. **The industry is converging on one object model, so adopt it from day one**: an authoring layer of *entities in a tree with components* (Unity's ergonomics) stored in *data-oriented ECS storage* (DOTS/Mass/Bevy/flecs performance). Unity 7 is making "GameObjects are Entities" (EntityId is already 64-bit generational in the 7.0 alpha source). UE6 is replacing Actors/Blueprints with Verse + Scene Graph entities/components/prefabs. Bevy layers BSN scenes over ECS. snengine can start where these engines are trying to get to, without carrying two object models.
2. **Use one reflection source of truth and generate everything from it**: serialization, inspector, undo, script bindings, replication, docs, JSON Schema, TypeScript types and MCP tools. UE 5.8's experimental Unreal MCP already derives tool JSON Schemas from UHT reflection plus doc comments, which shows the idea works. Godot's `extension_api.json` shows the right *output*: a machine-readable reflection DB that external tools consume.
3. **C++26 reflection is not shippable for a Windows-first engine in 2026–27.** GCC 16.1 has it behind a flag. Clang mainline is still upstreaming it. MSVC has no public support and no ETA. Use a **small header-scanning codegen tool now** whose attribute syntax is *typed C++ expressions*, so that migrating to P2996 + P3394 annotations later is mechanical and the reflection DB format doesn't change.
4. **Every on-disk source format should be canonical, deterministic, schema-validated JSON.** Entities and overridable collection items are keyed by stable short IDs and never by array index. Prefab overrides are ID-addressed property patches, not O3DE-style RFC 6902 index paths or Unity-style stringly `PropertyModification`s. Binary formats exist only as cooked or cache output.
5. **Mutation goes through transactions of patch ops.** The same op format serves undo/redo, prefab overrides, editor↔play-process sync, AI edits and (in binary form) network deltas. Never write do/undo pairs by hand: Godot's editor has ~1,334 of them.
6. **The world protocol is the AI-native backbone**: JSON-RPC over WebSocket/stdio, generated from reflection, used by the editor UI itself, the play process, the CLI and an MCP adapter. Copy UE 5.8's tool-search pattern (`list/describe/call`) so agents don't drown in schemas.
7. **Keep the kernel small** (platform, memory, jobs, reflection, ECS world, serialization, assets, transactions, protocol). Everything else is a statically linked module that registers types, systems and commands through reflection, which makes it AI-exposed automatically. Defer a third-party native ABI. When it's needed, copy GDExtension (C ABI plus a JSON-described interface).
8. **Process model**: the engine is a library. The editor hosts the edit world. Play mode runs in a **child process** (Godot's model, for crash isolation and clean state) that you inspect and patch over the same protocol the agents use. Asset importers are pure functions running in worker processes and feeding a content-addressed cache.
9. **Simulation is deterministic by construction on a given platform** (fixed step, ordered systems, seeded RNG streams, recorded input). Agents get reproducible test loops ("run 300 frames with this input, assert X"), and you get replays and a path to rollback netcode.

---

## 1. How each reference engine does it

### 1.1 Object / world model

| Engine | Core unit | Composition | Hierarchy | Storage | Direction (2026) |
|---|---|---|---|---|---|
| **Unreal 5.8** | `UObject` → `AActor` + `UActorComponent` | Components on actors, plus deep class inheritance (`ACharacter`…) | Two kinds: actor attachment and `USceneComponent` attachment | Heap objects, GC; Mass for crowds | UE 5.8 (June 2026) is the last UE5. UE6 Early Access is targeted at end of 2027, with Verse + **Scene Graph** (entities/components/prefabs) eventually replacing Actors/Blueprints ([State of Unreal 2026](https://www.unrealengine.com/news/state-of-unreal-2026-top-news-from-the-show); [Scene Graph docs](https://dev.epicgames.com/documentation/fortnite/scene-graph-in-unreal-editor-for-fortnite); the replacement statement is reported secondhand, **[partly unverified]**) |
| **Unity 6.x / 7α** | `GameObject` + `Component`/`MonoBehaviour` | Flat components | `Transform` parent chain | Native objects plus managed wrappers; DOTS is a separate world reached through baking | **"GameObjects are Entities"** with chunk storage, a unified transform and unified authoring ([ECS for all thread](https://discussions.unity.com/t/2026-ecs-for-all/1732447)). The 7.0α source moves Entities into core (`Modules/Entities/Managed/`, including `CompanionGameObjectBindings.bindings.cs`), and `EntityId` is a 64-bit `[Version:24 \| TypeId:12 \| Index:28]` handle (`Modules/Entities/Public/EntityId.bindings.cs:21,60-65`) |
| **Godot 4.7** | `Object` → `Node` | **Child nodes are the composition**, with one script per node | The scene tree *is* the composition | Nodes front "servers" (Rendering/Physics) addressed by RIDs | 4.6 added persistent node IDs (`unique_id=` in `.tscn`) so references survive renames, plus LibGodot embedding ([4.6 notes](https://godotengine.org/releases/4.6/)). 4.7 shipped 2026-06-19 ([4.7](https://godotengine.org/releases/4.7/)) |
| **O3DE 25.10** | `AZ::Entity` + `AZ::Component` | Flat components; messaging over EBus and `AZ::Interface` | Via `TransformComponent` | Heap components | Separate *editor* component classes compile into *game* components via `BuildGameEntity` (`EditorComponentBase.h:202`; 65 overriding headers in `Gems/`). Two messaging systems coexist (840 headers declare EBuses, 752 files use `AZ::Interface<>`). Latest release found: 25.10.x (Oct 2025) ([notes](https://docs.o3de.org/docs/release-notes/)); no 2026 release found **[unverified]** |
| **Flax 1.12** | `Actor` (has a transform) + `Script` | Scripts attach to actors, but features are Actor *subclasses* (lights, models…) | Actor tree | Heap objects; C++ and C# peers | Unity-like UX, Unreal-like class tree. 1.12 added DDGI and WebGPU ([Flax blog](https://flaxengine.com/blog/tag/flax/)) |

**Beyond the five engines:**
- **Bevy 0.20** (2026-10-08) is pure ECS with relationships (`Children`, custom ones) and observers. BSN scenes arrived in 0.19 as *composable patches* defined with `bsn!`, but the on-disk `.bsn` asset format is still "what's next" ([0.19](https://bevy.org/news/bevy-0-19/), [0.20](https://bevy.org/news/bevy-0-20/)).
- **flecs v4** has first-class relationships. Prefabs are entities, and instances `IsA` the prefab, *sharing* components until they are overridden (copy-on-override, auto-override, variants) ([Prefabs manual](https://www.flecs.dev/flecs/PrefabsManual.html)).
- **s&box** has a Unity-style GameObject/Component model in C#, JSON scenes and millisecond C# hotload. It was open-sourced under MIT in 2026 ([docs](https://sbox.game/dev/doc/getting-started/), [GFS](https://gamefromscratch.com/sbox-game-engine-open-sourced/)).
- **Fyrox** uses node trees with *per-property inheritance flags* (`InheritableVariable`) for prefabs ([book](https://47x5b-baaaa-aaaak-ao3zq-cai.icp0.io/scene/inheritance.html)).
- **Stride** uses Entity + EntityComponent + processors, with YAML archetypes/prefabs where overridden properties carry a `*` and collection items carry IDs ([asset introspection](https://doc.stride3d.net/latest/en/manual/engine/asset-introspection.html)).

**What makes these easy or painful:**

- **For humans**, the easiest are Unity and s&box: a flat "thing + nouns attached to it" model, a visual hierarchy, and predictable lifecycle callbacks. Godot is easy to *start* with because it has one concept, but composition-through-children produces deep trees and `NodePath` string coupling. Unreal's power comes with a large tax: UObject/CDO/subobject rules, GC, two attachment systems, and the `AActor` god class. O3DE's editor/game component split doubles the class count for every feature.
- **For AI agents**, what matters is a **small vocabulary**, **everything addressable by stable IDs**, **all state reachable through reflection** (no hidden native state), **text serialization** and **machine-readable errors**. Godot scores well on legibility (readable `.tscn`) but poorly on addressing (paths) and undo (manual). Unity scores well on uniform `SerializedProperty` paths but badly on files (opaque fileIDs, YAML noise, `.meta`). Unreal is opaque offline because its assets are binary. Bevy and flecs score best structurally: everything is data, queryable, and remotely accessible (BRP, flecs REST). They are weakest on human editor UX.

### 1.2 Reflection and type systems

| Engine | Mechanism | What it drives | Problems |
|---|---|---|---|
| **Unreal** | `UCLASS/UPROPERTY/UFUNCTION` macros, parsed by UHT (a custom C# header parser) into `.gen.cpp` ([UHT docs](https://dev.epicgames.com/documentation/en-us/unreal-engine/unreal-header-tool-for-unreal-engine)) | GC, serialization, details panel, Blueprints, replication, Python, undo transactions, and in 5.8 **MCP tools**: `UFUNCTION(meta=(AICallable))` on a `UToolsetDefinition`, with JSON Schema generated from reflected param types and doc comments ([Unreal MCP](https://dev.epicgames.com/documentation/unreal-engine/unreal-mcp-in-unreal-editor?lang=en-US)) | `meta=` values are untyped strings. Custom-parser syntax limits. Live Coding can't add `UFUNCTION`s, so even a new MCP tool needs an editor restart |
| **Godot** | Manual registration: `GDCLASS` (`core/object/object.h:245`), `ClassDB::bind_method`, `ADD_PROPERTY(PropertyInfo(...))` | **One `PropertyInfo` drives storage and editor through usage flags** (`PROPERTY_USAGE_STORAGE \| EDITOR`, `property_info.h:92-125`). The whole ClassDB is dumped to `extension_api.json` (`core/extension/extension_api_dump.cpp:104`), which feeds godot-cpp, godot-rust and C#. The extension C interface itself is JSON with a **JSON Schema** (`gdextension_interface.schema.json`). API compatibility is checked by diffing old/new dumps (`extension_api_dump.cpp:1612-1642`) | Registration duplicates the declarations. Hints are stringly (`PROPERTY_HINT_RANGE` + `"0,100,1,or_greater"`). Undo is not derived from reflection |
| **O3DE** | `AZ_RTTI` UUIDs, then **three separate registrations**: `SerializeContext` (fields + version converters), `EditContext` (UI), `BehaviorContext` (script). See `AzFramework/Components/TransformComponent.cpp:835-895`: ~50 hand-listed `->Event("SetWorldX", …)` lines | Serialization (JSON + binary), property editor, Lua/ScriptCanvas, Python | Triple bookkeeping drifts. Class UUIDs leak into files. The modern **Document Property Editor** (`AzFramework/DocumentPropertyEditor/`) is the good part: the inspector is a *DOM generated by adapters*, and edits are `Dom::Patch`es (`AzCore/DOM/DomPatch.h`) |
| **Flax** | `API_CLASS/API_FIELD/API_FUNCTION` are **empty macros** (`Source/Engine/Core/Config.h:50-60`) parsed by Flax.Build (`Source/Tools/Flax.Build/Bindings/BindingsGenerator.{Cpp,CSharp}.cs`) | C++↔C# glue and auto-serialization (`API_AUTO_SERIALIZATION`); the C# editor builds inspectors from C# reflection | Editor metadata is **C# attribute source pasted into C++ strings** (`Attributes="EditorOrder(-5), …"` appears 1,182 times in `Source/Engine`), which the C++ compiler never checks |
| **Unity** | C# reflection plus serialization rules (`[SerializeField]`, `[SerializeReference]`) | `SerializedObject`/`SerializedProperty` is one uniform path for inspector, undo (`Undo.RecordObject` snapshots, 265 call sites in `Editor/`), prefab overrides and multi-edit | Serializer limits lasted 15 years: native `Dictionary` serialization only arrives in 6.6 ([CoreCLR/serialization update, June 2026](https://discussions.unity.com/t/coreclr-scripting-and-serialization-update-june-2026/1723299)). The managed/native split causes "fake null". Domain reloads are only removed in 7.0 |
| **Bevy** | `#[derive(Reflect)]` + `TypeRegistry` | Scenes, BRP JSON-RPC (`bevy/registry/schema`), inspector crates | Schema incompleteness: the `bevy_brp_mcp` maintainers say the registry schema "doesn't fully show what spawn/insert/mutate expect", so agents fall back to trial and error and a `discover_format` helper ([docs.rs](https://docs.rs/crate/bevy_brp_mcp/0.22.3)). **Lesson: the AI schema must be complete and validating, not just descriptive.** |
| **flecs** | `flecs.meta` runtime reflection; components can be **described at runtime** | JSON serialization of the world, queries and entities; REST (`/entity`, `/component`, `/query`, `/type_info`, `/script`, …); the Explorer web UI ([Remote API](https://www.flecs.dev/flecs/FlecsRemoteApi.html)) | C-flavoured API; the editor is minimal |

**C++26 static reflection status (verified Oct 2026):**
- P2996 (reflection) and P3394R4 (annotations, `[[=expr]]`) were voted into C++26 at Sofia in June 2025 ([P3394R4](https://isocpp.org/files/papers/P3394R4.html); [Fertig](https://isocpp.org/blog/2025/09/cpp26-reflection-at-compile-time-andreas-fertig)).
- GCC 16.1 (April 2026) ships reflection behind `-freflection`.
- Clang mainline is still upstreaming it. Bloomberg's `clang-p2996` fork is "highly experimental".
- As of 2026-10-10, clang-cl 23.1 lists `-freflection` but ignores it.
- **MSVC has no public support and no ETA** ([OloEngine tracker #688](https://github.com/drsnuggles8/OloEngineBase/issues/688); [cppreference](https://en.cppreference.com/cpp/compiler_support/26); [Whole Tomato](https://www.wholetomato.com/blog/cpp26-reflection-large-codebases/)).
- Annotations on function parameters are still a gap ([P3795R2](https://www.open-std.org/jtc1/sc22/wg21/docs/papers/2026/p3795r2)).
- **Verdict:** design *for* P2996 + P3394, but don't depend on them before roughly 2028 on Windows.

**Codegen vs macros vs language reflection:**
- **Registration DSLs** (Godot, O3DE) need no build step but duplicate every declaration and drift.
- **Header-scanning codegen** (UHT, Flax.Build) has one source and is proven at AAA scale. Its costs are a parser to maintain, syntax restrictions inside marked regions and a build step.
- **Language reflection** (C#, Rust derive, C++26) is the ideal end state.

The decisive factor isn't the mechanism. It's **what the mechanism outputs**: a *complete, typed, machine-readable reflection DB*. Godot's API dump and UE 5.8's schema-from-reflection are the precedents.

### 1.3 Serialization and file formats

**Unity: YAML plus `.meta` GUIDs.**
- Each object is a YAML document (`--- !u!1 &<fileID>`) with class IDs and `m_` field names. Every asset has a `.meta` sidecar holding its GUID and import settings.
- Prefab overrides are a list of `PropertyModification { target, string propertyPath, string value, objectReference }` (`Editor/Mono/Prefabs/PropertyModification.bindings.cs:17-26`). Values are strings, and paths include array indices (`m_Materials.Array.data[0]`).
- *Good:* text; a uniform property path.
- *Bad:* noisy and opaque (fileIDs), merge conflicts need UnityYAMLMerge, orphaned or forgotten `.meta` files, and index-based overrides break when lists reorder.

**Godot: `.tscn` / `.tres` text.**
- An INI-like format. Nodes are flat, with `parent="."` paths, and now `unique_id=` too (`scene/resources/resource_format_text.cpp:1760`). Resource identity is `uid://` in the header (`:2161`). Non-imported files get `.uid` sidecars (`core/io/resource_loader.cpp:123`) and imported ones get `.import` sidecars. Instances store only changed properties.
- Sample (`modules/gdscript/tests/.../argument_options.tscn`):
  ```
  [gd_scene format=3 uid="uid://dl28pdkxcjvym"]
  [node name="GetNode" type="Node" unique_id=876232322]
  [node name="AnimationPlayer" type="AnimationPlayer" parent="." unique_id=1664998033]
  ```
- *Good:* the most human-readable and compact format of the group, with low noise.
- *Bad:* custom syntax that no generic tool or JSON Schema understands, path-based parenting, and two kinds of sidecar.

**O3DE: prefab JSON.**
- `Assets/Editor/Prefabs/Empty.prefab` is a single *empty* entity serialized with **10 editor components** (`EditorLockComponent`, `EditorVisibilityComponent`…), each with a random 64-bit `Id`, and `"$type": "{27F1E1A1-…} TransformComponent"`.
- Nested-instance overrides are **RFC 6902 JSON Patch with array-index paths**. For example, `NavigationSample.prefab:955-967` contains `{"op":"remove","path":"/ContainerEntity/Components/Component_[17136984997565909930]/Child Entity Order/7"}`.
- *Good:* standard JSON and standard patch semantics.
- *Bad:* the noise is hostile to humans and token-expensive for AI. Index paths are fragile. Component keys are random numbers.

**Unreal: binary `.uasset` / `.umap`.**
- These can't be diffed, merged or read without the editor, which drives an exclusive-checkout culture.
- One File Per Actor splits levels into per-actor external files (editor only, the default with World Partition) purely to reduce contention ([OFPA](https://dev.epicgames.com/documentation/en-us/unreal-engine/one-file-per-actor-in-unreal-engine)). The resulting file names are hashed and illegible.
- *Good:* load speed and robustness at scale.
- *Bad:* the worst of the group for AI. Agents need a live editor for everything.

**Flax: JSON scenes and JSON assets with `Guid` IDs.**
- Objects serialize **only their delta against the prefab object** (`Serialize(stream, otherObj)`; `PrefabID`/`PrefabObjectID`/`ParentID` in `Source/Engine/Level/SceneObject.cpp:95-118`).
- *Good:* simple and diff-friendly.
- *Bad:* GUID noise; meshes and textures are binary `.flax` packages.

**Bevy, flecs and Stride:**
- **Bevy BSN:** scenes are patch layers over defaults, and relationships and `#Name` references are first-class. There is no file format yet.
- **flecs:** the scripting DSL plus JSON. Prefab variants inherit and override per component.
- **Stride:** YAML with `*` override markers and **`CollectionItemIdentifiers`**, which give collection items stable IDs so that overrides survive reordering. This is exactly the fix for the Unity and O3DE index problem.

**What's best for AI and humans together:** a standard syntax (JSON, so JSON Schema, structured outputs and every language's parser all work), *low noise* (Godot-level: omit defaults, no editor-only cruft, readable type names), *stable short IDs as map keys* (Stride/Godot 4.6), and *deterministic output* (so diffs show only intent).

### 1.4 Asset pipeline

**Unity:**
- AssetDatabase v2 turns source + `.meta` into importer artifacts in `Library/`, keyed by a hash of dependencies.
- **Import runs in parallel worker processes** (Desired Import Worker Count, about 25% of cores by default) ([Parallel Import](https://docs.unity3d.com/Manual/ParallelImport.html)), with an optional shared cache (Accelerator).
- *Pain:* `Library/` rebuilds and `.meta` hygiene.

**Godot:**
- An `.import` sidecar holds settings, and `.godot/imported/` is the cache (importers live in `editor/import/`). It's editor-only, and the remaps are used at export.
- Simple and transparent, but there is no shared cache.

**O3DE:**
- The **Asset Processor is a separate always-on application** with builder workers (`Code/Tools/AssetProcessor/{AssetBuilder,AssetBuilderSDK}`) and a per-platform `Cache/` of products with sub-IDs.
- Robust and scriptable, but heavyweight, and a frequent source of "why isn't my asset showing up".

**Unreal:**
- Import is a one-time conversion into `.uasset`, so the source file stops being authoritative and reimport causes friction.
- Derived data (shaders, platform textures) lives in the DDC, which is served by **Zen Storage Server** since 5.4 as a local or shared HTTP service ([DDC](https://dev.epicgames.com/documentation/en-us/unreal-engine/using-derived-data-cache-in-unreal-engine)). Cooking is per platform.

**Flax:** imports into binary `.flax` packages (UE-like), plus JSON assets.

**Hot reload:**
- Assets are fine everywhere: watch, reimport, swap.
- Code reload is the hard part:
  - UE Live Coding (Live++) can't add reflected members.
  - Unity is removing domain reload in 7.0 (CoreCLR); 6.6 already defaults Play Mode to "scene reload only".
  - s&box hot-loads C# "within a few milliseconds".
  - Rust gets ~150 ms hot-patching through Dioxus `subsecond`. It's used experimentally in Bevy via `bevy_hotpatching_experiments` ([docs.rs](https://docs.rs/crate/bevy_hotpatching_experiments/latest)); not upstream **[unverified]**.
- **Lesson:** reload is easy when *state lives in reflected data* and *behavior is swappable code*. That is the ECS split.

### 1.5 Modules and plugins

- **Unreal:** modules plus `.uplugin` JSON with loading phases. Powerful, but heavy and slow to build.
- **Godot:**
  - Compile-time *modules*, plus **GDExtension**: a stable C ABI described by `gdextension_interface.json` with a JSON Schema and compatibility hashes (`core/extension/`).
  - This is the best extension boundary in the group: language-neutral and versioned.
  - LibGodot (4.6) lets the engine be embedded as a library.
- **O3DE:** **everything is a Gem** (79 `gem.json` in-repo) with CMake per gem. It's maximally modular, but configuration overhead and two messaging systems make the core hard to see.
- **Unity:** UPM packages (`package.json`, semver, registries) plus asmdefs. The cautionary tale is that putting core features in out-of-band packages created a version matrix, and Unity is now folding them back: Entities, Collections and Mathematics became core packages in 6.4, and Burst became a built-in module in 6.6 ([ECS status Dec 2025](https://discussions.unity.com/t/ecs-development-status-december-2025/1699284); [CoreCLR update](https://discussions.unity.com/t/coreclr-scripting-and-serialization-update-june-2026/1723299)).
- **Flax:** plugins are projects referenced by `.flaxproj`, with C# `GamePlugin`/`EditorPlugin` entry points. Lightweight and pleasant.

### 1.6 Runtime core

**Jobs:**
- UE moved from TaskGraph to `UE::Tasks`.
- Unity has the C# Job System + Burst, with a safety system for dependency checks.
- Godot has `WorkerThreadPool` (`core/object/worker_thread_pool.h`).
- O3DE has *both* legacy `AzCore/Jobs` and `AzCore/Task/TaskGraph`. That's a recurring O3DE pattern of two systems for one job.
- Flax has a minimal `JobSystem::Dispatch(job, dependencies, count)` (`Source/Engine/Threading/JobSystem.h:22-50`).
- The Bevy executor **derives parallelism from each system's declared data access**, and flecs runs a pipeline over a lockless scheduler. Declared access is the design to steal: it's safe parallelism without locks, and the same declarations tell an agent what a system touches.

**Threading:** every engine has a main/game thread, a render thread (or render pipelining) and workers. Extracting a render snapshot (Bevy "extract", UE render proxies, Godot servers) is the common pattern for decoupling simulation from rendering.

**Determinism:** none of the five engines is deterministic by default. Unity Physics (DOTS) is deterministic per platform, and Netcode for Entities does prediction. Most engines simply don't guarantee it.

**Editor/runtime sharing:**
- All five ship the editor as "runtime + editor modules" in one binary.
- **Play mode** differs:
  - UE PIE duplicates the world in-process.
  - Unity runs in-process, historically with domain reloads.
  - Flax runs in-process.
  - **Godot always runs the game as a separate process** "to keep the editor alive if the game crashes", with a remote scene tree and, since 4.4, window embedding ([4.4](https://godotengine.org/releases/4.4); [game embedding docs](https://docs.godotengine.org/de/4.x/tutorials/editor/game_embedding.html)).

### 1.7 Networking implications for the object model

All replication systems need four things: **stable network IDs per entity**, **per-property metadata** (replicated, quantization, interpolation), **cheap change detection**, and an **authority model**.
- UE: `UPROPERTY(Replicated)` + RPC `UFUNCTION`s, with **Iris** production-ready in 5.8 ([5.8 announcement](https://www.unrealengine.com/news/unreal-engine-5-8-is-now-available)).
- Unity: `[GhostField]` in Netcode for Entities and `NetworkVariable` in NGO.
- s&box: `[Sync]`.
- Godot: `MultiplayerSynchronizer` with property paths.

The object-model consequences:
- Replication flags belong in reflection.
- Entities need IDs that are mappable across processes.
- ECS change ticks are the cheapest dirty tracking available.

The same three properties also serve the editor↔play-process link and AI edits.

---

## 2. State of the art beyond these engines

1. **Hybrid convergence.** Unity 7 ("GameObjects are Entities"), UE6 (Scene Graph entities/components/prefabs; components in Verse; constructed entities don't auto-get a Transform ([UEFN SG](https://dev.epicgames.com/documentation/fortnite/getting-started-in-scene-graph-in-fortnite))), Bevy (ECS + BSN) and s&box (GameObject/Component) all land on the same model: ergonomic entity/component authoring over data-oriented storage. Nobody is building a new deep-inheritance actor model.
2. **Editors built around a data model.** The Machinery's *The Truth* centralized all editable data in one versioned store, so undo, redo and real-time collaboration fell out of the history instead of the Command pattern ([Story behind The Truth](https://gamedeveloper.com/programming/the-story-behind-the-truth-designing-a-data-model)). O3DE's DOM + Document Property Editor is a newer, incomplete version of the same idea.
3. **Remote world protocols are now how agents get into engines.**
   - flecs REST + Explorer.
   - Bevy BRP (JSON-RPC) and `bevy_brp_mcp`.
   - **Unreal MCP (5.8, experimental):**
     - Tools are reflected `UFUNCTION`s.
     - Schemas come from reflected types and doc comments.
     - `tools/list` returns three meta-tools (`list_toolsets`, `describe_toolset`, `call_tool`) instead of hundreds of schemas.
     - Calls run serially on the game thread.
     - It is loopback-only with no auth ([docs](https://dev.epicgames.com/documentation/unreal-engine/unreal-mcp-in-unreal-editor?lang=en-US)).
   - **Unity MCP** (open beta since May 2026, part of the AI Assistant package, subscription-gated) ([Unity blog](https://unity.com/blog/unity-ai-mcp-how-to-get-started)).
   - Community Roblox Studio MCP servers wrap writes in `ChangeHistoryService` so agent edits are undoable ([listing](https://glama.ai/mcp/servers/kwqaglxiux)).

   Every one of these bolts AI onto an engine that wasn't designed for it. snengine can make the protocol the engine's own primary API.
4. **Language-level reflection.** Rust derive (Bevy), C# source generators, C++26 P2996/P3394 (2027+ on MSVC at the earliest **[unverified ETA]**), Zig comptime.
5. **Content-addressed derived-data caches** (UE Zen, Unity Accelerator, Bazel-style action caches): importers as pure functions keyed by input hash.
6. **Fast code reload:** Live++ for C++, .NET hotload (s&box), `subsecond` for Rust. All are much easier when gameplay state is plain reflected data.

---

## 3. Recommendations for snengine

### 3.1 Object model: entities in a tree, components as data, ECS underneath

```
Entity     = runtime: 64-bit generational handle   [gen:24 | index:40]
             files:   short stable id (sid), unique within its file
Component  = reflected plain struct (public fields only), stored in archetype/sparse ECS storage
Behavior   = a component with lifecycle hooks (onStart/onUpdate/onEvent): "scripts are components", Unity-style
System     = function with declared reads/writes, auto-scheduled in parallel (engine subsystems, power users)
Hierarchy  = built-in ChildOf relationship + sibling `order` key; Transform is an ordinary optional component
Prefab     = an asset containing an entity subtree; instances are patch layers over it (nestable, variants)
Event      = typed events + observers (Bevy/flecs style); no string buses, no EBus
Asset      = reflected resource referenced by handle; hot-swappable in place
```

**Steal:**
- Unity's flat, noun-like components and "scripts are components".
- flecs/Bevy relationships, observers and declared-access scheduling.
- Unity 7's generational `EntityId` as the *only* identity scripts see (no managed wrapper objects, so no fake null).
- Godot's split between "authoring objects" and "servers": render, physics and audio keep their own data-oriented internal scenes, synced from components through change detection (extract).
- Fyrox and flecs per-property/per-component inheritance for prefabs.

**Avoid:**
- Deep class hierarchies for features (UE `AActor`, Flax `Actor` subclasses). A light is an entity with a `Light` component.
- Separate editor/game component classes (O3DE `BuildGameEntity`). Editor-only *fields* are flagged in reflection, and editor-only *components* are stripped at cook. There is one class per concept.
- Composition by child nodes (Godot), which conflates spatial hierarchy with composition.
- Two messaging systems (O3DE).

**Pillars:**
- *AI-native:* the vocabulary is about eight nouns. All state is data reachable through reflection. ECS queries (`with Health, without Dead`) make a natural, precise agent query language.
- *Unity-level UX:* it's the GameObject/Component mental model users already know. ECS is an implementation detail beginners never have to see, though they can write systems when they want to.
- *Unreal-level fidelity:* data-oriented storage scales to Mass-style crowds and large worlds without a second object model.
- *Great editor:* one generic inspector path for everything.

### 3.2 Reflection: one schema, many projections

**Authoring (C++ today).** Use a tiny codegen tool (`snreflect`, UHT/Flax.Build-style but minimal) that scans for markers. **Attribute arguments are typed C++ expressions copied verbatim into generated code**, so the compiler type-checks them. That avoids Flax's `Attributes="…"` strings and Godot's `"0,100,1"` hint strings, and it mirrors P3394 annotations one-to-one.

```cpp
/// Hit points of a damageable entity. Emits `Died` when current reaches 0.
SN_COMPONENT()
struct Health {
  /// Maximum hit points.
  SN_FIELD(sn::Range{1, 10'000}, sn::Unit::HitPoints, sn::Replicate{})
  float max = 100;

  /// Current hit points, clamped to [0, max].
  SN_FIELD(sn::Replicate{})
  float current = 100;

  float regenTimer = 0;   // unmarked: not saved, not shown, not exposed
};

/// Apply damage and return the remaining hit points.
SN_COMMAND(sn::AgentCallable{}, sn::Undoable{})
float damage(sn::Mut<Health> target, float amount);
```

```cpp
// health.gen.h (generated). Later produced by a consteval function over C++26 reflection.
template <> struct sn::Reflect<Health> {
  static constexpr auto fields = std::tuple{
    sn::field<&Health::max>("max", sn::Range{1, 10'000}, sn::Unit::HitPoints, sn::Replicate{}),
    sn::field<&Health::current>("current", sn::Replicate{}),
  };
};
```

The migration target is `[[=sn::Range{1, 10'000}]] float max = 100;` with P2996. The tool gets deleted, and the attribute types and the reflection DB stay the same.

**Rules that keep it simple:**
- Reflected types are aggregates with public fields. Behavior lives in functions and systems.
- **Doc comments are required on public types and fields**, because they feed the inspector tooltips, docs, JSON Schema `description` and MCP tool text (UE 5.8 does exactly this).
- Type identity is the stable qualified name (`physics.RigidBody`), with explicit `sn::Aliases{"OldName"}` for renames. No per-class UUIDs as in O3DE.
- Every type has a version and optional upgrade functions (O3DE's `->Version(5, converter)` is the right idea).
- **The runtime `TypeRegistry` accepts dynamic types.** Script types and *data-only components defined from a JSON Schema at runtime* (flecs-style) register through the same API. An agent or designer can define `Loot { rarity: enum, weight: f32 }` live, without compiling.

**Output: the Reflection DB (`.sn/reflection.json`).** It is emitted at build time, so agents can read it offline without running the engine, and it is also served live by the protocol:

```json
{
  "types": {
    "Health": {
      "kind": "component", "module": "gameplay", "version": 1,
      "doc": "Hit points of a damageable entity. Emits `Died` when current reaches 0.",
      "fields": {
        "max":     { "type": "f32", "default": 100, "range": [1, 10000], "unit": "hp", "replicate": true, "doc": "Maximum hit points." },
        "current": { "type": "f32", "default": 100, "replicate": true, "doc": "Current hit points, clamped to [0, max]." }
      }
    }
  },
  "commands": {
    "damage": { "params": { "target": "Health", "amount": "f32" }, "returns": "f32", "agentCallable": true, "undoable": true }
  }
}
```

**Projections, all generic and all generated from the DB or the runtime registry:**

| Projection | Mechanism |
|---|---|
| Serialization | One generic visitor writes the canonical JSON (§3.3) and the cooked binary |
| Inspector | A DOM built from type info + attributes (O3DE DPE idea), with custom widgets keyed by type or attribute. Works in-process and remotely |
| Undo/redo | Automatic inverse patch ops (§3.4). No hand-written undo |
| Script bindings | Generated from the DB (Godot `extension_api.json` model) for whichever gameplay language is chosen, plus `.d.ts` / C# types |
| Replication | `Replicate`, `Quantize` and `Interpolate` attributes plus ECS change ticks |
| AI | JSON Schema per type, TypeScript types, MCP tools from `SN_COMMAND(sn::AgentCallable{})`, and docs, all generated |
| Validation | Range, enum, reference and required checks, used by the editor, the CLI and the protocol alike |
| Visual scripting / graphs | Commands and fields become graph nodes and pins (the Blueprint/BehaviorContext role) |

### 3.3 File formats: canonical JSON, keyed by stable IDs

**Decisions:**
- **Strict JSON (RFC 8259)** for every source asset snengine owns: scenes, prefabs, materials, UI, settings and project files. A deterministic writer emits fields in reflection order, with shortest round-trip floats, 2-space indentation, LF line endings and a trailing newline, and **omits default and inherited values**. Files therefore contain only intent, and AI token cost stays low.
- Every file has a `$schema` pointing at the generated schema. Editors (VS Code), agents and CI can then validate files with no engine running.
- **Entities are a map keyed by `sid`, not an array.** Parenting is a `parent` field plus a fractional-index `order` string, so reparenting or reordering changes one line. Compare O3DE's `Child Entity Order` array, which produces index patches.
- Readable type names as keys (`"Health"`), with no `$type` UUIDs and no editor cruft.
- **References are ID-authoritative and path-annotated:** `{ "id": "a_91fd02", "path": "prefabs/rifle.prefab" }`. On input, the protocol and loader also accept path-only references and fill in the ID. Agents can therefore write references by path, while humans and AI can *read* what a file references. The editor rewrites stale path hints on save, and the validator flags them.
- Assets that snengine owns carry their `id` inside the file. **Only imported foreign sources** (`.png`, `.glb`, `.wav`) get a sidecar, `rock.glb.import.json`, holding the ID and importer settings. `sn mv` and the editor move sidecars together, and `sn validate` reports orphans.
- **Prefab overrides are an ID-path map**, never index-based:

```json
{
  "$schema": "sn://schema/scene/1",
  "id": "s_4f7a2c9e",
  "entities": {
    "k3q9": { "name": "Player", "order": "a0",
      "components": { "Transform": { "position": [0, 1, 0] }, "Health": { "max": 150 }, "PlayerController": {} } },
    "x8m2": { "name": "Rifle", "parent": "k3q9", "order": "a0",
      "prefab": { "id": "a_91fd02", "path": "prefabs/rifle.prefab" },
      "overrides": {
        "Transform.position": [0.3, 0, 0.5],
        "m4tz/MeshRenderer.material": { "id": "a_77c1aa", "path": "materials/gold.mat" },
        "Inventory.slots[s_7c].count": 3
      },
      "removed": ["m4tz/Muzzle"]
    }
  }
}
```

Override keys take the form `<entity sid path inside the prefab>/<Component>.<field path>`. Collections that need per-item overrides use keyed items (`slots[s_7c]`), as in Stride's `CollectionItemIdentifiers`. Plain value arrays are replaced wholesale. Nested prefabs chain sids (`x8m2/m4tz/...`), which is the same idea as Godot's `id_path` and Unity's `GlobalObjectId.targetPrefabId`, but readable.

**Why not the alternatives:**
- YAML (Unity, Stride) has ambiguous typing, custom tags and noise.
- Godot's custom format is more readable but invisible to JSON Schema and structured-output tooling.
- Binary (UE) is illegible to agents and to git.
- A code DSL like BSN reads beautifully, but executable scene files are hard for tools to round-trip-edit.

JSON's verbosity is tamed by omitting defaults and by readable keys. Ship a **semantic git merge driver** (`sn merge`), which is trivial because entities are keyed maps.

**Binary data** exists only as cooked or cache artifacts: memory-mappable and versioned from the same reflection. Large worlds can later split into per-cell or per-layer files (OFPA-lite) without a format change, because entity sids are already file-local and stable.

### 3.4 Transactions, undo and the patch op format

There is **one op vocabulary** for everything that mutates the world:

```ts
type Op =
  | { op: "set";    target: EntityRef; path: string; value: Json }      // "Health.max"
  | { op: "add";    target: EntityRef; component: string; value?: Json }
  | { op: "remove"; target: EntityRef; component: string }
  | { op: "spawn";  prefab?: AssetRef; parent?: EntityRef; with?: Record<string, Json>; as?: string }
  | { op: "destroy"; target: EntityRef }
  | { op: "reparent"; target: EntityRef; parent: EntityRef | null; order?: string };

interface Transaction { label: string; actor: `user:${string}` | `agent:${string}`; ops: Op[] }
```

- The engine validates the whole transaction against reflection *before* applying it, applies it atomically, records **automatically computed inverse ops**, and emits change events.
- Undo means applying the inverse. There are no hand-written undo calls. Compare Godot's ~1,334 `add_undo_*` calls in `editor/` and Unity's 265 `Undo.Record*` calls, each one a place where a bug can hide.
- **Because `actor` is recorded**, the editor can show "Claude changed 14 things", and an agent's batch can be undone selectively or as a whole.
- The same ops encode prefab overrides (a `set` on a prefab-instance path), editor→play-process live edits, multi-user collaboration later (The Truth's lesson), and, in a quantized binary encoding with the same addressing, network deltas.

### 3.5 The AI-native layer: the World Protocol

The protocol is JSON-RPC 2.0 over WebSocket (with stdio for the CLI), and **the editor UI uses it too**, so it can never fall behind what humans can do.

| Method | Purpose |
|---|---|
| `schema.list / schema.describe` | Reflection DB slices (tool search, UE 5.8-style), so agents fetch only the schemas they need |
| `world.query` | ECS query (`with`, `without`, `changedSince`, name/path glob) returning sids + selected fields |
| `world.get` | Components of an entity, defaults elided, inherited values marked |
| `world.transact` | A `Transaction` (§3.4); returns an undo handle or structured errors |
| `world.call` | Invoke an `AgentCallable` command |
| `sim.step / sim.play / sim.record` | Deterministic stepping with input injection; replays |
| `view.capture` | Screenshot or render target from a named camera |
| `assets.find / assets.import / assets.validate` | Asset graph queries and import |
| `history.list / history.undo` | Actor-attributed history |

**Errors are data:** `{ code: "field_out_of_range", target: "k3q9", path: "Health.max", legal: { min: 1, max: 10000 } }`, with did-you-mean suggestions for unknown component and field names. An MCP adapter exposes this as a small, fixed set of meta-tools (`describe`, `query`, `transact`, `call`, `capture`, `step`), the opposite of dumping 800 tool schemas into context.

**Offline parity:** `sn` CLI subcommands (`validate`, `schema`, `query <file>`, `import`, `test`, `run --frames N --input rec.json`) work on files with no editor. Agents can also simply edit the JSON files, which works because the files are canonical and schema-checked.

**Agent tests:** with deterministic stepping plus `capture`, agents can verify their own work ("spawn 10 enemies, step 600 frames, assert none fell through the floor").

**Security:** the protocol is localhost-only by default with a per-session token. UE's MCP has no auth at all, which is a mistake not to copy. Commands carry capability tags (for example, filesystem writes outside the project are never agent-callable).

### 3.6 Asset pipeline

- **Importers are pure functions**: `(source bytes, settings, importer version, target platform) → artifacts`. The cache key is a hash of those inputs plus dependency hashes. The cache is content-addressed in `.sn/cache/` (gitignored), with an optional shared cache server later (Zen/Accelerator-style).
- Imports run in **worker child processes**, for parallelism and so that a crashing FBX/glTF importer can't take the editor down (Unity's model). There's no always-on separate daemon (O3DE's AP). The editor and `sn import` drive the same library.
- The asset dependency graph is explicit (material → textures, prefab → meshes) and queryable over the protocol.
- **Hot reload:** `Handle<T>` points at a slot, so a reimport swaps content in place and fires an `AssetChanged` observer. Code reload follows the ECS split: data stays in components, the module is swapped, and only types whose layout changed are re-serialized through reflection.
- Cooking is a projection of the same graph per platform.

### 3.7 Modules

- **Kernel**, kept deliberately boring and small: platform, memory, jobs/scheduler, reflection, ECS world, serialization, assets, transactions, World Protocol.
- **Modules** (render, physics, audio, animation, UI, VFX, navigation, net, editor panels) live in the monorepo, are linked statically, and are enabled per project in `project.json`. A module registers types, systems, commands and editor panels *only* through reflection, so every module is automatically inspectable, serializable and AI-exposed.
- **Packages** (content + scripts + optional native code) use a manifest with semver dependencies, later.
- **Third-party native ABI:** defer it (YAGNI). When needed, copy GDExtension: a C ABI with a JSON-described, schema-validated interface and compatibility hashes.
- **Avoid:** O3DE's gem granularity (79 gems, CMake per gem) and Unity's core-features-as-external-packages version matrix, which Unity is now undoing.

### 3.8 Runtime core and process model

- **Scheduler:**
  - One work-stealing job system with a task graph.
  - Systems declare reads and writes, and the scheduler builds parallel batches (Bevy).
  - The main thread owns the OS and window; render is pipelined one frame behind using an extract step.
  - No fibers in v1. They complicate debugging and TLS, and you can add them later inside the scheduler.
- **Loop:**
  - Fixed-step simulation with variable-rate rendering and interpolation.
  - **Deterministic by construction on a given platform and build**: stable system order, fixed `dt`, per-system seeded RNG streams, recorded input.
  - Cross-platform bit-exact determinism is *not* a goal (it would require fixed-point math or strict FP). Revisit if a rollback-netcode genre demands it.
- **Memory:**
  - Archetype chunks for components.
  - Per-frame arenas.
  - Generational handles, never raw pointers, in the script and protocol API, so a stale handle produces an error rather than undefined behavior.
- **Process model:**

```
            ┌───────────────── libsnengine ─────────────────┐
            │ kernel: platform · memory · jobs · reflection │
            │ ECS world · serialization · assets · txns     │
            │ World Protocol (JSON-RPC)                     │
            │ modules: render · physics · audio · anim · UI │
            └───────────────────────────────────────────────┘
 sn-editor ── edit world + editor UI (a protocol client) ── WebSocket ── agents (MCP adapter) / web tools
    │ spawns                                                   │
    ├─ sn-player (play world, child process) ◄── same protocol: live inspect/patch, step, capture
    └─ import workers (child processes) ──► content-addressed cache
 sn (CLI, headless): validate · schema · import · cook · test · run --frames
 sn-player (shipping): same library, editor modules compiled out
```

Play in a child process (Godot) rather than in-process (UE/Unity), because:
- Crashes, including agent-induced ones, never kill the editor.
- Play always starts from clean state, with no domain-reload or static-state bugs.
- Multi-client network testing comes for free.
- It *forces* the live-inspection protocol to be complete, which is exactly what agents need.

The cost is startup time, which you mitigate with the memory-mapped cache and a no-reload runtime. Godot proves the UX is acceptable, and window embedding hides the seam.

### 3.9 Networking (design-for, don't build yet)

Reserve these now so you don't have to retrofit them:
- Entity net IDs, mapped from sids or runtime handles.
- `Replicate`, `Quantize` and `Interpolate` attributes.
- ECS change ticks.
- A server-authoritative default.

The op addressing model (§3.4) is shared with the netcode, but the wire format is separate: binary, quantized, and prioritized by relevancy. Netcode tests are just two `sn-player` processes driven over the protocol.

---

## 4. Recommended architecture at a glance

| Concern | Choice | Steals from | Avoids |
|---|---|---|---|
| Object model | Entities + hierarchy relationship + reflected data components + behaviors + declared-access systems, on ECS storage | Unity UX, Unity 7 / UE6 / Bevy / flecs convergence, Godot servers | UE actor inheritance, O3DE editor/game split, Godot children-as-composition |
| Identity | Runtime: 64-bit generational handles. Files: short sids per file + asset IDs in-file or in import sidecars | Unity 7 `EntityId`, Godot 4.6 `unique_id`, Stride item IDs | Unity fileIDs/`.meta` everywhere, O3DE random component IDs |
| Reflection | Typed-attribute markers → `snreflect` codegen → `sn::Reflect<T>` + Reflection DB JSON. Dynamic runtime types allowed. Move to P2996/P3394 once MSVC ships it | UHT single source, Godot API dump, UE 5.8 schema-from-reflection, flecs runtime meta | O3DE triple contexts, stringly attributes (Flax/Godot/UE meta) |
| Formats | Canonical strict JSON with `$schema`, defaults omitted, keyed maps, ID+path refs, ID-path overrides | Flax delta-vs-prefab, Godot readability, BSN patches | Unity YAML, O3DE index patches, UE binary sources |
| Mutation | Validated transactions of ops, auto-inverse undo, actor attribution | The Truth, O3DE DOM patches | Hand-written undo (Godot/Unity) |
| AI | World Protocol (also the editor's API) + MCP meta-tools + offline CLI + deterministic stepping | flecs REST, Bevy BRP, Unreal MCP tool search | Bolt-on MCP plugins, incomplete schemas (BRP) |
| Assets | Pure importers in worker processes, content-addressed cache, handle hot-swap | Unity parallel import + ADBv2, UE Zen DDC, Godot `.import` | O3DE always-on AP, UE source-file non-authority |
| Modules | Small kernel + statically linked modules registering via reflection; defer native ABI | Godot modules, GDExtension later, Flax simplicity | O3DE gem sprawl, Unity package version matrix |
| Runtime | Work-stealing task graph, Bevy-style access scheduling, extract-based render pipelining, fixed-step deterministic sim | Bevy, UE Tasks, Unity jobs safety | Two job systems (O3DE), fibers in v1 |
| Processes | Engine-as-library; editor hosts the edit world; play and import in child processes; headless CLI | Godot, LibGodot, Unity import workers | In-process play with domain reloads |

---

## 5. The five decisions that are hardest to change later

1. **The object model and identity scheme.** Entity/component/behavior/system semantics, hierarchy as a relationship, generational runtime handles and file sids. Every script, file, tool and agent prompt bakes this in. Unity has needed about six years (DOTS 2019 to Unity 7) to try to merge two models.
2. **The reflection source of truth and its vocabulary.** The marker syntax, the attribute types, type-identity and rename rules, and the Reflection DB format. Every module and every generated binding depends on it. Typed attributes now make the C++26 migration a tooling change rather than a rewrite.
3. **On-disk formats and addressing.** JSON canonical form, sid-keyed maps, reference shape, and prefab override addressing. Once user projects exist, every change needs migrators forever (Unity still carries `m_` YAML from 2005).
4. **Languages and the hot-reload boundary.** The core language (C++ vs Rust) and the gameplay language (C#, TypeScript, Luau…), plus where the reload boundary sits. This decides bindings, tooling, platform reach and hiring. It's owned by another track, but it constrains §3.2.
5. **The execution and process model.** Declared-access scheduling, the threading/extract split, the determinism guarantees, and editor ↔ play-process ↔ protocol boundaries. Retrofitting determinism or out-of-process play into code that assumes shared mutable globals is close to impossible (see Unity's multi-year removal of domain reload).

---

## 6. Open questions and risks

- **Core language.** Everything here assumes C++, because of console SDKs and the middleware ecosystem (Jolt, Wwise, DLSS…) needed for Unreal-level fidelity.
  - **Rust** would give derive-based reflection today and memory safety, which matters more than usual when agents write engine-adjacent code. Bevy shows the reflection/BRP story works.
  - The risks with Rust are console support, editor-UI ecosystem maturity and compile times.
  - The Reflection DB design is language-neutral, so this decision doesn't invalidate §3.
- **Gameplay language** (C# like Unity/Flax/s&box, TypeScript, Verse-like, or Luau). It decides the binding generator target and the hot-reload story, and belongs in the scripting track. Recommendation from this track: whatever is chosen must register types into the same runtime `TypeRegistry` and consume the Reflection DB.
- **Cost of the codegen tool.** UHT is large. Keep `snreflect` deliberately dumb (it only extracts marked declarations and copies attribute tokens) and let the compiler do the checking. The risk is template-heavy or macro-generated reflected types, so forbid them in reflected regions.
- **JSON at scale.** Loading a 100k-entity world from JSON is slow. The editor should load from a binary cache keyed by file hash, with text staying the source of truth. Benchmark early.
- **Child-process play UX.** Unity users expect instant Play and inspector edits during play. This needs a sub-second boot target and a live-patch round trip under 50 ms. Prototype it early, and keep an in-process "simulate" mode as a fallback option **[design risk]**.
- **Exposing ECS to beginners.** Unity spent years failing to make DOTS approachable. Behaviors-as-components must feel exactly like MonoBehaviour, and archetypes must never appear in beginner-facing UX.
- **Merge conflicts.** Even keyed JSON conflicts on concurrent edits to the same entity. A semantic merge driver is required, and per-entity files (OFPA-lite) may be needed for team scale.
- **Agent safety.** Protocol auth, capability tags, rate limits, and keeping destructive commands (delete asset, overwrite file) behind confirmation. UE's MCP ships unauthenticated on loopback. Don't copy that.
- **Determinism discipline.** Every module must respect ordered execution and seeded RNG, and one non-deterministic system breaks replays. Enforce it with CI replay-hash tests.
- **Version currency.** Some 2026 facts come from secondary sources: Godot 4.7 date, Bevy subsecond upstream status, Unity version numbering (forum posts disagree on 6.5/6.6/6.8 timing), the UE6 statement that Actors/Blueprints will be replaced, and the latest O3DE release. Recheck before committing.

---

## Sources

**Local clones (read-only):**
- `godot/core/object/object.h:245`, `godot/core/object/property_info.h:41-125`, `godot/core/object/undo_redo.h:107-112`, `godot/core/object/worker_thread_pool.h`, `godot/core/extension/{gdextension_interface.json, gdextension_interface.schema.json, extension_api_dump.cpp}`, `godot/scene/resources/resource_format_text.cpp:195-272,1728-1761,2161`, `godot/scene/resources/packed_scene.cpp:1180-1204`, `godot/scene/main/node.h:295,552`, `godot/core/io/resource_loader.cpp:123,1430`, `godot/editor/import/`, `godot/version.py`
- `flax/AGENTS.md`, `flax/Flax.flaxproj`, `flax/Source/Engine/Core/Config.h:50-60`, `flax/Source/Engine/Level/Actor.h:26-190`, `flax/Source/Engine/Level/SceneObject.cpp:95-118`, `flax/Source/Engine/Level/SceneObjectsFactory.cpp`, `flax/Source/Engine/Level/Level.cpp:267,601`, `flax/Source/Engine/Threading/JobSystem.h`, `flax/Source/Tools/Flax.Build/Bindings/`
- `o3de/Code/Framework/AzFramework/AzFramework/Components/TransformComponent.cpp:835-895`, `o3de/Code/Framework/AzToolsFramework/AzToolsFramework/ToolsComponents/EditorComponentBase.h:202`, `o3de/Code/Framework/AzToolsFramework/AzToolsFramework/Prefab/`, `o3de/Assets/Editor/Prefabs/Empty.prefab`, `o3de/AutomatedTesting/Levels/Navigation/NavigationSample/NavigationSample.prefab:955-967`, `o3de/Code/Framework/AzFramework/AzFramework/DocumentPropertyEditor/`, `o3de/Code/Framework/AzCore/AzCore/{DOM,Jobs,Task,Interface}/`, `o3de/Code/Tools/AssetProcessor/`
- `unity-cs-reference/README.md`, `unity-cs-reference/Modules/Entities/Public/EntityId.bindings.cs`, `unity-cs-reference/Modules/Entities/Managed/`, `unity-cs-reference/Editor/Mono/Prefabs/PropertyModification.bindings.cs`, `unity-cs-reference/Editor/Mono/Undo/Undo.bindings.cs`, `unity-cs-reference/Editor/Mono/GlobalObjectId.bindings.cs`

**Web:**
- C++26 reflection: [P3394R4](https://isocpp.org/files/papers/P3394R4.html) · [P3795R2](https://www.open-std.org/jtc1/sc22/wg21/docs/papers/2026/p3795r2) · [P2996R11](https://www.open-std.org/jtc1/sc22/wg21/docs/papers/2025/p2996r11.html) · [cppreference C++26 support](https://en.cppreference.com/cpp/compiler_support/26) · [OloEngineBase #688 toolchain tracker](https://github.com/drsnuggles8/OloEngineBase/issues/688) · [bloomberg/clang-p2996](https://github.com/bloomberg/clang-p2996) · [GCC PR120775](https://gcc.gnu.org/bugzilla/show_bug.cgi?id=120775) · [Whole Tomato on C++26 reflection](https://www.wholetomato.com/blog/cpp26-reflection-large-codebases/) · [Fertig, C++26 reflection](https://isocpp.org/blog/2025/09/cpp26-reflection-at-compile-time-andreas-fertig)
- Unreal: [UE 5.8 announcement](https://www.unrealengine.com/news/unreal-engine-5-8-is-now-available) · [State of Unreal 2026](https://www.unrealengine.com/news/state-of-unreal-2026-top-news-from-the-show) · [Unreal MCP (5.8)](https://dev.epicgames.com/documentation/unreal-engine/unreal-mcp-in-unreal-editor?lang=en-US) · [UHT](https://dev.epicgames.com/documentation/en-us/unreal-engine/unreal-header-tool-for-unreal-engine) · [One File Per Actor](https://dev.epicgames.com/documentation/en-us/unreal-engine/one-file-per-actor-in-unreal-engine) · [DDC / Zen](https://dev.epicgames.com/documentation/en-us/unreal-engine/using-derived-data-cache-in-unreal-engine) · [UEFN Scene Graph](https://dev.epicgames.com/documentation/fortnite/scene-graph-in-unreal-editor-for-fortnite) · [Getting started in Scene Graph](https://dev.epicgames.com/documentation/fortnite/getting-started-in-scene-graph-in-fortnite) · [UE 5.7 release](https://www.unrealengine.com/news/unreal-engine-5-7-is-now-available)
- Unity: [2026 ECS for all](https://discussions.unity.com/t/2026-ecs-for-all/1732447) · [ECS status Dec 2025](https://discussions.unity.com/t/ecs-development-status-december-2025/1699284) · [CoreCLR, scripting & serialization update, June 2026](https://discussions.unity.com/t/coreclr-scripting-and-serialization-update-june-2026/1723299) · [Unity 7.0 alpha thread](https://discussions.unity.com/t/unity-7-0-alpha-is-now-available/1738310) · [Unity MCP open beta](https://unity.com/blog/unity-ai-mcp-how-to-get-started) · [Parallel Import](https://docs.unity3d.com/Manual/ParallelImport.html)
- Godot: [4.6 release](https://godotengine.org/releases/4.6/) · [4.7 release](https://godotengine.org/releases/4.7/) · [4.4 release (separate-process game, embedding)](https://godotengine.org/releases/4.4) · [Game embedding docs](https://docs.godotengine.org/de/4.x/tutorials/editor/game_embedding.html)
- O3DE: [Release notes](https://docs.o3de.org/docs/release-notes/) · [25.10 announcement](https://o3de.org/o3de-release-25-10-now-available/)
- Flax: [Flax blog (1.12)](https://flaxengine.com/blog/tag/flax/) · [Flax 1.11 forum](https://forum.flaxengine.com/t/flax-engine-update-1-11/2330)
- Bevy: [0.19 (BSN)](https://bevy.org/news/bevy-0-19/) · [0.20](https://bevy.org/news/bevy-0-20/) · [bevy_brp_mcp](https://docs.rs/crate/bevy_brp_mcp/0.22.3) · [bevy_hotpatching_experiments](https://docs.rs/crate/bevy_hotpatching_experiments/latest)
- flecs: [Remote API](https://www.flecs.dev/flecs/FlecsRemoteApi.html) · [Prefabs manual](https://www.flecs.dev/flecs/PrefabsManual.html) · [releases](https://github.com/SanderMertens/flecs/releases)
- Others: [s&box docs](https://sbox.game/dev/doc/getting-started/) · [s&box open-sourced (GFS)](https://gamefromscratch.com/sbox-game-engine-open-sourced/) · [Fyrox property inheritance](https://47x5b-baaaa-aaaak-ao3zq-cai.icp0.io/scene/inheritance.html) · [Stride asset introspection](https://doc.stride3d.net/latest/en/manual/engine/asset-introspection.html) · [Stride archetypes/prefabs](https://www.stride3d.net/blog/feature-spotlight-archetypes-prefabs-and-nested-prefabs/) · [The Truth (Our Machinery)](https://gamedeveloper.com/programming/the-story-behind-the-truth-designing-a-data-model) · [Roblox Studio MCP listing](https://glama.ai/mcp/servers/kwqaglxiux)
