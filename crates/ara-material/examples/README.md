# Starter materials

Each `.material` is the canonical text view of the adjacent `.graph`; both describe
the same typed AST. The graph is ordinary Ara strict JSON, with stable semantic IDs
and a separate `l_editor` canvas section. No textures are bundled: the example
indices describe slots that the renderer/importer must populate.

| Material | Demonstrates | Texture slots |
|---|---|---|
| Painted metal | Color sampling, tangent normal mapping, metallic/roughness PBR | 0 color, 1 normal |
| Car paint | Linear tint, explicit static coat variant, opaque typed Slang, clear-coat roughness | None |
| Foliage | Color/data separation, shared alpha masking, transmission extension | 2 color, 3 data/alpha |
| Emissive screen | Unlit HDR linear color with runtime intensity | 4 color |
| UI rounded panel | Pixel-sized rounded coverage, premultiplied UI alpha | None |
| Terrain blend | Two triplanar layers blended by bounded value noise | 5 grass color, 6 rock color |

TextureColor slots bind raw UNORM views; the generated sample helper decodes sRGB.
Normals use RGB tangent-space UNORM. Data textures and scene color stay linear.
Surface normal outputs are transformed with the renderer-supplied tangent frame.
See `../AGENTS.md` for the exact resource/root-constant and upload ABI.
