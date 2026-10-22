# R04 — Editor UX, editor technology, and graph tools

Research date: **2026-10-22**. Product judgment is explicitly distinguished from observed mechanisms. All four clone completion markers existed before inspection; clones were read only. No engines were built, run, or benchmarked. Source heads are development snapshots, not evidence that a feature shipped in a particular stable release. Pillars below: **P1** AI-native; **P2** high fidelity; **P3** Unity-like ease; **P4** excellent editor UI; **P5** strong human/AI authoring tools.

## TL;DR

- **Build a React/TypeScript editor shell around a native engine host.** Start with Electron for a controlled Chromium environment; validate native GPU viewport presentation before committing. Tauri is a credible alternative, not a free reduction in integration complexity.
- Unity's strongest idea is a **shared serialized property model**: fields become inspectors, custom drawers, multi-editing, prefab overrides, and undo. Expose that model to agents too.
- Make **typed commands the only authoring mutation path**. UI, gizmos, CLI/MCP, plugins, and graph edits use the same validation, transactions, revisions, and diagnostics.
- Use **semantic commands plus automatically captured reversible document changes**. Pure handwritten undo commands invite omissions; full-scene snapshots waste memory and produce poor review diffs.
- **Share graph infrastructure, not one execution language.** Materials, VFX, gameplay, animation, behavior, and dialogue need different scheduling and correctness rules.
- Start graph interaction with **React Flow behind an adapter**, keep a native/headless graph model and compiler, and store stable IDs, named ports, and readable versioned text independently of the UI library.
- Steal Niagara's **stack-first progressive disclosure**, Sequencer's shared timeline/curves, Godot's coherent scene composition, and Flax's reusable Visject surface. Avoid making every task a node graph.
- Treat play mode as a **disposable runtime world with explicit “keep these changes” operations**. Avoid hidden persistence and “disable reset to go fast” correctness traps.
- Ship inspection and diagnosis with authoring: searchable assets, import status, console, basic profiler/frame inspection, command history, and runtime UI preview. Defer general multiplayer and a comprehensive visual scripting language.

## How each reference engine does it

### Unity: easy because the same concepts repeat everywhere

**Inspector mechanics.** Unity does not require an editor form for every component. Its [serialization rules](https://docs.unity3d.com/6000.0/Documentation/Manual/script-serialization-rules.html) select supported public fields and fields marked for serialization; presentation attributes add constraints and hints. The inspected default IMGUI implementation calls `UpdateIfRequiredOrScript()`, obtains a serialized-property iterator, visits visible properties, draws `PropertyField`, then calls `ApplyModifiedProperties()`. The UI Toolkit implementation similarly generates `PropertyField` elements from `SerializedProperty` entries, giving them names derived from `propertyPath`. This is concrete evidence for generating UI from authoring data, rather than maintaining parallel handwritten forms. [U1] [U2]

`SerializedObject`/`SerializedProperty` provide the intermediary editing representation, typed value access, paths, traversal, and mixed-value information (`hasMultipleDifferentValues`). `[Range]`, `[Tooltip]`, and `[Header]` are metadata; `PropertyDrawer.OnGUI` and `CreatePropertyGUI` are escape hatches for specialized widgets. A developer can add a gameplay field and immediately manipulate it, while tools retain consistent selection and property behavior. Directly assigning arbitrary runtime fields bypasses parts of this machinery; an equivalent snengine inspector should never need its own persistence implementation. [U3] [U4]

**Play, tweak, repeat.** The Scene view remains a spatial authoring surface while the Game view runs simulation. Pause/step and live inspection shorten the hypothesis-to-feedback loop. Runtime scene changes generally disappear when leaving play; asset edits can persist, so “everything resets” is an unsafe mental model. Unity's configurable play options skip domain and/or scene reload. Disabling domain reload also preserves static variables and event subscriptions, requiring explicit reset logic; it does not eliminate script recompilation/assembly reload. The source exposes separate `DisableDomainReload` and `DisableSceneReload` flags. The lesson is to make simulation-state ownership obvious, rather than offering speed through subtly different initialization semantics. [U5] [U6]

**Prefabs.** Reusable hierarchies, nested prefab instances, variants, and property-level overrides turn repeated assembly into controlled reuse. A variant inherits a base and adds overrides; nesting supports composition. Apply/revert UI makes the relationship visible. Source functions include `GetPropertyModifications`, `ApplyPropertyOverride`, `RevertPropertyOverride`, and prefab-instance recording. This is powerful because the inspector can show where a value comes from, but nested ownership makes “apply” increasingly consequential. Copy the affordances and provenance; resist unrestricted inheritance depth and ambiguous apply targets. [U7]

**Spatial tools and asset discovery.** Move/rotate/scale handles, local/world and pivot/center modes, grid increments, vertex snapping, and custom scene gizmos let developers author behavior in its spatial context. The Project window combines folder navigation, search, thumbnails, and asset selection in the same inspector. GUID-backed references permit assets to be moved without rewriting every consumer. Package Manager centralizes optional capabilities and versions; package, import, and compilation waits still interrupt the otherwise immediate loop. Source anchors include `Editor/Mono/Handles/Handles.cs`, `Editor/Mono/ProjectBrowser/ProjectBrowser.cs`, and `Modules/PackageManagerUI/Editor/` under the Unity clone.

**Unity 6 specifically.** The 6.0 change log documents Build Profiles, selection of overlapping objects through the piercing menu, and UI Toolkit/UI Builder improvements. It also lists changes inherited from earlier releases, so those should not all be described as new 6.0 inventions. More consequential for this research: the old experimental Graph Toolkit package is deprecated **from Unity 6.4**, when Graph Toolkit becomes an Editor module; current 6.6 documentation describes a graph front end. “Unity still only has an abandoned experimental GraphView” is stale. Shader Graph, VFX Graph, and Graph Toolkit nevertheless should not be assumed to share every backend or asset format. [U8] [U9]

The specialized tools illustrate why: **Shader Graph** ends in a Master Stack with vertex/fragment contexts and compatible output blocks. **VFX Graph** combines vertical processing stages with horizontal property dataflow; contexts contain ordered blocks, with operators feeding their parameters. **Graph Toolkit** supplies reusable front-end interaction rather than either domain's simulation/compiler. Shared search, wiring, selection and diagnostics are sensible; forcing both shader stages and particle lifecycle into a generic undifferentiated graph would remove useful constraints. [U11] [U12]

**Cost.** Unity has both IMGUI and UI Toolkit extension paths, serialization-specific limitations, domain reload friction, and prefab complexity. Its ease comes from useful defaults and consistent workflows, not absence of machinery. The local reference is **Unity 7000.0.0a12**, so this report uses Unity 6 documentation for release-specific claims and the clone for inspected implementation patterns.

### Unreal: excellent specialist tools, substantial conceptual surface area

The **Details panel** turns reflected properties into editing controls. `IDetailCustomization` customizes an object's layout; `IPropertyTypeCustomization` customizes a property type; `IPropertyHandle` connects controls to properties. Categories, conditional visibility, metadata, and bespoke Slate widgets support production tooling. This is the same valuable default-plus-escape-hatch pattern as Unity, with more C++ infrastructure to learn. [E1]

**Blueprints** combine component assembly, exposed parameters, event/control-flow graphs, debugging, and reuse. **Materials** provide typed dataflow and immediate preview, with stats and palette alongside the graph. **Niagara** offers systems, emitters, ordered module stacks, curves, parameter namespaces, and lower-level graphs. Its generated-code and attribute views make complex behavior inspectable. Crucially, many effect adjustments are stack edits rather than wiring operations. **Sequencer** supplies tracks, keyframes, curves, scrubbing, camera cuts, and auto-key integration with property editing. These are complementary representations, not evidence that every editor should become one graph canvas. [E2] [E3] [E11]

Unreal's reusable graph architecture separates `UEdGraph`/nodes/pins from schema-defined connection and action rules and Slate graph presentation. Current schema inheritance includes material, Niagara, K2, animation state machine, and behavior-tree specializations. Share this separation, not Unreal's entire class hierarchy. Blueprint compilation has a backend pipeline; Epic's detailed compiler overview found here is explicitly **4.27**, so it is historical evidence for graph-to-statements/VM design, not a verified current promise of C++ generation or nativization. [E4] [E5]

**World Partition** manages editor/runtime world cells and distance-based loading. **One File Per Actor** externalizes actor-instance authoring data to reduce source-control overlap; it also works without partitioning and is an editor storage mechanism, with actors embedded during cooking. OFPA does not by itself provide simultaneous editing, merge resolution, or readable files. Its encoded filenames and cross-file consistency requirements illustrate the cost of granular storage. [E6] [E7]

**Live Coding** patches C++ binaries using Live++. It supports editor and play sessions, but reinstancing can invalidate pointers/caches, constructor-default changes have limitations, and preload choices trade iteration speed against startup. “Hot reload” is a family of constrained operations, not a guarantee that arbitrary structural edits are safe. [E8]

**Where it feels heavy — assessment, not measured ranking:** multiple specialist asset editors, reflected/native/generated concepts, compile and shader jobs, asset references, world-loading state, and package/source-control state all compete for attention. Unreal earns much of this complexity through capability. snengine should expose the current task's controls first and retain expert views behind explicit expansion.

### Godot: a coherent editor built from its own product

Godot's C++ editor uses the engine's own hardware-accelerated **Control** UI toolkit, not Qt. Inspected classes include `EditorNode : Node`, `EditorInspector : ScrollContainer`, and `EditorProperty : Container`. The scene tree, inspector, resource system, and UI authoring therefore share underlying concepts. Scenes are reusable node trees, supporting composition and instancing instead of a separate prefab ontology. [G1] [G2]

Its compact native distribution and minimal setup are valuable UX properties; the official FAQ explicitly cites small distribution size. Exact current binary size and comparative startup latency were **not measured** here. Fast perceived iteration is an architectural target, not something proved merely by choosing a native toolkit. Large imports and `@tool` work can still dominate responsiveness.

`EditorPlugin` exposes node/resource editors, docks, menus, inspector plugins, viewport input/drawing, and import/export integrations. `EditorUndoRedoManager` routes actions to per-scene histories or a global history. Plugins declare do/undo methods or properties and commit a named action; context inference can be wrong, so the API supports an explicit context. This is admirably approachable, but method-name callbacks are not a durable, typed, process-independent command protocol. [G3] [G4]

`GraphEdit`/`GraphNode` provide graph interaction. Connection/delete attempts emit requests; the consumer owns the semantic changes. VisualShader is a specialized consumer, not a universal gameplay language. The clone locates it under `modules/visual_shader/editor/`. This clean division is worth copying. [G5]

**Tradeoff:** dogfooding improves consistency and runtime UI investment, but building a desktop toolkit also means owning text input, accessibility, focus, docking, platform behavior, and automation. A new engine should not assume Godot's mature UI capability arrives automatically with its renderer.

### Flax: a particularly relevant Unity-to-native bridge

Flax offers an integrated editor with dockable windows, hierarchy/properties/content workflows, in-editor play, and C# extensibility. Its serialization documentation says JSON state is used for scene loading, hot reload, and undo; supported public/annotated members are discovered automatically. This is close to Islam's existing workflow. [F1] [F2]

Source shows a concrete reload boundary: `Scripting.cpp` raises reload events and unloads/recreates the scripting `AssemblyLoadContext` before completing reload. Consequently, describe C# hot reload as **state restoration and module lifecycle management**, not free in-place replacement of arbitrary objects. No comparative reload-time benchmark was performed. [F3]

**Visject** is the best compact reference for reuse: `MaterialSurface`, `ParticleEmitterSurface`, `AnimGraphSurface`, `BehaviorTreeSurface`, and `VisualScriptSurface` derive from `VisjectSurface`. It shares node interaction, archetypes, contexts, and edit actions while specializing domains. However, inspected surface serialization uses binary readers/writers. Steal its division of responsibilities; choose a different authoring format for AI and version-control readability. `IUndoAction` exposes `Do`/`Undo`; object edit actions also use captured member differences and JSON storage, demonstrating a hybrid rather than one pure undo strategy. [F4]

### O3DE: strong reusable infrastructure, visible workflow friction

The main editor derives from Qt `QMainWindow`; GraphCanvas uses `QGraphicsView`. Script Canvas builds visual scripting atop reusable graph tooling, while Atom, animation, terrain, and UI tools expose substantial specialist capability. Source includes a Script Canvas `GraphToLua` backend, a useful real example of visual programs lowering through a code model to textual code. [O1] [O2]

The **Asset Processor** is a separate background service: scan sources, schedule builders, maintain dependencies/cache, report jobs, and notify editors/runtimes when products update. It prioritizes critical assets before the editor opens, then continues processing others; referenced assets can be escalated. This is sound architecture, but users must understand why an asset is unavailable and what work remains. Put that explanation in the asset browser instead of making users discover another application. [O3]

**Prefabs** form nested reusable structures; the currently edited prefab owns overrides to contained instances. Edit scope determines whether a change affects an instance or all instances. The source's `URSequencePoint` supports hierarchical undo/redo commands; prefab link undo stores undo/redo DOM patches. These are directly relevant to grouping agent operations and prefab deltas. [O4] [O5]

**What users criticize, with evidence boundaries:** a May 2025 contributor RFC reports constrained UI component selection, difficult animation/navigation workflows, inconsistent saving/undo, and editor performance problems. That RFC is now closed and links subsequent work; it is evidence of reported pain, **not proof every defect remains in October 2026** or a representative user survey. The transferable lesson is that rich systems lose value when related tools disagree about save state, undo, shortcuts, or ownership. [O6]

## State of the art beyond these engines

### UI technology: choose the total authoring cost

- **Qt Widgets/Qt Quick:** mature desktop primitives, focus/text/accessibility infrastructure, native integration, and docking (`QDockWidget`). Qt is not limited to OpenGL: `QRhiWidget`, available since 6.7, supports modern graphics backends; QRhi internals have limited compatibility guarantees. O3DE is the concrete engine-editor example. Best when native integration dominates and the team wants C++/QML; less aligned with Islam's React expertise. Styling and custom editor interaction still require product work. [T1]
- **Dear ImGui:** exceptionally direct C++ tools, easy engine texture display, debug overlays, docking/multiple viewports via its docking branch, and low dependency burden; Lumix is an editor example. The project explicitly says full internationalization and accessibility are unsupported. Excellent for internal diagnostics, weaker as snengine's primary long-lived, human-friendly and agent-inspectable shell. A canvas needs an added semantic automation layer. [T2]
- **Custom retained UI in-engine:** Godot Controls and Unreal Slate show the ceiling: complete visual control, GPU integration, reusable widgets, and consistent styling. The cost is becoming a toolkit maintainer before becoming a great engine. Slate and UMG are related layers, not evidence of one identical editor/game UI surface. Use this approach only with a strong strategic reason beyond avoiding Chromium. [G1] [T3]
- **egui:** Rust immediate UI with native/web integrations, useful reflection tooling such as `bevy-inspector-egui`, and optional AccessKit accessibility. It should not inherit blanket claims that all immediate UI lacks accessibility. It fits a Rust-heavy tools team, but sophisticated authoring widgets and agent semantics remain integration work; it does not reuse React/DOM skills. [T4]
- **Slint:** declarative native UI with Rust/C++ and JavaScript/TypeScript bindings, custom rendering and a smaller-runtime focus. It is not React, and native viewport/docking fit requires a prototype. Licensing is multi-option; the verified desktop-readiness roadmap also demonstrates why “small native toolkit” should not imply complete professional-editor ergonomics. [T5]
- **Zed GPUI:** a real Rust/GPU editor framework with a responsive desktop ambition. Zed's account of building its settings editor describes additional focus/navigation and form-state work. It proves the approach is viable and investment-heavy; it is not an inexpensive web-shell replacement for this owner. [T6]
- **Electron:** React/TS, Chromium devtools, established DOM automation/accessibility, theming, and a controlled browser version. Costs include distribution/RAM, update maintenance, IPC boundaries, and GPU compositing. Its experimental `sharedTexture` API imports native texture handles and transfers frames to renderers. That is the relevant **native-to-web** direction; offscreen rendering's **web-to-native** export is a different path. Neither alone proves snengine's full integration. [T7]
- **Tauri/system webviews:** preserves React/TS while avoiding bundling a complete browser. Windows uses WebView2, macOS WKWebView, Linux WebKitGTK. The resulting platform differences and native GPU-surface bridges must be owned; smaller download size does not establish lower viewport latency. Strong alternative if the prototype demonstrates comparable integration. [T8]
- **CEF/native-owned shell:** embeds Chromium into a native application and supports offscreen web content. Attractive if the native compositor must own the whole window; the team then owns more browser/window/input integration than Electron provides. **Browser UI plus remote native engine** gives excellent sharing and automation but requires a frame transport; compressed streaming adds encode/decode delay and visual tradeoffs. Local native fidelity does not imply browser-native renderer parity. [T9]

These tradeoffs are engineering assessments, not benchmark results. React reconciliation is not required on the render-frame path. The dangerous design is serializing frame pixels or whole scene trees through ordinary JSON IPC, regardless of shell choice.

### Product examples: borrow workflows, verify architecture claims

**PlayCanvas** proves browser-based scene editing and visible collaborative presence. Its current documentation also exposes Editor MCP and a VS Code synchronization workflow; notably, it recommends explicit pull/push for external AI assistants because realtime sync can ignore external changes to closed files. snengine needs a deliberate external-edit protocol, not an assumption that file watching equals collaboration. [W1]

**Babylon.js Editor 5** is a concrete Electron editor; the repository describes React renderer and TypeScript tooling, and exposes an MCP package. **Needle** is a different example: it integrates existing Unity/Blender authoring with web projects, rather than proving that it has a standalone web-based replacement for those editors. [W2] [W3]

**Figma** combines browser UI with a specialized canvas/WASM core, and its documented multiplayer architecture uses an authoritative server ordering property changes. It is not “everything is ordinary DOM” or an off-the-shelf CRDT everywhere. **Rive** is compelling for the design → animation → state-machine → production-runtime loop; its current editor frontend implementation was not independently verified here. **Spline** demonstrates shared 2D/3D interaction and collaboration; its October 19 update specifically reports moving layout/constraints into a Rust runtime as the single source of truth. That last point directly supports sharing layout semantics between authoring and shipped UI. [W4] [W5] [W6]

**Bevy's current editor effort** is further along than an empty roadmap: its August 2026 account lists scene composition, UI/widgets, a remote protocol, dynamic-plugin work, and a functional prototype. It is still an evolving editor effort, not a proven drop-in Unity replacement. Reuse its separation of reflection, scene format, remote inspection, and UI; do not confuse community egui inspectors with the eventual official editor architecture. [W7]

### Web graph libraries

**React Flow/xyflow** is the pragmatic first choice: custom React nodes/ports, selection, pan/zoom, groups, and keyboard/screen-reader support. Its performance guidance warns about expensive nodes, broad store subscriptions, and rerenders during movement. It is an interaction library, not a material compiler or authoritative graph database. **Rete.js** offers modular renderers, dataflow/control-flow processing and plugins; however, its current Codegen guide literally says “Under development.” Do not make its advertised code generation a delivery dependency. **LiteGraph** supplies Canvas2D interaction, execution concepts and JSON serialization; it reduces DOM load but requires more work for semantic accessibility, rich text, and a strongly typed domain API. None removes the need for domain compilers and diagnostics. [N1] [N2] [N3]

## Recommendations for snengine

### 1. Copy Unity's property contract and make provenance visible

Define one versioned schema for components, assets, properties, defaults, ranges/units, object references, enums, documentation, and editability. Generate native/TS bindings and runtime validators from it. Reflection can populate schemas, but stable property IDs and migrations must survive member renaming. Inspector widgets, agent discovery, animation tracks, prefab overrides, and serialization all address the same properties.

Ship default inspectors immediately; custom drawers register by schema/type and emit standard commands. Support multi-edit/mixed values, reset, search, read-only explanations, and an origin indicator: default, prefab, variant, instance, animation, or runtime. Provide “why is this disabled?” and “show referencing assets.” Never make custom widgets responsible for dirty flags or persistence. **Serves P1/P3/P4/P5**, and makes P2's sophisticated settings manageable.

### 2. Make transactions the product's integration point

The reference systems are hybrids. Unity exposes object recording, complete-object snapshots, and grouped operations. Godot registers explicit forward/inverse operations. Unreal `FTransaction` stores serialized object changes; its API explicitly warns that its traditional undo buffer is not persistable because it depends on object/array layout. O3DE combines command trees with prefab DOM patches. Therefore, **command versus diff is a false binary**. [U10] [G4] [E9] [O5]

Use this contract:

1. A serializable semantic command identifies stable targets and expected revisions, for example `SetProperty`, `InstantiatePrefab`, `ConnectPorts`, or `ApplyPrefabOverride`.
2. The authoritative document host validates it, resolves references, and computes the affected documents and normalized changes. Optional dry-run returns a concrete diff and diagnostics.
3. Commit atomically captures forward/inverse document deltas plus necessary deleted data. Native GPU objects and simulation memory are derived state, never serialized into undo.
4. A transaction has an ID, author, label, base/result revisions, and idempotency key. Persist a small recovery journal before acknowledging durable edits; checkpoint documents periodically. Do not turn this into perpetual event sourcing.
5. A slider/gizmo gesture streams provisional updates and commits one undo entry; Escape restores the initial state. Agent batches similarly commit one meaningful operation. Terrain strokes store touched-tile deltas, not a full terrain snapshot per sample.

Keep view selection/navigation history separate from content undo. Begin with a workspace chronological content history that groups cross-document edits atomically; show its target documents explicitly. For concurrent human/AI edits, reject stale writes or acquire a short edit lease. Undo is a new validated inverse against current state, not a blind rewind that erases another author's work. External file edits enter through staged parse/validate/diff/reconcile; never silently replace dirty documents.

This costs more than a local UI undo stack but establishes **P1's reliable agent interface**, **P3's predictable recovery**, and **P4/P5's consistent tools**. General multiplayer is deferred; two clients, a human and an agent, already require revision semantics.

### 3. Keep one graph document model, with domain-specific rules

The common layer owns stable node/edge/port identity, coordinates, selection, wiring, reroutes, comments, groups, subgraph references, search, copy/paste, diagnostics, and transactions. A node registry defines typed ports, defaults, documentation and versions. Domain adapters define compatibility, cycle rules, execution, preview, and compilation. A material DAG, VFX stage graph, control-flow program, animation state machine, ordered behavior tree, and dialogue graph are not interchangeable merely because all contain boxes and lines.

Store canonical, pretty-printed JSON with deterministic object ordering, stable opaque IDs plus human labels, explicit port names, schema/node versions, and project-relative asset IDs. Keep editor layout metadata in a distinct section excluded from semantic compilation hashes; do not let React Flow's transient selection/measured sizes become the asset format. Arrays remain where order has meaning. Unknown nodes retain their payload and fail with actionable diagnostics rather than disappearing.

Illustrative shape, not a finalized schema:

```json
{
  "schema": "sn.graph/1",
  "domain": "material",
  "nodes": {
    "n_8f3a": { "type": "math.multiply", "version": 1, "label": "Roughness scale", "defaults": { "a": 0.8, "b": 0.5 } },
    "n_9b20": { "type": "material.output", "version": 1 }
  },
  "edges": {
    "e_a701": { "from": ["n_8f3a", "result"], "to": ["n_9b20", "roughness"] }
  },
  "editor": { "positions": { "n_8f3a": [0, 0], "n_9b20": [300, 0] } }
}
```

Agents may patch this text offline; connected agents should use typed graph commands and receive the same compiler errors as humans. Validate port types, required inputs, cycles, versions, references, and domain invariants headlessly. Code generation lowers graph → typed intermediate representation → target compiler: shader code for materials/VFX and the selected gameplay language for supported gameplay graphs. Preserve node-to-generated-line mappings and expose generated code as read-only. Support custom code functions as nodes. Arbitrary bidirectional graph/code round-tripping is explicitly out of scope; importing every possible textual program would force the graph to become a full language IDE.

Implement the first real domain, **materials**, before abstracting six editors. Add a second domain to prove the shared boundary. Favor stacks and property lists for common material/VFX cases, graphs for composition, timelines for time, and ordinary code for dense algorithms. This serves **all five pillars** without making “generic graph framework” the largest v1 project.

### 4. Preserve native rendering without making the UI native-only

Run React/TS with Vite, Tailwind tokens and a narrow Zustand view-state store; keep authoritative project state in the native document host. Use subscriptions to selected objects and visible asset ranges rather than mirroring the entire engine. Dockview is a reasonable existing docking implementation; its core packages are MIT and enterprise additions are separately licensed. Persist layouts by stable panel IDs and support a reset layout command. [T10]

Use a versioned local protocol with generated TS/native types, runtime validation, request IDs, subscriptions, cancellation and structured errors. JSON is adequate for control messages initially; binary buffers/shared handles carry large data. The same host should run headlessly for agent commands and validation. UI accessibility names and stable locators enable interaction testing, but an agent should not need to click menus to modify an asset.

On Windows, prototype native render target → shared GPU texture → Electron presentation, with explicit handle ownership, synchronization, frame IDs, resize generations and device-loss recovery. Render gizmos, picking and scene overlays in the native viewport; send input directly to that controller without waiting for React rerenders. Do not call the path zero-copy until measured. A separate native viewport window is a useful fallback, but its focus/docking UX is weaker. Avoid native-child-window embedding as the default because clipping, overlays and popouts become platform-specific work.

Set **proposed acceptance targets**, not claims: less than one 60-Hz frame of added presentation latency versus the native baseline, no unbounded frame queues, smooth 1440p interaction, a responsive shell during import/compile, and correct DPI/multi-monitor/input capture. Measure p50/p95 latency, CPU/GPU cost, memory, resize behavior, and crash recovery on integrated and discrete GPUs. If integration fails, retain the protocol and compare Tauri/native composition or Qt; do not sink months into a brittle bridge. **P2 stays native; P1/P3/P4/P5 exploit the owner's strongest UI stack.**

### 5. Make play mode and game UI authoring trustworthy

Keep authoring documents distinct from edit-world projections and a disposable play-world process. Live tweaks target either runtime state or authoring state explicitly. Provide selective “Keep changes” with a preview, including create/delete and prefab consequences. Reinitialize the play session deterministically; promise safe data reload first, then bounded script reload, with restart as an explicit fallback for incompatible changes.

For game UI, build a structured document of elements, layout constraints, style tokens, reusable components, bindings, states, and animation tracks. The runtime layout/behavior evaluator must also drive the authoring preview. React renders the editor chrome; choosing it does **not** require shipping Chromium or React inside games. Start with containers, text, images, buttons, clipping, anchors/flex-like layout, input focus and navigation. Include device sizes, localization/RTL, font fallback and keyboard/gamepad preview. AI edits the same UI tree and bindings humans manipulate. This directly addresses **P5**, while shared evaluation avoids two subtly different layout engines.

### 6. Invest in the tools that shorten the feedback loop

Asset browsing needs cached thumbnails, type/tag/name search, saved filters, dependencies/referencers, drag-to-place and visible import failures/progress. Compile previews asynchronously and retain the last good result. Console entries need object/asset/node links, structured diagnostics, filtering and command access. The command palette discovers the same typed operations used by menus and agents.

Provide a basic CPU/GPU timeline, allocation/asset residency view, and frame pass/resource inspection early; integrate an external capture tool before recreating a complete GPU debugger. Animation should reuse property addressing, curves, keyframes and transport controls across object motion, game UI and VFX. Level design needs reliable grids, local/world transforms, surface/vertex snapping and prefab painting before elaborate procedural tooling. Terrain/foliage should use non-destructive layers, seeded scatter and stroke transactions when introduced.

For collaboration, first support stable IDs, mergeable documents, revision checks and scoped edits. Unreal Multi-User uses immediate level transactions, saved-asset synchronization and locks; Figma's published design has different conflict semantics. Neither is a drop-in recipe for scene graphs, terrain and binaries together. Add presence and shared sessions only when a concrete team workflow justifies the persistence and undo semantics. [E10] [W4]

## Open questions / risks

- **Viewport interoperability is the largest early technical risk.** Electron's relevant API is experimental. D3D12/Vulkan interoperability, HDR/color management, multiple adapters and platform parity remain unverified for snengine.
- **A web shell can still become slow.** Asset lists and graphs need virtualization, fine-grained subscriptions and bounded preview work. Toolkit choice alone proves nothing about snengine performance.
- **Document identity, prefab overrides and schema migration are coupled.** Resolve rename/reparent/delete semantics before accumulating real projects; property array indices cannot serve as durable identity.
- **Runtime language is undecided in this track.** It determines graph code-generation targets and reload boundaries. Generating TypeScript is appropriate only if gameplay actually runs the supported TS-derived execution model.
- **Undo durability is not multiplayer undo.** Recovery replay, offline file merges and concurrent inverse operations need separate, bounded designs and tests.
- **UI parity requires one evaluator.** Sharing schemas without sharing layout, text measurement and binding semantics will still produce inaccurate previews.
- **Evidence limits:** no comparative startup, memory, reload or graph-scale benchmarks; no independent verification of Rive's current editor implementation; historical O3DE reports may have been fixed. Reference code demonstrates mechanisms, not freedom to copy implementation; Unity's clone is explicitly reference-only.

## Sources

All web sources accessed on 2026-10-22. Local paths are relative to this report. Unity clone: `bf6028387a4fc2ea6d41aa7d8998e774655aedf6` (7000.0.0a12); Godot: `9c2e39242e20846f84d990fe457a5e146b77fa36`; Flax: `394658f3cba5002c6cc16f054b1443a1b6022f05`; O3DE: `2462b90ee389939336931a14e234fc4e3b4cd534`.

- [U1] — Unity default inspector, `DoDrawDefaultInspector` around line 984.
- [U2] — UI Toolkit `InspectorElement`, `FillDefaultInspector` around line 633.
- [U3] — `SerializedProperty`, including mixed values and paths; also sibling `SerializedObject.bindings.cs` for update/apply.
- [U4] — `PropertyDrawer`; attributes in `Runtime/Export/PropertyDrawer/PropertyAttribute.cs`.
- [U5] — Unity 6 domain reload behavior and reset requirements.
- [U6] — Source `EnterPlayModeOptions` flags.
- [U7] — Unity 6 prefab documentation; inspected apply/revert implementation in `Editor/Mono/Prefabs/PrefabUtility.cs` and `.bindings.cs`.
- [U8] — Unity 6.0 release changes, with inherited-version sections.
- [U9] — Deprecated package notice; [current Graph Toolkit manual](https://docs.unity3d.com/6000.6/Documentation/Manual/gtk/gtk-index.html). Clone framework under `Modules/GraphToolkitEditor/`.
- [U10] — Unity Undo bindings: recording, complete snapshots and grouping. [RecordObject API](https://docs.unity3d.com/6000.0/Documentation/ScriptReference/Undo.RecordObject.html).
- [U11] — Shader Graph 17 Master Stack and shader-stage contexts.
- [U12] — VFX Graph 17 processing/property workflows, contexts, blocks and operators.
- [E1] — Details customization interfaces and property handles.
- [E2] — Niagara panels, stacks, generated code and diagnostics.
- [E3] — Sequencer tracks, curves, auto-key and scrubbing.
- [E4] — Current graph schema and domain-specific subclasses.
- [E5] — Historical Blueprint compiler overview, explicitly version 4.27.
- [E6] — World Partition.
- [E7] — OFPA storage and source-control constraints.
- [E8] — Live Coding and reinstancing limitations.
- [E9] — `FTransaction`, including undo persistence limitations.
- [E10] — Multi-User synchronization, conflict handling and per-user undo.
- [E11] — Material Editor preview, graph, palette and statistics.
- [G1] — Godot UI toolkit FAQ; [editor root source](../.refs/godot/editor/editor_node.h).
- [G2] — Nodes/scenes composition; [inspector source](../.refs/godot/editor/inspector/editor_inspector.h).
- [G3] — EditorPlugin API; [source](../.refs/godot/editor/plugins/editor_plugin.h).
- [G4] — EditorUndoRedoManager; [implementation](../.refs/godot/editor/editor_undo_redo_manager.cpp).
- [G5] — GraphEdit interaction semantics; [VisualShader implementation](../.refs/godot/modules/visual_shader/editor/visual_shader_editor_plugin.cpp).
- [F1] — Flax editor tools and docking.
- [F2] — Flax serialization rules and use in undo/reload.
- [F3] — Native scripting reload sequence, around lines 736–801.
- [F4] — Visject surface directory, including domain subclasses, binary serialization and undo; [general undo source](../.refs/flax/Source/Editor/Undo/).
- [O1] — Qt main window; [GraphCanvas view](../.refs/o3de/Gems/GraphCanvas/Code/StaticLib/GraphCanvas/Widgets/GraphCanvasGraphicsView/GraphCanvasGraphicsView.h).
- [O2] — Script Canvas graph-to-Lua backend.
- [O3] — Asset Processor job behavior; [pipeline overview](https://www.docs.o3de.org/docs/user-guide/assets/asset-processor/).
- [O4] — Prefab edit scope and overrides.
- [O5] — Undo command tree; [prefab link patch implementation](../.refs/o3de/Code/Framework/AzToolsFramework/AzToolsFramework/Prefab/Undo/PrefabUndoUpdateLink.cpp).
- [O6] — Contributor UI modernization RFC, opened May 2025, now closed.
- [T1] — Qt docking; [QRhiWidget documentation](https://doc.qt.io/qt-6/qrhiwidget.html).
- [T2] — Dear ImGui capabilities/limitations; [docking branch](https://github.com/ocornut/imgui/wiki/Docking); [Lumix listed among users](https://github.com/ocornut/imgui/wiki/Software-using-dear-imgui).
- [T3] — Slate historical design rationale (4.27); current Slate use confirmed by E1.
- [T4] — egui features/AccessKit; [Bevy inspector example](https://github.com/jakobhellermann/bevy-inspector-egui).
- [T5] — Slint capabilities; [desktop-readiness discussion](https://www.slint.dev/blog/making-slint-desktop-ready).
- [T6] — GPUI and Zed's form/focus investment.
- [T7] — Electron shared-texture import; [offscreen export direction](https://www.electronjs.org/docs/latest/tutorial/offscreen-rendering).
- [T8] — Tauri process model and operating-system webviews.
- [T9] — CEF embedding architecture.
- [T10] — Dockview docking, layout persistence and license split.
- [W1] — PlayCanvas external-assistant workflow; [collaboration](https://developer.playcanvas.com/user-manual/editor/realtime-collaboration/); [Editor MCP entry](https://developer.playcanvas.com/en/).
- [W2] — Babylon.js Editor; [repository architecture description](https://github.com/BabylonJS/Editor/blob/master/CLAUDE.md), treated only as source evidence, not instructions.
- [W3] — Needle editor/web project separation.
- [W4] — Figma authoritative multiplayer/property model; [WASM canvas memory](https://help.figma.com/hc/en-us/articles/360040528173-Reduce-memory-usage-in-files).
- [W5] — Rive editor/runtime authoring loop.
- [W6] — Spline October 2026 changes; [shared 2D/3D product model](https://docs.spline.design/basics/what-is-spline).
- [W7] — Bevy's August 2026 editor/scene/UI progress.
- [N1] — React Flow performance; [accessibility](https://reactflow.dev/learn/advanced-use/accessibility).
- [N2] — Rete processing/plugin APIs; [Codegen status](https://retejs.org/docs/guides/processing/codegen/).
- [N3] — LiteGraph graph/runtime/serialization API.

[U1]: ../.refs/unity-cs-reference/Editor/Mono/Inspector/Editor.cs
[U2]: ../.refs/unity-cs-reference/Editor/Mono/UIElements/Inspector/InspectorElement.cs
[U3]: ../.refs/unity-cs-reference/Editor/Mono/SerializedProperty.bindings.cs
[U4]: ../.refs/unity-cs-reference/Editor/Mono/Inspector/Core/ScriptAttributeGUI/PropertyDrawer.cs
[U5]: https://docs.unity3d.com/6000.0/Documentation/Manual/domain-reloading.html
[U6]: ../.refs/unity-cs-reference/Editor/Mono/EditorSettings.bindings.cs
[U7]: https://docs.unity3d.com/6000.0/Documentation/Manual/Prefabs.html
[U8]: https://docs.unity3d.com/6000.0/Documentation/Manual/WhatsNewUnity6.html
[U9]: https://docs.unity3d.com/Packages/com.unity.graphtoolkit@0.4/manual/index.html
[U10]: ../.refs/unity-cs-reference/Editor/Mono/Undo/Undo.bindings.cs
[U11]: https://docs.unity3d.com/Packages/com.unity.shadergraph@17.0/manual/Master-Stack.html
[U12]: https://docs.unity3d.com/Packages/com.unity.visualeffectgraph@17.0/manual/GraphLogicAndPhilosophy.html
[E1]: https://dev.epicgames.com/documentation/en-us/unreal-engine/details-panel-customizations-in-unreal-engine
[E2]: https://dev.epicgames.com/documentation/en-us/unreal-engine/editor-ui-reference-for-niagara-effects-in-unreal-engine
[E3]: https://dev.epicgames.com/documentation/en-us/unreal-engine/sequencer-cinematic-editor-unreal-engine
[E4]: https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Runtime/Engine/UEdGraphSchema
[E5]: https://dev.epicgames.com/documentation/unreal-engine/blueprint-compiler-overview?application_version=4.27
[E6]: https://dev.epicgames.com/documentation/en-us/unreal-engine/world-partition-in-unreal-engine
[E7]: https://dev.epicgames.com/documentation/en-us/unreal-engine/one-file-per-actor-in-unreal-engine
[E8]: https://dev.epicgames.com/documentation/en-us/unreal-engine/using-live-coding-to-recompile-unreal-engine-applications-at-runtime
[E9]: https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Editor/UnrealEd/FTransaction
[E10]: https://dev.epicgames.com/documentation/en-us/unreal-engine/multi-user-editing-overview-for-unreal-engine
[E11]: https://dev.epicgames.com/documentation/en-us/unreal-engine/unreal-engine-material-editor-ui
[G1]: https://docs.godotengine.org/en/4.6/about/faq.html
[G2]: https://docs.godotengine.org/en/stable/getting_started/step_by_step/nodes_and_scenes.html
[G3]: https://docs.godotengine.org/en/stable/classes/class_editorplugin.html
[G4]: https://docs.godotengine.org/en/stable/classes/class_editorundoredomanager.html
[G5]: https://docs.godotengine.org/en/stable/classes/class_graphedit.html
[F1]: https://docs.flaxengine.com/manual/editor/index.html
[F2]: https://docs.flaxengine.com/manual/scripting/serialization/index.html
[F3]: ../.refs/flax/Source/Engine/Scripting/Scripting.cpp
[F4]: ../.refs/flax/Source/Editor/Surface/
[O1]: ../.refs/o3de/Code/Editor/MainWindow.h
[O2]: ../.refs/o3de/Gems/ScriptCanvas/Code/Include/ScriptCanvas/Translation/GraphToLua.h
[O3]: https://www.docs.o3de.org/docs/user-guide/assets/asset-processor/interface/
[O4]: https://docs.o3de.org/docs/learning-guide/tutorials/entities-and-prefabs/override-a-prefab/
[O5]: ../.refs/o3de/Code/Framework/AzToolsFramework/AzToolsFramework/Undo/UndoSystem.h
[O6]: https://github.com/o3de/o3de/issues/18910
[T1]: https://doc.qt.io/qt-6/qdockwidget.html
[T2]: https://github.com/ocornut/imgui
[T3]: https://dev.epicgames.com/documentation/en-us/unreal-engine/slate-architecture?application_version=4.27
[T4]: https://github.com/emilk/egui
[T5]: https://slint.dev/faqs
[T6]: https://zed.dev/blog/settings-ui
[T7]: https://www.electronjs.org/docs/latest/api/shared-texture
[T8]: https://v2.tauri.app/concept/process-model/
[T9]: https://github.com/chromiumembedded/cef
[T10]: https://github.com/dockview/dockview
[W1]: https://developer.playcanvas.com/user-manual/editor/scripting/vscode-extension/
[W2]: https://github.com/BabylonJS/Editor
[W3]: https://engine.needle.tools/docs/explanation/core-concepts/project-structure
[W4]: https://www.figma.com/blog/how-figmas-multiplayer-technology-works/
[W5]: https://www.rive.app/editor
[W6]: https://updates.spline.design/
[W7]: https://bevy.org/news/bevys-sixth-birthday/
[N1]: https://reactflow.dev/learn/advanced-use/performance
[N2]: https://retejs.org/docs/api/
[N3]: https://github.com/jagenjo/litegraph.js/blob/master/src/litegraph.d.ts

## Final editor architecture and v1 boundary

**Process model:** Electron application for UI/window management; native authoring host owns schemas, documents, transactions, edit-world projection and viewport rendering; disposable native play process runs simulation. Import/compiler workers are spawned jobs, not an elaborate service mesh. Electron itself has multiple internal processes. Host recovery uses document checkpoints plus the bounded journal; UI survives a native-host crash and reconnects. Headless clients use the same host protocol.

**UI and plugins:** React/TypeScript, Vite, Tailwind tokens, Zustand for view state, Dockview behind a panel adapter. Panels register a stable ID, title, factory, state serializer, context requirements and command contributions. Inspector drawers and graph domains register separately. Plugins receive scoped typed services rather than direct mutable world access; start with trusted first-party modules and version the extension boundary. Defer a general plugin marketplace.

**Commands and undo:** one schema-generated protocol; native validation and revision authority; semantic commands normalized to reversible document deltas; atomic cross-document transactions, gesture coalescing, selective runtime-change retention, recovery checkpoints. **Graphs:** React Flow interaction adapter, engine-owned text graph documents and registry, domain-specific compilers/previews, material graph first.

**V1 panel list:** Scene viewport; Game/Play viewport; Hierarchy; Inspector; Assets with import queue/dependencies; Console/Problems; Command History/agent changes; Material graph with preview/generated diagnostics; UI Composer with hierarchy/layout/state preview; basic Profiler/Frame Inspector. Command palette, play/pause/step, project/package settings and layout reset are shared shell surfaces. A minimal curve/clip inspector may serve UI animation; full Sequencer, general visual scripting, terrain suite and multiplayer sessions follow demonstrated need.

**The five editor decisions hardest to change later:**

1. **Document/schema identity and prefab override semantics:** stable object/property IDs, references, migrations and readable serialization determine every existing project.
2. **Authoritative mutation and history model:** command granularity, atomicity, revision checks, undo and external-edit reconciliation determine whether humans and agents can safely work together.
3. **Process and viewport boundary:** GPU ownership, presentation, input routing and crash isolation constrain desktop portability and remote editing.
4. **Graph semantic contract:** typed ports, node versioning, execution rules and compiler/debug mappings become permanent content dependencies.
5. **Runtime/editor UI contract and extension boundary:** shared layout/binding evaluation and stable panel/drawer/plugin services determine preview fidelity and the future ecosystem. Styling and docking libraries are comparatively replaceable when kept behind these contracts.
