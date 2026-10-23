# Starter materials

Each `.material` is the canonical text view of the adjacent `.graph`. Values use
names in source; persistent IDs, declaration order and nonsemantic canvas metadata
live in the graph. Load existing text with dsl::reconcile against that graph so
matching names keep their IDs and layout. Use material.rename for intentional
renames; plain text renames create new nodes and report a recovery diagnostic.

| Material | Demonstrates | Resources |
|---|---|---|
| Painted metal | Color sampling, tangent normal mapping, metallic/roughness PBR | Paint asset, flat_normal fallback |
| Car paint | Linear tint, explicit static bool coat, opaque typed Slang, clear coat | None |
| Foliage | Color/data separation, alpha masking, transmission extension | Leaf color and alpha/data assets |
| Emissive screen | Unlit HDR linear color with runtime intensity | Screen color asset |
| UI rounded panel | Pixel-sized rounded coverage, premultiplied UI alpha | None |
| Terrain blend | Two triplanar layers blended by value noise | Grass and rock color assets |

Textures are illustrative asset references, not bundled files. The asset index
fills their IDs on save through ara-doc reference repair. The renderer resolves
assets or built-in white/black/flat_normal defaults to heap indices at bind time;
indices never enter authored files. TextureColor binds raw UNORM and is decoded
once by sampleColor; normal/data textures remain linear. See ../AGENTS.md for the
exact upload, binding and lighting integration contract.
