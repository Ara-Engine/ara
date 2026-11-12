# ara-frame

CPU description of one frame, drawn by Renderlab (through `ara-runtime-render`) and read by
`ara-render-fidelity`.

## Public API

- `scene`: snapshot meshes, materials, lights, cameras and textures.
- `instanced`: native model-space geometry, materials, instances, decals and the
  `Textures` residency trait. `GeometryGpu` lives in `ara-render-fidelity`.
- `look`: authored look, quality budgets, atmosphere, fog volumes, lens, probe
  records and reflection-capture specs. GPU passes live in the renderer, not here.
- `capture`: `Capture`, `CaptureRequest`, `ViewMode` and `compare`.
- `change_feed`: the render-v3 change feed. Hosts reach it through
  `instanced::transport`, which re-exports it.

No device, shader or pass graph. Gameplay state stays in ECS.

## Test

`cargo check -p ara-frame --all-targets` and `cargo test -p ara-frame`.
