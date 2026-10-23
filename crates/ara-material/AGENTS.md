# ara-material

One typed material program, with name-based text and a graph projection (A8).
No renderer/ECS dependency. All live authoring uses ara-cmd/ara-ops; offline
compilation runs externally and only checked programs can generate shaders.

## Language and identity

- `.material` never contains semantic or material asset IDs. The file/asset index
  supplies material identity to `dsl::parse(source, asset_id)`.
- Values have unique names. `dsl::reconcile(source, &previous)` matches names to
  the graph's existing IDs. Use it when loading edits alongside an existing graph;
  parsing as a new asset intentionally creates a fresh name-to-ID mapping.
  dsl::reconcile_graph reads identity metadata directly, including draft sidecars.
- `material.rename { from, to }` changes a name without changing ID, connections,
  layout or overrides. Plain text renames are delete + create. ARA3208 offers an
  undoable rename recovery; its restoreId field is diagnostic-only.
- Optional `name #n_id` annotations can disambiguate identity. Canonical printing
  always omits them. Graph/layout data is authoritative for semantic identity.
- Parameters print in declaration order; lets use dependency order with author
  order as the ready-node tie-break; outputs use Domain::output_order. Custom
  functions print after parameters, before their call sites. Aliases use `let b
  : float = a;`. Whitespace/comments normalize; program and order round-trip.
- Numeric literals are plain numbers, vec2/vec3/vec4 or row-major mat3/mat4
  constructors. `linear(r,g,b)` means scene-linear RGB; `srgb(r,g,b)` and #RRGGBB
  mean encoded sRGB. Convert a named encoded color with `linear(encodedColor)`.
  No implicit color-space conversion or numeric-to-texture coercion occurs.
- Texture defaults/overrides are symbolic ParameterValue::Texture values:
  @textures/paint.png, @textures/paint.png#tx_id, white, black or flat_normal.
  `Material::resolve_assets` uses ara-doc's AssetIndex and reference repair on
  save: missing IDs fill from paths, authoritative IDs repair moved paths.
- Static switches are `static param coat : bool = true;`; the variant set is the
  Cartesian product of false/true choices. Ordinary parameters stay runtime values.
- Inline Slang functions retain typed I/O, docs and `cost(alu: N, samples: N,
  branches: N)` estimates. Calls use their ordinary function name.
- ARA3207 rejects draft IDs, array literals, texture slot numbers, variants arrays,
  custom.* calls and positional cost arrays with executable source fixes. Slot
  migration deliberately suggests a semantic fallback; it cannot infer an asset.

## Public API

- Material/Binding/Expr/Parameter/Type/Domain are the single AST, with graph IDs
  internal and declaration order explicit. ParameterValue distinguishes numeric,
  boolean and symbolic texture values. TextureSource/TextureDefault never store
  heap indices. Graph::Editor is separate and excluded from compilation hashes.
- Material::{to_graph,from_graph}: typed projections with deterministic destination
  edge IDs. Use material.connect for wiring; generic graph edge IDs are not the
  material format. Rename recovery can retain layout tombstones for deleted names.
- check/CheckedMaterial/fold_constants: types, domains, finite f32 values, cycles,
  static budgets, dependency scheduling and reachable constant folding.
- library::{BUILTINS,get,infer,registry}: 49 documented operations, signatures,
  generic graph search metadata and cost estimates (including named aliases).
- register: graph and materialinstance Schema DB document kinds. InstanceAsset
  saves a definition @path#id and typed overrides, including symbolic textures.
- commands::register: text/graph apply, rename, wiring, typed fixes, budget and
  instance override/clear commands. All mutations produce invertible kernel ops.
  Registers the `doc.create` material template: an empty, valid authoritative
  `.graph`, with the graph records and document lifecycle in one transaction.
- Instance::{validate,variant_key,pack}, parameter_layout: typed overrides, static
  selection and GPU uploads. Runtime artistic/texture overrides do not add variants.
- generate/Generated/ENGINE_TEMPLATE: Slang, node/line mappings, sample/ALU/branch
  estimates, runtime layout and variant identity. ShaderCompiler spawns slangc
  below normal priority with no console, retaining compiler identity/diagnostics.

## ara-render adoption (after the renderer merges)

1. Register schemas and commands in the engine's shared SchemaDb/Registry. For a
   new material, parse with its asset ID. For edits, load its .graph, reconstruct
   its identity metadata with dsl::reconcile_graph (draft sidecars retain their
   IDs/layout too). Save through material.text.apply or
   material.graph.apply; these normalize ordering and resolve asset references.
   Present all ARA diagnostics/fix commands; compile only after check succeeds.
2. Resolve InstanceAsset against the referenced definition. Overrides are typed
   values keyed by graph identity; the source editor displays names. Static bools
   derive two variants each; enforce budget before pass/tier/vertex-layout PSOs.
   Instance::variant_key excludes runtime override values.
3. Cook via generate in a background worker, then run slangc externally. The
   supplied fragmentMain is a compile/preview harness. Renderer passes call
   ara_evaluate(AraMaterialInput) and consume AraSurface with their real entry
   points. Depth/shadow masking, decal, reference and RT paths share evaluation;
   derivative-dependent roundedPanel is UI-fragment-only.
4. shaders/engine.slang owns the bindings/helpers and GGX metallic/roughness plus
   clear coat lighting. Supply actual lights/probes/shadows to ara_engine_light;
   the validation entry's fixed light is not the renderer integration. Foliage
   transmission is an extension field, leaving room for richer engine scattering.
5. Keep S1 ABI v1: eight words / 32 bytes, in order materialIndex, textureIndex,
   bufferIndex, flags, phase, aspect, padding0, padding1. Vulkan push constants;
   DXIL b0/space0. Set 0 textures binding 0 (t0/space1, 16 entries), RW float4
   buffers binding 1 (u0/space2, 4 entries), sampler binding 2 (s0/space0).
   Binding 3 remains available for the renderer TLAS.
6. At bind time call Instance::pack(material, resolve_texture). The renderer maps
   each TextureSource asset/default plus semantic Type to a resident heap index;
   the callback may fail for missing assets or descriptor exhaustion. Only the
   resulting GPU upload contains indices. Never write them back to defaults,
   overrides, .material, .graph or instance files. The S1 table permits indices
   0..15. Declare the resolved image reads to the render graph and retire heap
   slots/resources only after GPU use. Changing allocation never edits the asset.
7. Upload PackedParameters.rows into buffers[bufferIndex] at materialIndex, a ROW
   offset rather than material ordinal. Runtime parameter layout follows declaration
   order; static bools are compiled into source. Mat3 uses three padded rows,
   Mat4 four; compile row-major. Supply normalized tangent/bitangent/world-normal
   and surface-to-camera directions; material normal outputs are tangent-space XYZ.
8. TextureColor binds raw UNORM; sampleColor decodes sRGB once. TextureNormal is
   RGB UNORM normals; TextureData is linear RGBA. Post sceneColor is already-linear
   at draw.textureIndex, resolved by the renderer. Include it in resource reads.
   UI outputs premultiplied linear RGB/alpha plus clipping, without surface variants.
   Unlit/post output linear color; decals provide PBR channels and opacity.
9. Cache modules by variant + template hash + compiler identity/options/target/ABI;
   driver PSOs additionally need device/driver/pass/state/vertex-layout keys. DXIL
   still needs Metal Shader Converter plus its reflection before Metal runtime use.
10. Build replacement pipelines off the frame thread, then swap at a permitted
    frame boundary (D20). Keep last-good shader/layout/rows on failure. The host
    owns worker cancellation/time limits and human apply policy. Opaque Slang costs
    are declarations, not GPU measurements; renderer integration must verify visuals.

## Testing

Set CARGO_BUILD_JOBS=2. On this Windows machine set SLANGC to
C:\VulkanSDK\1.4.363.0\Bin\slangc.exe (or use VULKAN_SDK/PATH).
Run cargo check/test -p ara-material, cargo clippy -p ara-material --all-targets
-- -D warnings, and cargo fmt -p ara-material -- --check.

Six canonical DSL/graph pairs live in examples/. Slang snapshots live under
tests/snapshots/. Every operation and domain compiles to SPIR-V and DXIL; no
compiler tests silently skip. ARA_UPDATE_FIXTURES=1 refreshes the example round-trip
fixtures: inspect the diff, then clear it before final gates. Tests also cover
name matching, rename/recovery/undo, source ordering, literal types, draft fixes,
asset reference repair and renderer-only heap allocation. Codes 3200-3208 remain
local pending central catalog integration. The workspace owner regenerates
Cargo.lock during integration; this track leaves root files unchanged.
