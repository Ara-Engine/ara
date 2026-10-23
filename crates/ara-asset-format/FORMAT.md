# Cooked formats, version 1

All integer and floating values are **little endian**. Readers borrow the original
file: no vertex, index, joint, name or keyframe allocations. They accept unaligned
input. Upload byte slices directly; only use checked Pod casts when the original
mapping is suitably aligned. Keep the mapping alive while borrowing a view.

## ARAF container (`.mesh`, `.skeleton`, `.clip`)

The 32-byte header is `ARAF` (4 bytes), version u32 (=1), kind u32
(mesh=1, skeleton=2, clip=3), section count u32, complete file length u64,
reserved u64 (=0). Unknown versions/kinds are errors, never guessed.

Each 24-byte table entry is tag [u8;4], record stride u32, absolute byte offset
u64, byte length u64. Payloads follow table order, have 16-byte file alignment,
never overlap and have zero-filled writer padding. Tags are unique. Length must
be divisible by stride. Readers may ignore additional sections; required section
layouts cannot change without a version bump. No pointers or native usize values
appear on disk. Writers run on little-endian hosts (all supported Ara targets).

### Mesh

| Section | Record bytes | Meaning |
|---|---:|---|
| BNDS | 24 | One min.xyz, max.xyz AABB, six f32 |
| VERT | 52 | QuantizedVertex below |
| VSRC | 4 | Source vertex index per cooked vertex, for morph targets |
| XSKN | 16 | Four extra u16 joints, four UNORM16 weights per vertex |
| INDX | 4 | u32 triangle indices for all LODs |
| LODS | 16 | first index u32, count u32, relative error f32, reserved u32 |
| MLET | 48 | Meshlet below, finest LOD only |
| MVER | 4 | Global u32 vertex indices for meshlets |
| MTRI | 1 | Three local u8 indices per meshlet triangle; alignment padding allowed |

QuantizedVertex fields, in byte order: position UNORM16[4] (xyz plus zero),
normal SNORM16[4] (xyz plus zero), tangent SNORM16[4] (xyz plus handedness),
half[4] (UV0.xy, UV1.xy), linear RGBA UNORM8[4], joints u16[4], weights UNORM16[4].
Position decode is `min + position.xyz / 65535 * (max-min)`; zero extent is valid.
Normalize decoded normals/tangents and all eight combined skin weights. XSKN is
zero for unskinned/four-influence meshes. Joint indices address the owning skin's
joint order, not node IDs. VSRC follows every tangent split, dedup and fetch remap;
it indexes original morph target arrays in the scene manifest.

Meshlet: vertex offset u32 (in MVER), triangle offset u32 (in MTRI), vertex count
u32 (1–64), triangle count u32 (1–124), sphere center f32[3], radius f32,
cone axis f32[3], cone cutoff f32. Offsets are element offsets, never byte offsets
except the u8 triangle stream. Decode vertices through MVER → VERT. LODs retain
the same vertex buffer; simplification can stop early when the error limit or
locked borders prevent further reduction.

Readers check all global/local indices, LOD and meshlet spans, vertex side-stream
counts, finite half UVs and finite bounds/culling data. Coordinate convention is
right-handed Y-up, meters; glTF's convention is retained, FBX is converted by ufbx.

### Skeleton

JNTS stride 112: parent i32 (-1 for a root), source node u32, column-major inverse
bind matrix f32[16], rest translation f32[3], rotation xyzw f32[4], scale f32[3].
Joint order is the source skin order; parents need not precede children. The parent
is the nearest ancestor that is also a joint. Rest TRS is **source-node-local**;
non-joint ancestors remain in the scene hierarchy and must participate in pose
evaluation. Skinning matrix is animated scene-global joint matrix × inverse bind.

NOFF is u32 offsets into NAME, with `joint_count+1` entries. First offset is zero,
last is NAME byte length. NAME is concatenated UTF-8, including empty names.
Offsets are monotonic UTF-8 boundaries. `joint_names()` borrows individual names.
Readers check parent ranges/cycles and all finite transform/matrix values.

### Clip

CHAN stride 32: node u32, path u32 (translation=0, rotation=1, scale=2, morph=3),
interpolation u32 (step=0, linear=1, cubic=2), time offset u32, key count u32,
value offset u32, components u32, reserved u32 (=0).

TIME and VALU are contiguous f32 streams. Offsets count scalar elements. Times are
seconds, nonnegative, finite and strictly increasing within each channel. Linear
and step store `key_count * components` values. Cubic stores in-tangent, value,
out-tangent per key; tangents are derivatives per second. Translation/scale width
is 3, rotation width 4; morph width is the target count. glTF retains interpolation
and original keys. FBX uses ufbx's 60 Hz bake and node IDs. Runtime interpolation
must normalize rotations; cubic tangents themselves are not unit quaternions.

## Textures (`.ktx2`)

Standard [KTX2](https://github.khronos.org/KTX-Specification/ktxspec.v2.html), with
Khronos-generated DFDs, explicit Vulkan format, one 2D image, no supercompression.
Level index is finest first; physical level data is smallest first, block-aligned,
for streaming. Each mip is independently uploadable. NPOT edges are replicated
only for the encoder's full blocks; header dimensions retain the actual image.

Color uses BC7 sRGB (linear alpha); linear data BC7 UNORM; normals BC5 UNORM XY
(reconstruct positive Z); masks BC4 UNORM; nonnegative opaque HDR BC6H UFLOAT.
Signed HDR/alpha falls back to RGBA16F; values exceeding half range use RGBA32F.
Full mip chains use linear-light, alpha-aware filtering for color, renormalized
vector filtering for normals, and area filtering including NPOT edge contributions.
Normal Y inversion and mask-channel selection are explicit settings.

The reader/writer also accept ASTC 4×4, 6×6 and 8×8, linear/sRGB, so a mobile
importer can share this boundary. The builtin encoder is desktop-only and reports
a mobile-profile error until an ASTC importer is installed. No BCn mobile fallback.

Textures check header shape, DFD/format consistency, exact mip byte lengths and
finite raw float values. Distribution zstd belongs outside GPU payloads (cache/
pack chunks), not in the renderer's upload path.
