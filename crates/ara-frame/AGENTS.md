# ara-frame

CPU description of one frame. `ara-render` draws it and re-exports these modules,
so existing `ara_render::scene` paths keep compiling. New code imports `ara_frame`.

## Public API

- `scene`: snapshot meshes, materials, lights, cameras and textures.
- `instanced`: native model-space geometry, materials, instances, decals and the
  `Textures` residency trait. `GeometryGpu` stays in `ara-render`.
- `look`: authored look, quality budgets, atmosphere, fog volumes, lens, probe
  records and reflection-capture specs. GPU passes stay in `ara-render`.
- `capture`: `Capture`, `CaptureRequest`, `ViewMode` and `compare`.
- `change_feed`: the render-v3 change feed. Hosts reach it through
  `instanced::transport`, which re-exports it.

No device, shader or pass graph. Gameplay state stays in ECS.

## Test

`cargo check -p ara-frame --all-targets` and `cargo test -p ara-frame`.
