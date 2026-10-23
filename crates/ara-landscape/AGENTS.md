# ara-landscape

Native GPU landscape feature using ara-render::Graph and ara-gpu. Terrain and
foliage authoring live in their own crates. No gameplay state or simulation clock.

Public API: Landscape::{new,install_geometry,queue_tile,update,record,capture,
outputs,retire}, View, FoliageGeometry/FoliageMesh, material::evaluator, sample.
bind_virtual_textures accepts the shared ara-texture-stream Binding and four
virtual IDs for grass/rock/soil/snow (u32::MAX keeps a classic layer). Hosts submit
cache begin_frame before landscape sampling, end_frame after all consumers.
Sampling uses triplanar derivatives, shared min-mip feedback and bounded pages.
The new indirect draw command in ara-gpu is the only shared runtime hook.

Creation, mesh extraction, texture uploads and pipeline warming are synchronous
scene-preparation operations. Never call them on the live editor frame thread.
Prepare a replacement off the interactive path and publish under D20. Steady
recording performs no cooking, file I/O, resource creation or completion waits.
The host owns frame-slot completion, submission and retirement; update/queue_tile
reject overwriting in-flight upload buffers through ara-gpu's validation.
record advances graph state: submit its returned list before recording again;
discard/recreate this extraction if submission fails. Diagnostic capture waits.

Terrain is CDLOD over 32-cell patches. Six GPU-selected levels use exact shared
height samples, snapping fine border vertices to the coarser neighbor grid.
Missing pages enforce level >=2 and use a resident 8 m sample fallback. There are
256 detail pages (64 m tiles), independent of the 4096 m terrain extent. One tile
per frame, <=13084 upload bytes, including page-table/eviction metadata. The host
must reject stale completed revisions before queue_tile and honor Play/apply policy.

All passes declare complete bindless reachability. Static scene inputs are device
local; transient cull buffers are reused. Draw args are reset on GPU, with no count
readback. Terrain min-depth HZB uses full covered 32x32 blocks; projected sphere
bounds crossing >2 blocks conservatively skip the occlusion test. Shadow and
camera visibility are unioned, preserving off-camera casters.

Foliage has three LODs per grass/tree/rock family, complementary dither transitions,
current-tick vertex wind, and identical alpha/wind evaluation in depth/shadow/colour.
The sample uses original procedural stand-ins. Production assets use
FoliageMesh::from_cooked, FoliageGeometry::apply_bounds, then install_geometry.
Supply baked billboard impostor meshes/textures as each family's third LOD.
Asset textures bind raw UNORM; srgb flags control exactly one colour decode.

The terrain material is generated from ara-material's AST. evaluator extracts
only generated evaluation/custom functions; this crate supplies the larger
ara-render heap adapter and triplanar helpers. Four normalized splat channels.
Never change generated terrain_material.slang by hand. Run the offline cook.

Integration: Outputs exposes depth/shadow/HZB/SDR colour. ara-terrain's hole-aware
Heightfield::mesh supplies nav/GI/BLAS input. Fidelity should replace the sample's
ambient/probe fallback and lighting function, reusing the same wind/alpha masks.
This module does not enable RT or claim DDGI support. Metal needs ara-gpu's Metal
backend; cooks include SPIR-V and DXIL for the existing converter lane.

Set CARGO_BUILD_JOBS=2. Run ./run.ps1 -Mode Check for tests/clippy/fmt with the
process-local empty Vulkan implicit-layer workaround. -Mode Cook generates and
validates offline shaders. -Mode Capture records noon/dusk and GPU distributions.
Check never updates goldens. For deliberate updates only, set
ARA_UPDATE_LANDSCAPE_GOLDENS=1 and run the landscape integration test; inspect images.
Goldens require this GPU/driver baseline, with 1-SSIM <=0.005 and exact repeat frames.
