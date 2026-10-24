# Editor tool pages: thumbnails, Material, VFX, Animation

Owner feedback (preview 4): the Animation, Material and VFX pages "look ugly and nothing like the polished pages in other engines", and model previews in Assets are "unreadable". This doc is the design each page is built to. It follows D18 (tokens only), the UX v2 density scale (`D:\AraRefs\editor-ui\ANALYSIS.md`, Appendix B) and the container rule: cards only for things you can select or drag; groups are a header plus rows.

**References** (`D:\AraRefs\editor-ui\`, sources in `SOURCES.md`):
- Thumbnails: `unreal5/content-browser-1`, `unity/project-2`.
- Material: `unreal5/material-editor-1`, `unity/shader-graph-1`, `unity/shader-graph-2`, `unity/shader-graph-3`.
- VFX: `unreal/niagara-1`, `unreal/niagara-2`, `unity/vfx-graph-1`, `unity/vfx-graph-3`.
- Animation: `unity/animator-1`, `unity/blend-tree-1`, `unreal5/anim-state-machine-1`, `unreal5/anim-state-machine-3`.

**Shared rules for every tool page**
- **Regions:** a 28 px toolbar, a left sidebar (the document's own data), the main surface (canvas, stack or preview), and a 24 px status strip. The global Inspector stays the property sidebar for the selected item, so no page grows a second details panel.
- **Toolbar:** the document breadcrumb sits first ("samples / material.graph"), then the page's mode segment, then icon tools in groups split by 1 px separators. Text buttons only where an icon would be ambiguous. Document creation lives in the Create menu and the empty state, not in every toolbar.
- **Status strip:** state first (icon + word, coloured by `success`/`warning`/`destructive`), then the first problem as a link that selects its node, then quiet stats on the right in `muted-foreground`, tabular numerals.
- **Typography:** Inter 12/16 for rows and node titles, 11/14 for pins, captions and stats, 13 for sidebar headings. JetBrains Mono only for the DSL source and ids. Nothing under 11 px at 1×.
- **Spacing:** 4 px grid; 24 px rows; 8 px panel padding; 1 px `separator-strong` between regions.
- **Selection:** `selection-surface` with `selection-foreground` for rows; a 2 px `primary` ring outside canvas items. Hover is `raised`.
- **Empty states:** icon, one-line title, one-line hint and the actions that fix it (create, open from Assets).
- **Colours:** categories use the `chart-1..6` tokens, so both themes stay legible; type colours on pins and wires are the shared `graph::port_color` table.

---

## 1. Asset thumbnails

**Today:** models render with one bluish placeholder material instead of their own, the key light is about five times overexposed, the backdrop is effectively black, and small models (the 6 cm Avocado) sit far away because the renderer's minimum camera distance is 0.2 m. A 384 px image is drawn into 64 px tiles by a single-mip bilinear sampler, a 6:1 minification that aliases badly. Unpublished models silently render as a cached placeholder box. Material spheres fill about 63 % of the frame, have no anti-aliasing, and metals turn near-black because the preview's ambient term is diffuse-only.

**Design** (`asset_tools/studio.rs`, `thumbnails.rs`, `material_preview.slang`):
- **One studio for everything.** Models, material spheres and textures share one neutral backdrop: a cool grey gradient (about #3c3f45 at the top to #222428 at the bottom) with a soft glow behind the subject. It reads in Ara Dark and Ara Light alike, as UE's dark thumbnail backdrops do.
- **Models keep their materials.** The thumbnail reads the published runtime generation (`world::gltf::published`) with its own PBR materials and decoded textures; FBX geometry renders in a neutral clay. An in-flight import is waited for (bounded, off-thread), never cached as a placeholder.
- **Normalized, tight 3/4 framing.** The subject is scaled to a unit bounding sphere, then shot from the front right (35° bearing, 24° up) through a long 29° lens; the camera is fitted to the box's projected corners and centred, leaving a 5 % margin. Tall, wide and flat models all fill the tile.
- **Soft studio light.** A warm key from above-left (directional, with a matching softbox for broad highlights), a cool fill on the right, a rim strip behind and a dim cyclorama bounce, all as a procedural HDR environment, so glossy and metal surfaces show readable reflections.
- **Materials on a sphere** that fills about 88 % of the tile, at the same bearing, under the same lights (analytic softbox reflections with a split-sum BRDF), with 4× rotated-grid supersampling. The Plane option uses the same studio.
- **Textures** keep their aspect ratio on the backdrop; transparent texels show a quiet checkerboard; small sources scale with nearest so pixel art stays sharp.
- **Crisp at 1× and 2×.** Every preview renders once as a 512 px master (cached by content) and is resampled with Lanczos to 2× its logical display size: 128 px for 64 px grid tiles, 192 px for the 96 px import preview, 496 px for the material editor preview. 1× displays are an exact 2:1 reduction and 2× displays are 1:1. Corners are rounded to 1/16 of the edge. A larger view shows the grid tile's image until its own render lands.
- **Effects** are framed on what their particles do: the CPU reference simulation runs for three seconds and the 2–98 % particle extents are framed from the same 3/4 bearing (emitter culling bounds are deliberately loose, 16 m boxes). The live VFX preview uses the same framing.

---

## 2. Material page

**Benchmarks:** UE's Material Editor puts the preview and details on the left, the graph in the centre, the palette on the right and stats below. Shader Graph floats a Blackboard (parameters as typed pills), the Master Stack, a Graph Inspector and the Main Preview over the canvas, and its nodes have a coloured category strip, typed coloured ports and inline values. Both make the canvas the hero.

**Where Ara beats them:** one document with two views (Graph and Source) that edit the same AST; parameters edited inline in the sidebar without opening a node; a live, checked preview that keeps the last good image while compiling; errors that select their node.

**Regions**
```
[samples / material.graph] [Graph | Source] | [+ Add node] | [frame][comment][group][reroute]
┌ Preview ───────────┐┌ canvas ──────────────────────────────────────────────────────┐
│   (sphere, 88 %)   ││  ┌▀▀ Parameter ▀▀▀▀┐        ┌▀▀ Surface output ▀▀▀▀▀▀▀▀┐      │
│ [◯ ▭]        Live  ││  │ roughness  float │        │○ Base color              │      │
├ Parameters      +  ┤│  │ 0.35           ●─┼────────┼● Roughness               │      │
│ ● color    ■ 0.18… ││  └──────────────────┘        │○ Metallic (unused, dim)  │      │
│ ● roughness  0.35  ││                              └──────────────────────────┘ ▣ │
└────────────────────┘└──────────────────────────────────────────────────────────────┘
 ✓ Compiled                                   3 parameters · 4/5 nodes used · Surface
```
- **Sidebar (264 px):** the live preview fills the sidebar width; under it a 28 px row holds the sphere/plane icon segment and the render state ("Live", "Rendering…"). Then **Parameters** (header with +), one 24 px row each: a type dot in the pin colour, the name, and an inline editor on the right (number field, colour swatch + value, toggle, texture name). Clicking a name selects its node; the selected parameter's row uses `selection-surface`.
- **Toolbar:** breadcrumb, Graph | Source, Add node (Space), Frame (F), Comment (C), Group (Ctrl+G), Reroute. The material DSL is a mode, never a permanent pane.
- **Status strip:** compile state, first error (click selects the node), then parameter count, nodes used, domain and variant budget.

**Nodes**
- 208 px wide, radius 6, body `card`, 1 px `separator-strong` border. Selection adds a 2 px `primary` ring 2 px outside; an error uses a `destructive` ring and a one-line message strip at the bottom.
- **Header (26 px):** `raised` fill with a 3 px category strip along the top in a chart token: parameters `chart-2`, constants `chart-6`, maths `muted-foreground`, textures `chart-4`, colour `chart-5`, the output `primary`. The title is the human name ("Roughness", "Multiply", "Surface output"); the registry id never appears on the node.
- **Value line:** parameter and constant nodes show their value under the header (a swatch for colours), the way Shader Graph shows inline values.
- **Pins:** 22 px rows, inputs on the left edge, outputs on the right with right-aligned labels; an 8 px circle in the type colour, hollow when unconnected and filled when connected. Unconnected output-node inputs are dimmed (UE greys unused pins).

**Wires:** 2 px cubic curves in the source pin's type colour; wires touching the selection draw at 3 px and full strength, the rest at reduced alpha when something is selected. A drag in progress is `primary`, or `destructive` with the reason inline when the connection is invalid.

**Canvas:** `background` with a 16 px dot grid and faint 128 px major lines; the minimap (bottom right) shows nodes in their category colours and hides when everything fits. Groups are translucent titled regions; comments are sticky notes.

**In-node previews** (Shader Graph): every operation node whose output is a value (float, vector or colour) shows a 144 px preview of that output under its pins. Each preview is the material cut at that node: an Unlit material whose colour is the node's value (scalars and vectors as grey or RGB), rendered on a sphere when the value depends on the surface (normals, view, position, fresnel) and as a flat swatch otherwise. Previews render on the thumbnail workers from the material's revision, cache by content, and keep the last good image while a new one renders; the toolbar's image toggle hides them. Parameters, constants and the output node stay compact.

**Add node (Space, Tab or the toolbar):** a search popup over the canvas. Results are grouped by category (Inputs, Math, Texture, Colour, Utility) with a colour dot, the human name, the signature in `muted-foreground` and the doc line on the selected row. Enter adds at the canvas centre (or at the pointer when opened there).

---

## 3. VFX page

**Benchmarks:** Niagara's Selection stack: an emitter header with enable and live particle count, then groups (Emitter Spawn/Update, Particle Spawn/Update, Render) with colour-coded left bars and (+) buttons, and 24–28 px module rows with inline summary chips, a lock and an enable checkbox. Unity VFX Graph stacks Spawn, Initialize, Update and Output contexts with blocks inside and a VFX Control panel (stop, pause, step, restart, play rate).

**Where Ara beats them:** deterministic fixed-tick previews (the same result every run, step-exact), preview-only Hide and Isolate that never touch the asset, and live per-emitter counters without rebuilding the page.

**Regions**
```
[samples / vfx.graph]                                                  [+ Add emitter]
┌ Emitters ───────── 2 ┐┌ preview (fills) ─────────────────────────────────────────┐
│ ▾ ✦ flame  312/512 👁 ◎││  312 particles · tick 240                                  │
│ ▍SPAWN            +  ││                                                            │
│ ▍ Rate      120 /s   ││                   (live GPU preview)                       │
│ ▍INITIALIZE       +  ││                                                            │
│ ▍ Sphere    r 0.3    ││                                                            │
│ ▍UPDATE           +  │├────────────────────────────────────────────────────────────┤
│ ▍ Curl      f 3 · s… ││ [⟲][▶][⏭]  0:04.00 · tick 240   ¼× ½× 1× 2×   312 particles │
└──────────────────────┘└────────────────────────────────────────────────────────────┘
 ✓ Compiled                          2 emitters · 9 modules · capacity 1024 · fixed 60 Hz
```
- **Stack (320 px):** emitter header rows (collapse, sparkles icon, name, live/capacity, Hide, Isolate). Each stage is a group: a 24 px header with an uppercase 11 px caption and a (+), and its module rows beneath, all sharing one unbroken 2 px left rail in the stage colour (Spawn `chart-6`, Initialize `chart-2`, Update `chart-3`, Render `chart-4`). Module rows: 24 px; the module's name, its kind muted after it, and an inline summary of the key values that fit (never a value cut mid-way), with an enable checkbox at the right edge (Niagara). Up/Down/Duplicate/Delete appear only on the selected row (and in the context menu and palette); they are never repeated on every row.
- **Enable:** unchecking a module keeps it in place, dimmed, with "disabled" as its summary; its settings and bindings stay authored, and simulation, code generation and rendering skip it. It is the `enabled` field of the stack module and the typed command `vfx.module.enable`, so agents and undo see the same edit.
- **Preview:** the largest square the area allows, framed on the simulated particles (see thumbnails); the transport row sits under it: restart, play/pause, step one tick, time and tick, rate segment, particles.
- **Inspector:** module header with its doc line, then 24 px property rows with units.

---

## 4. Animation page

**Benchmarks:** Unity's Animator: Parameters (and Layers) on the left with inline values (float fields, bool checkboxes, trigger dots), states as rounded boxes, a green Entry, the default state in orange, straight arrowed transitions (a reverse pair draws as two parallel arrows), the playing state highlighted with progress. UE's state machine: an Entry pill, dark rounded states with a state glyph, transition-rule circles on each arrow, the active state tinted with "Active for 106.8 s". Unity's 2D blend tree plots samples as diamonds with the live parameter as a red dot and a motion table beneath.

**Where Ara beats them:** conditions are printed on the arrows (no need to click each transition), transitions are listed in priority order with their conditions inline, and the controller is one deterministic document shared by humans and agents.

**Regions**
```
[samples / animation.animation] [States | Blend spaces] | [+ State][+ Blend][+ Transition] | [frame]
┌ Parameters    + ┐┌ canvas ──────────────────────────────────────────────────────────┐
│ ● speed   0.0  f││   (Entry)──▶ ┏━━━━━━━━┓   speed > 0.2    ┌────────┐               │
├ Transitions     ┤│              ┃  Idle  ┃ ════════════════▶│  Run   │               │
│ 1 Idle → Run    ││              ┗━━━━━━━━┛ ◀════════════════ └────────┘               │
│   speed > 0.2   ││                 default                                           │
│ 2 Run → Idle    ││                        ┌────────────┐                             │
└─────────────────┘└────────────────────────┤ Locomotion ├─────────────────────────────┘
 ✓ Valid                    3 states · 2 transitions · 1 parameter · default Idle · 60 Hz
```
- **States view:** states are 160 × 40 rounded nodes with no visible ports: `card` body, a state glyph, centred 12 px title, a 3 px left accent (`primary` for the default state, which also gets an "Entry ▶" pill linked to it). Transitions are straight 2 px arrows between node edges with arrowheads; a reverse pair is drawn as two parallel arrows offset by 6 px. Each arrow carries its condition summary ("speed > 0.2", "always") as a small label at its midpoint. Dragging from a state's edge handle (shown on hover) to another state adds a transition.
- **Blend spaces view:** pose nodes (clips, 1D and 2D blends) keep the node-and-pin style of the material canvas; blend nodes list their samples as pins.
- **Sidebar (240 px):** **Parameters** with inline typed values (number fields for float and int, a toggle for bool, a trigger glyph) and **Transitions** in priority order, each a two-line row: "1 · Idle → Run" and its conditions. Clicking selects the transition.
- **Inspector:** typed widgets: the pose operation is a full-width segmented choice (Clip, 1D, 2D, Add, Layer, Match), looping a toggle, durations in seconds with their frames beside them (ticks only in the tooltip); blend samples are a plot (labelled dots on one axis, or crossed axes for 2D) plus a sample table (name, X, Y). Transitions show their crossfade, exit tick and conditions ("When speed > 0.2").
- **Views:** States ↔ Blend spaces are registry commands (`animation.view_states`, `animation.view_blends`) that frame the new projection.
- **Play:** while Play runs and the page is in front, it follows the first scene entity (the selected one first) whose `Animator` uses the open controller, reading `play.read <entity>.AnimatorState` ten times a second. The playing state gets a `success` ring and a progress bar along its bottom; a cross-fade target fills its bar with the fade and its arrow turns `success`; parameter values in the sidebar become live read-outs; the status strip reads "Playing Run · 0.42 s". Progress repaints the canvas only; a new state or parameter value rebuilds the page.

---

## 5. Import settings (Inspector)

- A header with a 96 px preview (192 px image) beside the file name, its path and an importer chip; then the settings as 24 px property rows (label left, typed control right: toggles, segments for enums and quality, fields) with per-field docs as tooltips; Reimport as a real button; Uses / Used by as captioned lists with counts.

---

## Next in build order

- Material: node search at the pointer and pin-drag search filtered by type, sticky-note colours, Stats with instruction counts per platform.
- VFX: curve and gradient editors, an emitter timeline for multi-emitter systems, GPU ms in the transport.
- Animation: an Any State node, condition editing in the Inspector, a draggable live parameter dot on the blend plot (and the live value on it during Play).
- Thumbnails: a per-asset turntable on hover in the grid.

## Verification

Every page is captured offscreen (`--verify-panels`, a fresh profile) at 1440×900 (1×) and 2560×1440 (2×) in Ara Dark and Ara Light, before and after, and each capture is inspected for clutter, alignment, contrast and purpose-fit.
