# Animation v2 capture

`pose-strip.png` and `pose-strip.marks.png` are real deterministic Vulkan
captures from `ara-anim-capture`, using Ara's committed cooked renderer shaders.
Bone prisms are rotated 0.65 radians for visibility of sagittal motion.

Rows, top to bottom:

1. Cyan: sampled synthetic walk, six integer clip times.
2. Amber: the same poses after FABRIK hand reaches.
3. Green: IK-corrected retargeting onto a 1.3x humanoid.
4. Violet: actual SIMD database-selected walk/run/strafe/back/idle frames from
   six velocity/pose/trajectory queries. The exact source clips/ticks are in the legend.
5. Red: real Jolt powered pose, hit/ragdoll collapse, and powered get-up recovery
   at ticks 0, 25, 50, 90, 120 and 180.

The JSON stores device, driver, shader, fixed request and scene hash provenance.
The legend maps visible marks to the actual feature/tick names. All 30 pose entities
were visible. The image was visually inspected after capture, and Vulkan validation
completed cleanly. This is solver/debug geometry, not a skinned character-art capture.

Regenerate with `CARGO_BUILD_JOBS=2 cargo run -p ara-anim-capture`. On this Windows
machine, set process-local `VK_LAYER_PATH=C:/VulkanSDK/1.4.363.0/Bin` and an empty
`VK_IMPLICIT_LAYER_PATH` directory, following ara-render/run.ps1's validation setup.
The generated GPU cache and large render-ID buffer are intentionally not committed.
