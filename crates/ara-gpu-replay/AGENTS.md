# ara-gpu-replay

Test support only: no engine GPU dependency and no production shader loader.

- `fixture`, `downsample`, `upsample`, `tone`: independent scalar references, with
  half-precision rounding at every RGBA16Float input/store.
- `msl(Pass)`: unchanged Renderlab WGSL math through naga; a checked entry shim reads
  Ara's 24-byte heap entries through TopLevel at buffer 2. Settings use storage.
- `spirv(Pass)`: WGSL resource-interface adapter to runtime descriptor arrays and
  32-byte roots. 3D sampled declarations alias binding 5; checked SPIR-V repairs add
  RuntimeDescriptorArray and correct naga30 resource-parameter pointer arguments.
- Parent GPU tests live in `ara-gpu`; this crate never creates a device.
- Shader copies are exact, pinned and attributed in README.md. Do not edit their
  algorithm while testing frontend compatibility.
- No generated outputs under tracked paths. Optional emit example writes target/.

Checks: CARGO_BUILD_JOBS=2 cargo test/clippy -p ara-gpu-replay; cargo fmt -p ara-gpu-replay.
