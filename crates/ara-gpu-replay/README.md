# Renderlab WGSL replay support

This test-only crate keeps Renderlab's WGSL algorithms and compiles them through
naga 30.0.1 directly to MSL/SPIR-V. It has no GPU device or production loader.

The exact shader copies come from Ara-Engine/renderlab commit
`71ea35542346c70a2eea0b48d8ef05a931876979`:
- https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/shaders/post.wgsl
- https://github.com/Ara-Engine/renderlab/blob/71ea35542346c70a2eea0b48d8ef05a931876979/shaders/post_tone.wgsl

Copied at Islam's explicit request for replay validation. Existing shader attribution
comments are retained.

Root words: input/colour, second/bloom, auxiliary, tile velocities, writable output,
settings storage-buffer slot, sampler slot, LUT slot. Each is a u32 heap index.

MSL changes only the entry interface: settings become read-only storage in naga IR;
a checked wrapper passes textures/samplers/typed storage from Ara's existing tables.
The native test harness prepends TopLevel and its 24-byte descriptor entry layout.
SPIR-V adapts WGSL resource declarations/references to runtime binding arrays.
Different 2D/3D sampled declarations use a temporary source binding, then alias onto
binding 5 in SPIR-V. The adapter also adds RuntimeDescriptorArray when naga 30
omits it for uniformly indexed runtime descriptor arrays (Vulkan VUID 04680).
Naga 30 also emits an image value where a texture helper parameter expects its
UniformConstant pointer when the call uses a binding-array element. A checked
SPIR-V repair substitutes the original pointer only after matching the exact callee
parameter type; it does not change shader arithmetic. All four emitted stages pass
`spirv-val --target-env vulkan1.3`. Descriptor aliasing and the generated pipeline
remain native-unverified until the PC test runs. There is no binding-0/low-128 workaround.

The scalar reference matches clamp filtering, tent weights, half stores, convex
upsample, tone/grade/LUT/vignette with grain and chromatic aberration disabled.
These are fixture settings, not removed renderer features.
