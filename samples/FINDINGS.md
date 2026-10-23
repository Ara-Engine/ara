# Findings from building the real-art samples

What broke, what was missing and what was awkward when three scenes with real CC0
content (Poly Haven, Khronos samples, Mesh2Motion/Quaternius, OpenGameArt audio)
were built through Ara's own tools and rendered by Play, the fidelity renderer and
the editor. Each item has a repro. Severity: **P0** blocks real content, **P1**
visibly wrong or slow, **P2** friction.

Repro setup for everything below:

```sh
cargo build --release -p ara -p ara-player -p ara-import-worker -p ara-editor -p ara-samples
target/release/ara-samples fetch                  # ~510 MB of pinned CC0 files
target/release/ara-samples build outpost          # and: build showcase
target/release/ara-samples capture outpost outpost   # looks: outpost, hangar; showcase showcase
target/release/ara-samples play outpost levels/hangar.scene hangar
target/release/ara-samples editor outpost levels/outpost.scene outpost --camera 194,2,190,-2.55,0.42,75
```

## Engine bugs fixed in this branch (minimal, additive)

| Crate | Bug | Fix |
|---|---|---|
| `ara-doc` | `ara check` rejected **every** `AudioSource.clip` (`is a source asset, expected a audio`), including the engine's own `ara-player/demo` (3 errors there). `asset_kind = "audio"` names imported media, which the asset index never classifies. | `refs.rs`: kinds that are not registered document kinds resolve without the kind check. Demo and samples now check clean. |
| `ara-runtime` | Imported models lost their normal, metal/roughness and emissive maps in Play (only base colour was decoded, though the importer cooks all of them). | `ModelMaterial` gains `normal_texture`, `normal_scale`, `metallic_roughness_texture`, `emissive_texture`; `ModelTexture` records its cook usage. |
| `ara-runtime` | Cooked mesh LODs were discarded; everything drew at the finest LOD. | `Primitive::lods` exposes the coarser cooked LODs (finest stays in `indices`). The sample captures use them (Outpost: 39.5 M → 11.0 M triangles). Play does not select LODs yet. |
| `ara-runtime` | `.foliage` exclusion zones were ignored at runtime (scatter passed `&[]`): foliage grew inside the Outpost compound. | `terrain.rs` passes the document's `x_` exclusions. |
| `ara-runtime-render` | Play ignored normal/ORM/emissive maps; glTF emissive factors were not scaled by `MeshRenderer.emissive` although the field doc says so (imported lamps could never glow). | Presenter binds all four slots, decodes BC5 normals, scales emissive by the entity radiance. |
| `ara-runtime-render` | With real content Play failed outright: `ARA_GPU_CONTRACT: Sampled heap exhausted` (see P0 #1). | `SCENE_TEXTURE_BUDGET` (32): colour maps first, then normal/ORM/emissive; maps past it fold into material factors (mean texel of the coarsest mip) with one stderr note. Degrades instead of failing. |
| `ara-player` | Windowed players were killed by the owner's watchdog (`player_unresponsive: no heartbeat for 2.98s`) during their first frame: the full scene upload (CPU texture decode) is not covered by `play.busy`, whose grace ends with the `play.start` response. | The first presented frame of a world announces `busy("upload")`. |

## P0: blocks real content

1. **The whole device has 128 bindless sampled images.** `shaders/baseline.slang` and
   `ara-render-fidelity/shaders/common.slangh` declare `Texture2D textures[128]`;
   `ara-gpu/src/vulkan/heap.rs` sizes the set to match. Render targets, environment,
   landscape and VFX share it. The fidelity view therefore rejects scenes with more than
   96 textures (`Invalid texture count`) and Play fails with `Sampled heap exhausted`.
   The Outpost needs 224 maps, the hangar 170, the showcase 86 — a small game level.
   *Repro:* revert the presenter budget and `play outpost levels/hangar.scene hangar`.
   *Workarounds here:* the capture bridge packs UV-square materials into 4096² atlas
   pages (Outpost: 224 maps → 67 textures); Play folds maps past 32 into factors (so
   Play shows most props untextured). *Needs:* a variable-count/large descriptor array
   (NVIDIA allows ~10⁶) in one re-cook of every pipeline, plus streamed BCn textures
   (`ara-texture-stream` exists but neither renderer binds model textures through it).
2. **No path from a project scene to the fidelity renderer**, except the editor viewport,
   which draws imported models untextured (`ara-editor/src/world/gltf.rs`, flat
   `MeshRenderer.color`). Every fidelity capture in the repo before this was a hand-built
   Rust fixture. `ara-samples capture` is a working bridge (cook + simulate like Play,
   convert `RenderFrame`, composite VFX); it belongs in the engine.
3. **Textures are uploaded uncompressed in both renderers.** Cooked BC7/BC5 is decoded on
   the CPU to RGBA8 (`decode_texture`), and the fidelity scene builds a CPU mip atlas
   (2× size). Outpost upload: ~2.9 GB. The first windowed frame of a real level takes
   seconds of CPU decode on the frame thread.
4. **Fidelity scenes cap at 4096 meshes with no instancing**; transforms are baked into
   world-space vertices. 8 281 Outpost objects (5 543 foliage instances) only fit because
   the bridge merges geometry per material.

## P1: visibly wrong or slow

5. **Play's raster path does not scale to a real level.** It draws every object at its
   finest LOD as its own mesh: Outpost windowed 15 fps (p95 72 ms) at 1280×720 and the
   player was killed once by the 1 s watchdog after a 1.4 s hitch around tick 175–190
   (walking near the campfire; acoustics/audio work is joined on the frame thread).
   Hangar 31.6 fps (p95 34.9 ms), showcase 90 fps.
6. **Emissive surfaces do not light anything in Play.** The night hangar, lit by emissive
   panels and bulbs in the fidelity view, is near-black in Play (only `Light` components
   light the raster path).
7. **ReSTIR is 2.1× slower than the plain hybrid path in a realistic interior and
   noisier.** Hangar native 1440p: 23.6 ms with reservoirs vs 11.2 ms without;
   `ReSTIR.Spatial.Visibility.Shade` is 11.2 ms here vs 1.6 ms in the synthetic
   1000-light corridor (alpha-blended glass, real textures, 245 objects). Upscaled
   11.9 ms (> the 8 ms budget). Visible grain on corrugated walls at EV 6.
8. **Terrain materials stop at the renderer.** `Header.materials` is ignored; the
   landscape renderer and the windowed terrain meshes use fixed grass/rock/soil/snow
   colours, so an arid biome renders green unless soil is painted everywhere. The
   fidelity view has no splat or terrain path at all (the bridge picks ground or rock
   per 1 m cell from the rock weight and slope).
9. **Foliage rules' assets are never drawn.** The landscape renderer scatters procedural
   stand-ins by species (`Species::Grass/Tree/Rock`), so Play shows green cones where the
   rules place Namaqualand shrubs and rocks. Windowed frames draw no foliage at all.
10. **Alpha-blended surfaces render opaque in presented (windowed) frames** but correctly
    in `play.capture` (chain-link fence: grey panels vs see-through wire).
11. **glTF features dropped by the importer:** `KHR_texture_transform` (Poly Haven's
    chain-link wire tiles ×4 and rotates 90°; Ara ignores it, so the samples generate
    their own fence), `KHR_materials_emissive_strength`, `KHR_materials_transmission`.
    Poly Haven glTF downloads also ship colour maps as JPG with opacity in separate maps;
    nothing in the pipeline can merge them (the build does it, `kit::cutout`).
12. **Physical sky, fog, exposure and post cannot be authored in a scene.** `Environment`
    is a 3-colour gradient plus a linear exposure; the fidelity atmosphere, height fog,
    EV100/auto-exposure, tone mapper and bloom are view settings only. The samples keep
    them in `look/*.look.json` next to the camera shots. Play and the editor show the
    gradient sky; the fidelity captures show the physical sky.
13. **VFX is limited to the committed library.** Only `campfire`, `sparks`, `smoke`,
    `magic_swirl`, `muzzle_flash`, `rain` resolve (`depth_sparks` is cooked but not in
    `ara_vfx::samples::all()` → `no cooked VFX effect`). Project effects cannot be cooked,
    so the campfire's oversized smoke plume can't be tuned and there is no dust effect
    for the hangar. Compositing into the fidelity view works through
    `feature_inputs`/`post_process` (done by the bridge).
14. **Music longer than ~2.8 minutes cannot be imported.** The audio importer decodes the
    whole file against a fixed 64 MiB budget (`audio capacity exhausted: resident decode
    budget`), even for streamed music; not a sidecar setting. The 4-minute CC0 track was
    swapped for a 2-minute one.
15. **Editor viewport with real content.** The editor opens only the project's main
    scene (no Engine Protocol method opens another document), draws imported models
    with their flat `MeshRenderer.color` (no textures), and its console errors do not
    reach `ara logs`. See the editor captures.

## P2: authoring friction (CLI/MCP dogfood)

16. **No `doc.create`.** New scenes, terrains, foliage, animation and music documents
    must be written by hand as empty canonical skeletons before commands can fill them.
    `ara init` seeds `levels/main.scene` with a random ID; renaming it or pointing
    `main` elsewhere is a hand edit of `ara.project.json`.
17. **Import sidecars have no CLI/MCP command** (`editor.import.initialize` exists only in
    the editor host). An agent with only `ara`/MCP writes `*.import.json` by hand and
    must know the importer's exact version string (the build asks the worker).
18. **`.animation` and `.music` documents have no commands** (`AnimationCommand` is a Rust
    API; the demo writes music with the canonical writer).
19. **`MeshRenderer` can't address part of a model or override its material.** Poly Haven
    ships variant sheets (4 rocks, 5 bushes, a 103-piece pole kit in one file) and lamps
    with their bulbs off; each needs a derived glTF (`kit::variant`). There is no material
    asset slot on `MeshRenderer`, so a palette-textured character can't be recoloured
    without a derived texture.
20. **Colliders have no offset and models can't be colliders in play.** Model origins sit
    at the base, box colliders at the entity origin; every solid prop needs a second
    entity at its bounds centre. `PhysicsId` defaults to 1 and must be made unique by hand.
21. **Aiming lights and cameras needs Euler math.** `Transform.rotation` is intrinsic XYZ
    degrees; there is no look-at helper or `direction` field, and the obvious
    pitch/yaw split yields upside-down cameras for targets behind −Z.
22. **The landscape renderer requires an origin-anchored square terrain**, so a level
    centred in a terrain has to be offset by hand (`outpost::SHIFT`).
23. **Agent edits enter review.** A scripted build ends with `ara accept --all`, or every
    entity stays "unreviewed".
24. **What worked well:** typed batches with `@file` args, deterministic record IDs,
    exact diagnostics with JSON paths (`commands[6].args.components[NavMeshSurface].size`),
    `ara check`, terrain/foliage commands, the import worker (107 assets cook in 166 s
    cold, 4–8 s warm), `ara mcp` + `ara_play` for Play automation, and the fidelity
    view's `feature_inputs`/`post_process` seam.
