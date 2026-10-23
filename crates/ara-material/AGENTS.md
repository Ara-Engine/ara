# ara-material

One material program with text and graph authoring views (A8). No renderer or ECS
dependency. Authoring errors can be saved/repaired; generation requires a checked
AST. Slang compilation is offline and external, never a render-thread activity.

## Public API

- `Material`, `Binding`, `Expr`, `Parameter`, `CustomFunction`, `Type`, `Domain`.
  Named SSA expressions refer to explicit persistent node IDs; no hidden graph IR.
- `dsl::{parse,print}`: bounded expressions, finite literals, typed resources,
  explicit domains and JSON-escaped opaque Slang bodies. Printing normalizes
  whitespace/comments; parse/print preserves the AST exactly, not original trivia.
- `Material::{to_graph,from_graph}`: same AST and supplied `ara_graph::Editor`.
  Material edge IDs derive from destination sockets, so rewiring preserves routing.
  Use `material.connect`, not arbitrary graph edge IDs, for material wiring.
- `check`, `CheckedMaterial`, `fold_constants`: types, domains, cycles, values,
  static budgets and reachable dependency scheduling. Constants use f32 arithmetic.
- `library::{BUILTINS,get,infer,registry}`: 47 documented operations with shared
  signatures, graph search metadata and work estimates.
- `Instance`, `InstanceAsset`, `VariantKey`, `ParameterSlot`, `PackedParameters`:
  definition defaults, saved/runtime overrides, static selections and uploads.
- `register(&mut SchemaDb)`: graph storage plus `materialinstance` document kind.
  Instance files contain `i_values` and an `@path#gr_id` definition reference.
- `commands::register`: typed text/graph apply, annotation/literal fixes, parameter
  reset, output reset, budget, rewiring and saved instance override/clear commands.
  Every live edit goes through EditCtx, yielding attributed undoable kernel ops.
- `generate`: checked, folded Slang source, node/line mappings, estimated sample/
  ALU/branch cost, parameter layout and a content-derived specialization key.
- `ShaderCompiler::{discover,compile}`: external slangc SPIR-V / SM6.6 DXIL;
  binary magic validation and compiler version/messages. Windows compiler work
  starts below normal priority without opening a console window.

## ara-render integration (after the renderer merges)

1. Register material schemas and graph/material commands in the engine's shared
   SchemaDb/Registry. Load definitions with `Graph::read` then `Material::from_graph`.
   Run `check`; forward the returned ARA diagnostics/fix commands to the problems UI.
2. Resolve saved `InstanceAsset`s against their definition. Runtime `Instance`
   overrides are cheap uniforms unless the definition lists static `variants`.
   `variant_key` ignores runtime override values. Enforce the definition's variant
   budget before scheduling reachable pass/tier/vertex-layout PSOs.
3. In a background cook worker, call `generate`, then invoke slangc externally.
   The provided `fragmentMain` is a compilation/preview harness. Renderer passes
   call generated `ara_evaluate(AraMaterialInput)` and consume `AraSurface`; select
   the renderer's real entry point when compiling its pass module. Use the same
   evaluation for depth/shadow opacity masking, decals and reference/RT shading.
   Derivative-dependent roundedPanel is restricted to the UI fragment domain.
4. The engine-owned `shaders/engine.slang` provides bindings/helpers and a compact
   GGX metallic/roughness + clear coat template. Renderer lighting passes supply
   actual lights/probes/shadows via `ara_engine_light`, rather than the validation
   entry's fixed light. Transmission is a bounded foliage extension field; richer
   scattering can extend engine lighting without creating a second material AST.
5. Keep S1 ABI v1: 32 bytes/eight words, in order `materialIndex`, `textureIndex`,
   `bufferIndex`, `flags`, `phase`, `aspect`, `padding0`, `padding1`.
   Vulkan push constants; DXIL b0/space0. Set 0: textures binding 0 (t0/space1,
   16 entries), RW float4 buffers binding 1 (u0/space2, 4 entries), sampler binding 2
   (s0/space0). Binding 3 remains available for the renderer TLAS.
6. `Instance::pack` returns row-major float4 rows plus slot metadata. Upload rows
   to buffers[bufferIndex] starting at materialIndex (a ROW offset, not a material
   ordinal). Mat3 has three padded rows, Mat4 four. Texture parameters store exact
   0..15 integer descriptor indices in x lanes. Compile with row-major matrix layout.
   Provide explicit normalized tangent/bitangent/world-normal and surface-to-camera
   vectors in AraMaterialInput; material normal outputs are tangent-space XYZ.
7. Bind TextureColor as raw UNORM: sampleColor performs sRGB decoding exactly once.
   TextureNormal uses RGB UNORM normals; TextureData is linear RGBA. Post-process
   sceneColor samples an already-linear scene image at draw.textureIndex. Collect
   resource reachability from texture-typed parameter slots/static values, plus the
   scene-color root index, and declare reads to the render graph. Descriptor count,
   residency, buffer bounds and safe retirement remain renderer responsibilities.
8. UI consumes linear premultiplied RGB/alpha and explicit clip coverage, with no
   surface lighting variants. Unlit/post return linear color; decals provide PBR
   channels and opacity. Surface exposes baseColor, metallic, roughness, normal,
   emissive, opacityMask, clearCoat, clearCoatRoughness and transmission.
9. Cache cooked modules by variant + ENGINE_TEMPLATE hash + compiler identity,
   target/profile/options and ABI version. Cache driver PSOs separately with device,
   driver, pass, render state and vertex-layout keys. DXIL still requires Metal
   Shader Converter and converter reflection before it is a Metal runtime artifact.
10. Build/validate replacement pipelines off the frame thread, then swap at a frame
    boundary subject to human apply policy. Keep last-good shader/layout/instance
    rows on failure. Opaque Slang is compiled in a disposable worker; the host owns
    cancellation/time limits. Neither successful compilation nor estimates prove
    visual quality or custom shader execution cost; preview in renderer integration.

## Testing

PowerShell setup: `$env:CARGO_BUILD_JOBS='2'` and
`$env:SLANGC='C:\VulkanSDK\1.4.363.0\Bin\slangc.exe'` (or set VULKAN_SDK/PATH).
`cargo check -p ara-material`
`cargo test -p ara-material` (compiler tests are required, never silently skipped)
`cargo clippy -p ara-material --all-targets -- -D warnings`
`cargo fmt -p ara-material -- --check`

Six starter DSLs + canonical graph JSON live in examples/. Source snapshots live
in tests/snapshots/. Every library operation and every domain is compiled to both
targets, including matrix overloads and opaque custom code. Snapshot refresh:
set ARA_UPDATE_FIXTURES=1 for example_roundtrips_and_codegen_snapshots, inspect the
diff, then clear it before running the final gates. Source trivia is not retained.
Diagnostic codes 3200-3206 are local pending central catalog integration.
The workspace owner should regenerate Cargo.lock when integrating these new crates;
this track owns only its crate directories and leaves root files unchanged.
