# R05 — Runtime game UI and authoring tools

Research date: **2026-10-22**. Recommendation, not an implementation specification. “Verified” below means checked against the linked documentation or completed source clones; no engine or middleware was built or benchmarked. Performance targets are proposed acceptance criteria, not measured results. Friction assessments are architectural judgments unless attributed to a documented limitation or developer report.

## TL;DR

- Build **one native retained UI system** for menus, HUDs, and world surfaces, with independent layout, text, rendering, input, and semantic accessibility layers.
- Author visually editable UI in **restricted, valid TSX** with typed properties, bindings, commands, and a small Tailwind-like token vocabulary. JSX syntax does not require React, JavaScript, or a browser in the player.
- Promise lossless editing only for that declarative subset. Arbitrary React/TypeScript functions cannot reliably be inverted into a visual document.
- Make the visual designer a **source editor**: stable node IDs, precise syntax patches, shared undo, source provenance, and runtime-identical preview. No second authoritative scene or exported JSON tree.
- Prefer **Taffy** for Flexbox/Grid; **HarfBuzz + ICU**, fallback fonts, and cached glyph rendering for text; a native engine renderer for batching and effects; **signals/change notifications** for bindings.
- Steal Unity’s text assets and UI Builder, Unreal CommonUI’s input scopes and focus restoration, Godot’s reusable scenes/themes and text stack, and Flax’s uncomplicated world-canvas integration.
- Evaluate **RmlUi as the low-cost native baseline**, and NoesisGUI/Gameface as buy-versus-build alternatives. Do not mistake layout libraries for complete UI frameworks.
- Integrate Rive as an optional animated component. Ship structured inspection, component stories, localization previews, and semantic UI tests as first-class authoring tools.

## How each reference engine does it

### Unity — UI Toolkit and uGUI solve overlapping problems

**UI Toolkit:** a retained `VisualElement` hierarchy, UXML structure, USS styling, C# behavior, and a visual UI Builder. Builder exposes a library, hierarchy, inspector, viewport, and generated code preview. It is already a strong example of designers and programmers working on the same textual layout/style assets. USS is a Unity styling language with web ancestry; assuming arbitrary browser CSS works is a mistake. [Unity’s authoring walkthrough](https://learn.unity.com/tutorial/getting-started-with-ui-toolkit?version=6.0).

Unity 6 runtime binding connects ordinary C# objects to controls. Editor `SerializedObject` binding is a separate integration for serialized properties, undo, and multi-selection. The source exposes binding modes, update triggers, property-change notifications, and source version hashes: change-driven behavior exists, but careless sources or update policies can still cause repeated work. Evidence: [runtime binding](https://docs.unity3d.com/6000.0/Documentation/Manual/UIE-runtime-binding.html); local `Modules/UIElements/Core/Bindings/{DataBinding.cs,BindingUpdater.cs,VisualTreeDataBindingUpdater.cs,INotifyBindablePropertyChanged.cs,IDataSourceViewHashProvider.cs}`.

**Version distinction matters.** World-space UI Toolkit arrived in 6.2. The current 6.6 comparison includes world rendering, custom shaders, RTL, and emoji; it still distinguishes keyframed animation and in-scene authoring from uGUI. Unity’s September 2026 **6.7 beta** announcement reports in-scene authoring and keyframed animation becoming supported in that release, plus `gap`, `z-index`, and text improvements. Do not repeat 2024 claims that UI Toolkit cannot do world UI, or silently treat beta features as the 6.0 LTS baseline. [6.2 changes](https://docs.unity3d.com/6000.2/Documentation/Manual/WhatsNewUnity62.html), [6.6 comparison](https://docs.unity.com/en-us/engine/6000.6/manual/uitoolkits/ui-system-compare), [6.7 announcement](https://discussions.unity.com/t/state-of-ui-toolkit-in-unity-6-7/1736756).

**uGUI:** GameObjects/components, `RectTransform`, Canvas, prefab composition, event handling, and conventional scene/animation workflows. It remains convenient for a Unity developer placing a HUD or diegetic canvas and wiring component references. Its cost model is less friendly: Unity documents canvas rebatching after changes, nested layout-group costs, unnecessary raycast targets, and virtualization/pooling needs. [Unity UI optimization guidance](https://unity.com/how-to/unity-ui-optimization-tips).

**Good:** UI Toolkit separates structure/style/behavior, shares editor/runtime concepts, and has inspectable documents. uGUI makes scene placement and custom component integration approachable. **Friction:** two UI systems, version-dependent feature advice, editor/runtime binding distinctions, and styling internals add conceptual overhead. Recent official-thread feedback also reports difficulty editing dynamically created controls in the new tooling; this is an anecdote, not a prevalence estimate. **Steal:** textual documents, live styling, reusable controls, and computed-style inspection. **Avoid:** making users learn two permanent primary systems or depend on string queries to wire ordinary controls.

### Unreal — Slate, UMG, CommonUI, and MVVM are distinct layers

**Slate** is the native C++ widget framework; **UMG** exposes widgets through Unreal objects and Widget Blueprints. UMG’s designer supplies hierarchy, palette, properties, preview, and animation tracks, while the graph supplies behavior. This gives artists a substantial integrated workflow, including native asset and material access. It does not provide an ordinary text-first authoring contract for the visual layout. [Epic UI overview](https://dev.epicgames.com/documentation/unreal-engine/creating-user-interfaces-with-umg-and-slate-in-unreal-engine), [Widget Blueprints](https://dev.epicgames.com/documentation/unreal-engine/widget-blueprints-in-umg-for-unreal-engine).

**CommonUI** adds the game-specific layer many general UI libraries omit: active screen stacks, selective input routing, controller navigation, focus history/restoration, and platform-specific action presentation. It originated in Fortnite. These are core engine responsibilities worth adopting, without reproducing every Unreal class. [CommonUI overview](https://dev.epicgames.com/documentation/en-us/unreal-engine/overview-of-advanced-multiplatform-user-interfaces-with-common-ui-for-unreal-engine).

**UMG Viewmodel/MVVM**, currently documented as **Beta**, uses field notifications to push changes from a viewmodel into widgets. This is meaningfully different from legacy raw property bindings. Epic’s optimization guidance explicitly discourages polling bindings and tick-based UI logic; it describes layout/paint invalidation and render-to-texture Retainer Panels. Retained widgets do not automatically imply zero per-frame work. [Viewmodel](https://dev.epicgames.com/documentation/en-us/unreal-engine/umg-viewmodel-for-unreal-engine), [optimization](https://dev.epicgames.com/documentation/en-us/unreal-engine/optimization-guidelines-for-umg-in-unreal-engine).

Epic also documents screen-reader integration through UMG/Slate, with platform and widget coverage limits and an Experimental label. Accessibility is therefore a real system, but not evidence that every custom widget or target platform works automatically. [Screen-reader support](https://dev.epicgames.com/documentation/unreal-engine/supporting-screen-readers-in-unreal-engine).

**Good:** strong designer experience, game-specific input, animation, and rendering integration. **Friction:** several layers/plugins, state spread across assets and graphs, and binding/invalidation rules that require expertise. Designer assets are less convenient for generic text-editing agents than explicit source files; engine APIs can compensate, but become mandatory infrastructure. **Steal:** CommonUI’s behavior, notification binding, and profiler explanations. **Avoid:** a pleasant binding button that silently introduces frame polling.

### Godot — UI belongs to the normal scene model

Godot builds UI from `Control` nodes and reusable scenes. Anchors/offsets serve simple placement; Containers own child layout; Theme resources provide fonts, colors, constants, icons, and StyleBoxes. Signals provide ordinary event-driven behavior. The editor itself exercises these controls. Textual `.tscn`/`.tres` assets make scene structures available to agents without inventing a UI-specific binary format. [UI overview](https://docs.godotengine.org/en/stable/tutorials/ui/index.html); local `scene/resources/theme.h`, `scene/gui/{control.cpp,container.cpp}`.

The ownership rule is explicit: a Container overrides its children’s manual positioning. It is reliable once understood, but explains a common “the editor ignores my position” experience. Nested margin/box/panel containers can also make a simple-looking interface structurally verbose. [Container rules](https://docs.godotengine.org/en/stable/tutorials/ui/gui_containers.html).

Focus supports automatic navigation and explicit neighbors. The documentation warns that guessed neighbors can be wrong, and an initial focused control must be established. That is a reason for good defaults and inspection, not a reason to eliminate explicit overrides. [Navigation](https://docs.godotengine.org/en/stable/tutorials/ui/gui_navigation.html).

Godot’s advanced text implementation integrates ICU, HarfBuzz, FreeType, and MSDF infrastructure. Godot 4.5 introduced AccessKit-based screen-reader support, initially experimental; current cloned `Control` code contains accessibility updates and warnings. Do not describe Godot as having no screen-reader architecture. [4.5 announcement](https://godotengine.org/article/godot-4-5-making-dreams-accessible/); local `modules/text_server_adv/text_server_adv.{h,cpp}`, `thirdparty/README.md`.

**Good:** one scene/composition model, usable text assets, rich native controls, and reusable themes. **Friction:** anchors versus containers, nested wrapper nodes, and theme overrides that need provenance to understand. **Steal:** reusable UI documents and explicit layout ownership. **Avoid:** forcing every layout wrapper to become a full scene entity with unrelated lifecycle machinery.

### Flax — direct controls and canvases

Flax uses C# GUI controls, container panels, `UIControl` actors, and `UICanvas`. Its UI tools operate inside the prefab/game workflow, with selection, resize handles, pan/zoom, and anchor presets. Screen, camera, world, and camera-facing canvases are documented; the current source also has a GPU-texture mode. [Control authoring](https://docs.flaxengine.com/manual/ui/control/transform.html), [canvas](https://docs.flaxengine.com/manual/ui/canvas/index.html), local `Source/Engine/UI/UICanvas.cs`.

The implementation is approachable: controls expose `Draw`, layout, input, and focus; standard panels perform arrangements. `UIControl.Serialize` stores a control type and JSON properties, using prefab diffs where applicable. Brushes include textures, gradients, materials, and video. This is practical engine integration, rather than a web component ecosystem. Evidence: local `Source/Engine/UI/GUI/{Control.cs,Style.cs,Panels/,Brushes/}`, `Source/Engine/UI/UIControl.cs:324`.

**Good:** familiar C# objects, small conceptual surface, prefab reuse, and explicit render-pipeline placement for diegetic UI. **Friction/inference:** property-oriented control serialization is less expressive for shared design systems and compact component authoring than typed declarations. I did not verify a stock Flexbox/Grid CSS engine, comprehensive reactive binding framework, or cross-platform semantic accessibility bridge; that is an evidence limit, not proof of absence. **Steal:** canvas integration and straightforward extension points. **Avoid:** equating an editable object-property dump with a carefully designed authoring language.

### O3DE — LyShine is present; Shine is an open migration proposal

LyShine provides the runtime UI system and UI Editor: canvases, entity components, anchor/offset layouts, controls, animation, layout components, and interaction/navigation. Canvases can render into attachment images for placement in the world. [Gem documentation](https://www.docs.o3de.org/docs/user-guide/gems/reference/ui/lyshine/), [canvas properties](https://www.docs.o3de.org/docs/user-guide/interactivity/user-interface/canvases/canvas-properties/).

The completed October checkout still contains `Gems/LyShine`, its editor, tests, XML object-stream canvas serialization, and substantial slice ownership code. Its gem manifest explicitly describes runtime and creation tools. This is evidence against calling the subsystem removed or abandoned.

However, **RFC #19910, opened July 10, 2026, remains open**. It proposes replacing slice-based LyShine with prefab-based Shine, using spawnables and migration tooling. The proposal identifies UI as a remaining dependency preventing removal of legacy slices. Its described destination is not proof that the migration shipped. [RFC](https://github.com/o3de/o3de/issues/19910); local `Gems/LyShine/gem.json`, `Code/Source/UiCanvasComponent.cpp:3560`.

**Good:** a full editor/runtime system, entity integration, and usable world-surface/input features. **Friction:** UI has retained an older composition system while the broader engine moved on. That duplicates asset pipelines and migration obligations. **Steal:** modular integration and explicit canvas policies. **Avoid:** a UI-only prefab/serialization universe. snengine’s UI documents should use the same asset IDs, import dependencies, versioning, and overrides as its other authored assets.

## Third-party and state of the art

### Complete middleware versus useful building blocks

| Technology | What is verified | Judgment for snengine |
| --- | --- | --- |
| **NoesisGUI / Noesis Studio** | XAML, retained graphics, property/command architecture; Studio edits the same UI files standalone or inside an engine and supports hot reload. [Architecture](https://www.noesisengine.com/docs/Gui.Core.Architecture.html), [Studio](https://www.noesisengine.com/docs/4.0/Gui.Studio.Documentation_Index.html). | Strongest XAML counterproposal. Steal templates, design-time data, visual states, and designer/runtime agreement. Dependency-property/template sophistication can become ceremony. Confirm engine redistribution terms before choosing it as a bundled foundation. |
| **Coherent Gameface / Prysm** | Proprietary game-focused Cohtml and Renoir, HTML/CSS/JS, web libraries and DevTools. Gameface is explicitly not Chromium/WebKit. Prysm materials describe a designer/Adobe Animate workflow and engine integration using Cohtml. [Gameface](https://docs.coherent-labs.com/cpp-gameface/what_is_gfp/overview/), [Prysm](https://coherent-labs.com/products/coherent-prysm/). | Credible route to rich shipped game UI with familiar web tooling. Buying the runtime does not solve lossless editing of arbitrary application code. Clarify product packaging and per-title versus engine distribution. |
| **RmlUi** | Lightweight native C++, RML/RCSS subset, retained DOM-like elements, Flexbox, transitions, binding, spatial navigation, replaceable font/render interfaces; own update loop. [Repository](https://github.com/mikke89/RmlUi). | Best open native baseline to evaluate before building everything. Less browser scope is a feature; unsupported CSS must produce explicit diagnostics. Advanced text, accessibility, designer support, and desired Grid behavior need concrete validation. |
| **Ultralight** | Current 2.0 materials describe a single-process WebKit-based renderer, GPU/CPU paths, native DOM access, optional JS, and console offerings. [Product](https://ultralig.ht/), [integration](https://ultralig.ht/docs/2.0/integrating-into-a-game-engine). | Do not use old “desktop only” or “always CPU rasterized” descriptions. More web compatibility brings a larger subsystem to integrate. Verify actual licensed platform/back-end availability; marketing performance is not our benchmark. |
| **Clay** | C layout library, arena-oriented design, nested declarations, text measurement hook, draw-command output. [Repository](https://github.com/nicbarker/clay). | Excellent simplicity reference and useful for constrained tools/HUDs. Re-declaring layout does not supply complete controls, text editing, accessibility, or a visual source model. Avoid choosing it from a microbenchmark alone. |
| **Yoga** | C++ layout with C API, focused on a CSS/Flexbox subset; no drawing. [Official overview](https://www.yogalayout.dev/docs/about-yoga). | Strong low-friction C++ option if Flexbox is sufficient. Choosing it while promising CSS Grid creates a later compatibility problem. Pin default semantics; web-looking names do not ensure identical browser behavior. |
| **Taffy** | Rust layout implementing Flexbox, Grid, and Block; high-level tree and low-level embedding APIs with custom measurement. [API documentation](https://docs.rs/taffy/latest/taffy/), [project](https://github.com/DioxusLabs/taffy). | Preferred layout dependency. It does not implement text shaping, styling, widgets, input, or accessibility. C++ would need a maintained narrow FFI; validate that cost before commitment. |
| **Dear ImGui** | Immediate API emitting renderable geometry; explicitly aimed at programmer tools. Upstream states full text shaping/bidi and accessibility are unsupported. [Repository](https://github.com/ocornut/imgui). | Use for bootstrap diagnostics or specialized debug tools. It is a poor default foundation for polished, localized, accessible player UI. Immediate versus retained is not a GPU-quality ranking. |
| **Slint** | Declarative UI language, native toolkit, live preview/data editing, and Figma import workflow. [Project](https://github.com/slint-ui/slint), [tools](https://www.slint.dev/). | Good evidence that a constrained typed language enables productive tooling. Less immediate familiarity than TSX; native engine compositing, console needs, and applicable licensing require evaluation. Its toolkit is not merely a layout dependency. |

### Rive, Lottie, and animated UI

Rive now spans layout, components, text, state machines, data binding, editor tooling, and game runtimes; reducing it to “animated icons” misses its scope. Its current site also advertises agent/CLI authoring of the same design artifact. That is relevant evidence for an AI/human shared-document workflow, but **not verified evidence of lossless TSX round-tripping**. [Rive](https://rive.app/), [features](https://rive.app/features).

Use Rive for an animated gauge, character portrait, tutorial illustration, or richer interactive component behind a typed host contract. Keep semantic roles, localization ownership, focus, and game commands explicit at that boundary. Verify renderer/platform support against the chosen runtime: for example, the Unity repository currently lists D3D12 as planned rather than supported. [Feature support](https://rive.app/docs/feature-support), [Unity runtime](https://github.com/rive-app/rive-unity).

Lottie is useful for imported motion graphics, but After Effects feature coverage varies by player. **dotLottie now has state machines and theming**; calling all Lottie playback noninteractive is outdated. Its documentation still cautions that state-machine maturity/coverage varies. Treat these assets as optional animation components, not the engine’s screen/navigation model. [Supported features](https://lottiefiles.com/en/supported-features), [dotLottie runtime features](https://docs.lottiefiles.com/en/runtimes/overview/features).

### Web authoring, native reconcilers, and shipped evidence

ReactUnity renders React into uGUI/UI Toolkit without an embedded browser, using JavaScript-engine options and CSS/Yoga integration. Current OneJS documentation describes React 19 and a reconciler targeting UI Toolkit. Older OneJS descriptions centered on Preact are not a safe current specification. These projects prove **web authoring and native rendering are separable**; they do not prove zero JS overhead or reversible visual editing. [ReactUnity](https://github.com/ReactUnity/core), [OneJS](https://onejs.com/docs).

React Three Fiber offers the related pattern: declarative component lifecycle with native host objects, while rapid updates can use the frame loop and on-demand invalidation. Steal the separation between composition and hot-path mutation. A full React render/reconcile pass need not run at gameplay frequency. [R3F performance guidance](https://github.com/pmndrs/react-three-fiber/blob/master/docs/advanced/scaling-performance.mdx).

For concrete shipped web-tech evidence, Microsoft Flight Simulator 2020’s official SDK documents HTML/JS/CSS cockpit instruments and `Coherent.js` communication with the simulation. This establishes production use, not a universal performance comparison or proof that its historical Coherent version equals today’s Gameface. [Official SDK](https://docs.flightsimulator.com/html/Programming_Tools/JavaScript/Coherent.htm).

### Figma pipelines

Figma’s MCP tools provide structured design context, assets, and code-component mappings; current tooling also supports writing native design content. Code Connect maps design components to actual production components. Prefer this over treating every design as unrelated rectangles to regenerate. [MCP tools](https://developers.figma.com/docs/figma-mcp-server/tools-and-prompts/), [Code Connect](https://help.figma.com/hc/en-us/articles/23920389749655-Code-Connect).

An engine importer should preserve auto-layout intent, tokens, component identity, and exportable artwork, then diagnose unsupported features. Figma remains a concept/import tool: no reviewed evidence establishes arbitrary Figma ↔ game behavior ↔ TSX as a lossless transformation. Screenshot-to-UI is even less constrained: screenshots omit hierarchy, bindings, focus, responsiveness, and hidden states. Generate a draft from them, then validate it in the engine.

## Recommendations for snengine

### Choose restricted TSX as the editable source language

| Candidate | AI/code strengths | Human designer and runtime tradeoff |
| --- | --- | --- |
| HTML/CSS subset | Familiar vocabulary, compact structure, existing parsers. | Easily visualized when declarative; needs typed component/binding schemas and an honest compatibility boundary. Strong runner-up. |
| Arbitrary React TSX + Tailwind | Familiar to Islam and models; excellent component ecosystem. | Runtime/reconciler/JS integration costs; arbitrary control flow, hooks, spreads, and closures defeat a universal visual inverse. Reject as the lossless contract. |
| **Restricted typed TSX** | Existing language services, typed props/assets/bindings; familiar JSX and utility styles. | Requires a source compiler and syntax-preserving editor, but closed declarations make round-tripping tractable. Recommended. |
| XAML-like markup | Proven templates, typed controls, visual states, and binding model. | Strong designer fit; XML verbosity and a second language/concept system for a TypeScript owner. Prefer if buying Noesis. |
| JSON/YAML tree | Schema validation and structured agent patches are easy. | JSON is verbose; YAML adds scalar/indentation ambiguities. Both are weak component-programming surfaces. Use JSON for transport/compiled inspection, not a second authored truth. |
| New typed DSL | Can precisely constrain semantics and guarantee editability. | New parser, formatter, LSP, documentation, and unfamiliar syntax. TSX can provide the constrained surface with less ecosystem invention. |

**The language is a declarative TSX DSL, not React with hidden restrictions.** Use a compiler-owned `defineUI` declaration, registered components, literal styles, typed binding references, named actions, and explicit conditional/repetition nodes. Normal TypeScript controllers implement computations and effects. Compile declarations to a versioned UI intermediate representation with source maps; ship that artifact, not a TypeScript parser or browser. Native/game scripts can drive the same typed model schema.

Illustrative proposed format; names are not existing APIs:

```tsx
import { defineUI, Panel, Text, Progress, Button } from "@snengine/ui";
import type { PauseModel } from "./pause.model";

export default defineUI<PauseModel>((model) => (
  <Panel nodeId="pause" role="dialog" labelKey="pause.title"
    className="absolute inset-0 flex-col items-center justify-center gap-4 bg-scrim">
    <Text nodeId="title" textKey="pause.title" className="text-heading" />
    <Progress nodeId="health" labelKey="hud.health"
      value={model.bind("healthFraction")} />
    <Button nodeId="resume" textKey="pause.resume" autoFocus
      onPress={model.action("resume")} className="button-primary" />
  </Panel>
));
```

`model` is a compiler-recognized binding scope. The generic schema constrains field/action names and types; the declaration callback is parsed, not executed to discover a tree. Derived values such as `healthFraction` live in the controller’s computed signals. Components expose typed props, slots, variants, and design-time fixtures. Preserve an escape hatch for custom native drawing behind a registered component, with its internals opaque to the designer.

Use Tailwind-like **finite utilities and semantic tokens**, not a promise to run every Tailwind plugin. Compile utilities to style records; reject unknown classes. Define component defaults → theme/variant rules → explicit local overrides, with state priority and token provenance visible. Reject conflicting utilities instead of depending on class order. Responsive conditions use panel size and input modality; support logical start/end properties and RTL. Avoid unrestricted descendant selectors and arbitrary runtime-generated class strings in v1.

### Make lossless round-tripping a testable contract

1. Parse source with retained tokens/comments and stable authored IDs. A no-op designer save must be byte-identical. A one-property edit should patch that property only.
2. Store component-instance identity and authored overrides separately in the compiled model. Selection resolves to its source declaration; editing an instance must not silently rewrite its component definition.
3. Show where each value originates: token, component default, variant, binding, animation, or local override. Changing a bound value edits its binding or fixture, not a contradictory constant.
4. Use revision-checked transactions for editor and agent edits, shared undo/redo, and conflict diagnostics. Do not overwrite external edits from stale designer state.
5. Reject unsupported expressions clearly, or preserve them as opaque code-owned components. Never flatten a dynamic tree into yesterday’s observed hierarchy and call it round-tripping.

These guarantees apply to source preservation and semantics within the supported subset. They do not imply pixel identity between browsers, font versions, or GPU vendors. The native preview must run the production layout/text/render path; a browser approximation would weaken the central promise.

### Runtime: explicit work, bounded scope

**Layout.** Adopt Taffy, pin its version and defaults, and expose Flexbox/Grid plus absolute overlays. Let the engine own nodes and measurement contexts; start with Taffy’s simpler tree API unless profiling justifies custom storage. If the engine is C++, use a coarse-grained C ABI with batched style/node updates, not per-field cross-language calls. Validate target compilers/consoles early. Yoga is the fallback if Rust integration is untenable; document a Flexbox-only product scope rather than quietly emulating Grid. Taffy computes boxes; the engine still owns scroll behavior, baselines, clipping, safe areas, styling, and text measurement.

**Text.** Use HarfBuzz for shaping and ICU for bidi, segmentation, line breaking, and locale data; integrate paragraph layout, fallback selection, and editing around them. HarfBuzz alone does not provide the bidi paragraph algorithm. Preserve glyph clusters through cursor movement, selection, hit testing, and accessibility. Test Arabic/English mixtures, Indic scripts, CJK, combining marks, and emoji ZWJ sequences before finalizing interfaces. [HarfBuzz responsibilities](https://harfbuzz.github.io/what-harfbuzz-doesnt-do.html).

Start with cached FreeType rasterization at small screen sizes and MSDF/MTSDF for scalable/world text, subject to actual readability tests. MSDF preserves corners better than single-channel SDF, but is not a shaping or color-emoji solution. Provide a separate color glyph path and explicit bundled fallback fonts; do not depend on whichever system fonts happen to exist. Cache shaped runs by text, font/style, language, direction, and relevant layout inputs. [msdfgen](https://github.com/Chlumsky/msdfgen).

Evaluate **Slug** for perspective-independent outline rendering. Also evaluate current **HarfBuzz hb-gpu**, which implements Slug-algorithm outline rendering and color paint support: this is a material 2026 development, not hypothetical future work. Keep shaping/paragraph APIs independent of glyph rasterization so it is an experiment rather than a rewrite. Do not assume hb-gpu replaces the commercial Slug SDK or has identical platform/hinting characteristics. [Slug](https://sluglibrary.com/), [hb-gpu](https://harfbuzz.github.io/harfbuzz-hb-gpu.html).

**Rendering.** Maintain retained layout and paint data, then emit engine-native display lists/batches. Track hierarchy, measure/layout, paint, and transform changes separately. A blinking caret or changing health fill should not relayout an entire HUD. Cache geometry and glyphs; resubmit/composite visible UI as needed without assuming retained means no GPU work. Batch adjacent compatible draws while preserving transparency order. Expose texture/material/clip changes and effect passes as batch-break reasons. Use atlases, scissors for rectangular clipping, stencil/masks when necessary, and pooled offscreen targets for expensive effects. Cache-to-texture selectively; memory, fill rate, and blur passes remain real costs.

Expose nine-slice images, vector assets, gradients, custom UI materials, video, and render-texture/3D-view components as the fidelity path. Share material assets and profiling with the engine, but define a UI shader contract for clipping, premultiplied alpha, and color space. Implement HDR composition deliberately. Screen UI normally composites after temporal processing; world UI needs explicit depth, lighting, resolution, and temporal policies. A world panel maps the same logical UI tree onto a plane or texture surface, with ray-to-local input mapping. Keep a sharp render-to-texture path for curved/diegetic surfaces and a direct path where appropriate.

**Bindings.** Use typed signals, computed values, observable collections, and named commands. Default one-way model → view; edited values send explicit actions with validation. Two-way convenience belongs on well-defined form controls. Batch changes once per UI update phase, invalidate only subscribers, and define reentrancy/cycle behavior. Lists require stable item keys, virtualization, recycled-state cleanup, and focus restoration by item identity. UI commands enqueue gameplay intents at a documented simulation boundary; rendering never reads mutable gameplay objects from arbitrary threads.

**Motion, themes, localization.** Animate retained properties with transitions/tweens and named visual states; add timeline tracks when product use justifies them. Transform/opacity animation should avoid measurement where possible, while animated size must report its layout cost. Use a UI clock that can continue while gameplay is paused, plus reduced-motion settings and deterministic preview stepping. Tokens cover colors, type, spacing, radii, durations, and accessibility scale. Localize keys with typed parameters/plurals, not concatenated English strings. Include pseudo-localization, RTL previews, dynamic font fallback, and user text-size changes; do not solve overflow by silently shrinking accessibility text.

**Input and accessibility.** Build CommonUI-like modal scopes, back/cancel routing, focus restoration, logical keyboard tab order, gamepad directional defaults with explicit overrides, and device-dependent action prompts. Handle per-player focus/input for split screen. Pointer hover and controller focus are separate states. Define capture/bubble events, clipping-aware hit tests, drag/drop, IME, clipboard, and text-editing behavior. Publish semantic roles, names, values, relationships, actions, reading order, and live announcements independent of visual geometry. AccessKit is a strong desktop/mobile bridge candidate; it is not a ready-made console screen reader. [AccessKit](https://github.com/AccessKit/accesskit).

**Performance.** At 60/120/240 Hz the entire frame is approximately 16.67/8.33/4.17 ms. Propose an initial desktop reference HUD target of **≤0.3 ms CPU and ≤0.3 ms GPU at p95**, measured separately with named hardware/content; stress inventories and animated menus get their own budgets. These are goals, not middleware claims. Record p99 spikes, allocations, glyph misses, layout passes, render-target memory, overdraw, and input latency. Test static HUDs, scrolling virtualized inventories, RTL text entry, 4K effects, and many world labels. Optimize observed costs; do not require zero allocation everywhere to ship the first screen.

### Editor-side designer: one document, several views

Dock a **UI Designer** beside the game view: component library/hierarchy on the left, production-rendered canvas or world preview in the center, typed inspector with binding/token provenance on the right, and an optional source pane. Dragging inside Flexbox changes order/alignment/gap; it does not manufacture absolute coordinates. Show overlays explaining layout ownership, clipping, focus paths, and inherited values. Live reload patches the running document while preserving compatible state, focus, and scroll position; incompatible schema changes explain and reset only the affected component.

Add Storybook-like component stories using the actual runtime, typed fixture models, sizes, locales, input modes, themes, and animation states. Stories are also AI examples and reproducible tests. Native preview can be embedded in the editor’s chosen shell: using React for editor chrome does not require shipping React for game UI.

Expose the same inspection/transaction API to the designer, CLI, and agents: stable IDs/source spans, computed boxes, style origins, overflow, clipped text, glyph fallback, accessibility names, focus graphs, draw costs, and batch-break causes. Contrast checks should report uncertainty over dynamic game imagery rather than invent a precise score. Screenshot generation must name viewport, scale, locale, fixture, and animation time.

Offer role/text queries and actions resembling Testing Library: find the localized Resume button, activate it, check focus restoration and the resulting game command. Combine semantic assertions with targeted visual snapshots; use fixed fonts, clocks, and data. Description/screenshot generation should choose existing components/tokens, compile, inspect diagnostics, and compare multiple representative states. Agent output is an ordinary source transaction humans can review.

### Sequence and relationship to the five pillars

First prove one pause menu, one virtualized inventory, and one world panel through source edit → visual edit → live reload → controller navigation → localization/accessibility → automated assertion. Compare the native prototype against RmlUi on those same tasks before investing in a broad widget catalog. Keep one chosen runtime; an evaluation adapter need not become a permanent multi-backend abstraction.

- **AI-native:** typed source, stable identities, structured inspection, and transactional edits let agents work without guessing screen coordinates.
- **Fidelity:** engine materials/compositing, scalable text, effect components, and optional Rive support rich presentation without web compatibility dictating rendering.
- **Unity-level ease:** good controls, sensible layout defaults, scene placement, immediate preview, and automatic focus behavior reduce setup.
- **Great editor:** source-aware inspection, fast iteration, reusable stories, and actionable diagnostics make authoring predictable.
- **AI and human tools:** one editable declaration and one runtime preview eliminate the usual export/rebuild handoff.

The reusable pattern is **text source + schema + visual editor + live preview + structured inspection**. Dialogue/quests can share node IDs, condition schemas, and simulation fixtures; input maps can share typed actions and binding inspection; animation can share textual tracks and scrubbable previews. Reuse asset identity and edit transactions, not one universal tree language that makes every domain awkward.

## Open questions / risks

- **Native language and targets:** Taffy FFI, console toolchains, the scripting model, and editor shell are not decided here. Validate them before committing public APIs.
- **Scope honesty:** restricting TSX gives reliable design editing but less arbitrary code composition. The compiler, syntax patches, fixture system, and component schema are real engineering costs; do not advertise a completed React ecosystem.
- **Build versus buy:** no representative benchmark or integration spike was performed. RmlUi could be the better delivery decision if building complete controls/text/accessibility overwhelms the schedule. Middleware licensing and engine redistribution terms remain unverified.
- **Text correctness is expensive:** font fallback, bidi line wrapping, emoji, IME, selection, and screen readers cross subsystem boundaries. A glyph demo does not validate an internationalized UI framework.
- **Fidelity/performance conflict:** transparent HUD layers, blur, world-label counts, HDR, and temporal artifacts need target-device tests. No vendor’s “sub-millisecond” claim is adopted as an engine guarantee.
- **Round-trip ownership:** component overrides, generated repetitions, external code edits, bindings, and animated properties need explicit provenance. This is the most important authoring prototype to test early.
- **Reference limits:** clones are development snapshots; Unity’s clone is **7000.0.0a12**, not Unity 6 release source. No Unreal source clone was available. Current Rive/dotLottie/Figma feature claims were documentation-checked, not exercised; no statistical claim about developer dissatisfaction is made.

## Sources

All accessed 2026-10-22. Inline links identify the claim-specific primary documents. Local references below are read-only source evidence, with paths relative to `D:\Coding\projects\desktop\snengine\.refs\`.

| Clone | Commit and inspected evidence |
| --- | --- |
| `unity-cs-reference` | `bf6028387a4fc2ea6d41aa7d8998e774655aedf6` (2026-10-22; 7000.0.0a12). `Modules/UIElements/Core/Bindings/DataBinding.cs:18`, `BindingUpdater.cs:84`, `VisualTreeDataBindingUpdater.cs:191`, notification/hash interfaces; `Core/GameObjects/PanelSettings.cs:62`; `Core/Renderer/UIRenderer/UIRenderDevice.cs`. |
| `godot` | `9c2e39242e20846f84d990fe457a5e146b77fa36` (2026-10-22). `scene/gui/container.cpp:133`, `control.cpp:310`; `scene/resources/theme.h:39`; `modules/text_server_adv/text_server_adv.h:34` and `.cpp`; `thirdparty/README.md:8`. |
| `flax` | `394658f3cba5002c6cc16f054b1443a1b6022f05` (2026-10-22). `Source/Engine/UI/UICanvas.cs:16`, `UIControl.cs:324`, `GUI/Control.cs:434`, `GUI/Style.cs:8`, `GUI/Panels/`, `GUI/Brushes/`; `Source/Editor/Gizmo/UIEditorGizmo.cs`. |
| `o3de` | `2462b90ee389939336931a14e234fc4e3b4cd534` (2026-10-19). `Gems/LyShine/gem.json`; `Gems/LyShine/Code/Source/UiCanvasComponent.cpp:3560`; layout components, text/input components, and `Gems/LyShine/Code/Tests/`. |

Primary source indexes for follow-up:

- Engines: [Unity runtime binding](https://docs.unity3d.com/6000.0/Documentation/Manual/UIE-runtime-binding.html), [6.6 feature comparison](https://docs.unity.com/en-us/engine/6000.6/manual/uitoolkits/ui-system-compare), [Epic UI documentation](https://dev.epicgames.com/documentation/unreal-engine/creating-user-interfaces-with-umg-and-slate-in-unreal-engine), [Godot UI](https://docs.godotengine.org/en/stable/tutorials/ui/index.html), [Flax UI](https://docs.flaxengine.com/manual/ui/controls/index.html), [LyShine](https://www.docs.o3de.org/docs/user-guide/gems/reference/ui/lyshine/), [Shine RFC](https://github.com/o3de/o3de/issues/19910).
- Middleware: [Noesis Studio](https://www.noesisengine.com/docs/4.0/Gui.Studio.Documentation_Index.html), [Gameface](https://docs.coherent-labs.com/cpp-gameface/what_is_gfp/overview/), [Prysm](https://docs.coherent-labs.com/unity-prysm/), [RmlUi](https://github.com/mikke89/RmlUi), [Ultralight 2.0](https://ultralig.ht/docs/2.0/integrating-into-a-game-engine), [Slint](https://github.com/slint-ui/slint).
- Building blocks: [Taffy](https://docs.rs/taffy/latest/taffy/), [Yoga](https://www.yogalayout.dev/docs/about-yoga), [Clay](https://github.com/nicbarker/clay), [Dear ImGui](https://github.com/ocornut/imgui), [HarfBuzz](https://harfbuzz.github.io/), [hb-gpu](https://harfbuzz.github.io/harfbuzz-hb-gpu.html), [Slug](https://sluglibrary.com/), [msdfgen](https://github.com/Chlumsky/msdfgen), [AccessKit](https://github.com/AccessKit/accesskit).
- Authoring/integration: [Rive support matrix](https://rive.app/docs/feature-support), [dotLottie runtime](https://docs.lottiefiles.com/en/runtimes/overview/features), [ReactUnity](https://github.com/ReactUnity/core), [OneJS](https://onejs.com/docs), [React Three Fiber](https://r3f.docs.pmnd.rs/), [Figma MCP](https://developers.figma.com/docs/figma-mcp-server/tools-and-prompts/), [Flight Simulator SDK](https://docs.flightsimulator.com/html/Programming_Tools/JavaScript/Coherent.htm).

**Closing decision record.** Recommended stack: **restricted `.ui.tsx` + compiled token styles → native retained UI; Taffy layout; HarfBuzz/ICU with cached raster/MSDF text; engine display-list renderer; typed signals/commands; optional Rive components.** The designer edits that same source, previews the shipping runtime, and shares inspection/edit APIs with agents.

The **five UI decisions hardest to change later** are:

1. **Authoritative source and identity:** the TSX subset, stable IDs, component instances/overrides, and lossless edit contract.
2. **Layout/style semantics:** units, default box sizing, intrinsic measurement, Flexbox/Grid rules, precedence, and RTL behavior.
3. **Text model:** Unicode/grapheme indices, bidi/fallback ownership, selection/IME, and localization interfaces.
4. **State and interaction contracts:** binding ownership, command timing, event propagation, per-player focus, and semantic accessibility.
5. **Rendering boundary:** native paint representation, material/clipping/color-space contracts, and screen/world-surface composition.
