# ara-render-fidelity
Native entry: FidelityView::new_native consumes ara-render::instanced::Scene and
NativeResidency. Model-space pools/LODs build once; TLAS carries real transforms.
Native scene count bit31 selects 64B header / 16B mesh / 192B instance records;
material stride is 320B and geometry rows are 32B. Never add world-space copies.
The native helper include must exactly match ara-texture-stream's sampling source.
Raw graph views decode colors after filtering; glTF retains hardware sRGB views.
`update_native_scene` streams rigid matrices/materials/lights and changed skin
revisions; unchanged frames perform no GPU upload. Layout/generation changes need
a replacement view. Rejected poses retain published resources. Native Frame.scene.w
is a motion-pending boolean (legacy uses a previous-table index); successful render
commits presented world/skin history. Repeated renders must not repeat stale motion.

M3 lighting and post extension over ara-render's Graph and ara-gpu's device.
Scene snapshots and physical units are exactly ara-render::scene's contract.
No authoritative gameplay or authoring state lives here.

## API
- `FidelityView`: per-view resources; takes a borrowed device at frame boundaries.
- `Settings`, `FrameInput`, `UpscalerInput`: validated temporal/physical contracts.
- `ProbeVolume`, `BakedProbes`: automatic DDGI volume (`automatic_within` sizes one to a probe count) and offline fallback data.
- `scene::GpuScene`: stable instance IDs, masked ray-query material inputs, AS cache.
- `Reference`: high-sample, undenoised reference transport, shared shader semantics.
- `FeatureInputs`, `ImageInput`: completed HDR/depth/probes for external features.
- `feature_inputs`, `post_process`: compose feature HDR before the shared post stack.
- `set_reservoir_sampling`: explicit RT DI/path reuse and split low-light histories.
  Spatial reuse and visibility resolve one reservoir per 2x2 grid block
  (`ReSTIR.Spatial.Visibility`); `ReSTIR.Shade` lights every pixel with its own BRDF
  from the four nearest blocks.
- `set_bounded_reservoirs`: presets below Epic end reservoir paths in the DDGI
  terminal, trace glossy specular at half rate, use 4 DI candidates and skip the
  finest wavelet (frame bit 6, transport.z).
- `set_deferred_post`: live views with particles leave bloom/post/upscale/output to
  `post_process` (otherwise the chain ran twice per frame).
- `SunShadowSource`, `render_jitter`: completed VSM sun mask with RT residency fallback.
- `presets`: `QualityPreset` (Low/Medium/High/Epic), `QualityChoice` (`--quality`),
  `Capabilities::of`, `Plan::resolve(choice, caps, display, RenderOptions)` →
  `RenderPath::Raster(RasterPlan)` (D27 default: every preset, every GPU;
  `raster_quality()` budgets; `raster_scale()` render size, upscaled by TAAU) or, with `RenderOptions::ray_tracing` on an RT device,
  `Fidelity(FidelityPlan)` for Medium and above. `FidelityPlan::settings` at
  creation, `configure` after new/resize/content swap. D26 RT tiers: Medium 0.4,
  High 0.5, Epic 2/3 transport scale.
- `bake_probe_surfaces`: one-time ray-traced probe surface caches for the raster
  renderer's GI (`ara_render::look::gi`); needs one rendered RT frame.
- `environment::FrameEnvironment::resolve(&frame.look, env.exposure)`: the scene's
  `SkyAtmosphere`/`HeightFog`/`PostProcess` → `view.set_environment`; `raster()`
  gives the raster renderer's `Look`, `manual_exposure()` its exposure, and
  `ground_sun()` the attenuated sun.

## Gates
Use CARGO_BUILD_JOBS=2. `cargo test -p ara-render-fidelity`,
`cargo clippy -p ara-render-fidelity --all-targets -- -D warnings`,
`cargo fmt -p ara-render-fidelity -- --check`.
Cook offline with `cargo run -p ara-shader-cook --
crates/ara-render-fidelity/shaders/pipelines.json crates/ara-render-fidelity/cooked`.
GPU sequence/golden/performance gates live in ara-pathtrace.
`cargo run --release -p ara-render-fidelity --example native_motion -- native`
measures shared rigid motion; `native-skin` measures one changed skin, and no
argument measures the legacy world-vertex update. JSON goes under target/.
Use run.ps1 there for process-local Vulkan implicit-layer isolation.

## Contracts
Linear HDR, right-handed Y-up metres; camera depth is positive view distance.
Motion is current-to-previous UV, without jitter; previous depth/normal/ID reject
history. Light edits retain geometric history, use responsive radiance weights,
and update more probes with 16 rays each inside the same per-frame ray budget.
Direct radiance has separate temporal history; incident radiance uses a wavelet
filter without luminance rejection on rough surfaces. Checkerboard transport must
repair invalid history before temporal resolve. Reference targets stay RGBA32F.
Exposure is applied once after reconstruction; grading (white balance, contrast,
saturation, vignette) follows it in scene-linear, then the tone mapper (AcesFit,
PbrNeutral, Filmic) and the LUT. `post_process` replays the last frame (TAA kept). Capture is explicit; ordinary
frames never read back image data. GPU pipelines only consume cooked artifacts.
Every bindless input is declared to the graph, including masked textures.
After merging shader changes, recook the merged source tree: neither parent's
cooked cohort represents their combined scene/frame ABI. Keep the native
shared-transform/emissive-UV GPU assertion enabled when checking the new cohort.
Feature inputs remain owned by the view; consume them at a completed boundary.
External HDR post input returns Graphics/ShaderRead; output remains CopySource.
Keep the first 28 target slots, FILTER0=12 and the frame's seven read/write rows
stable for features/VFX. Reservoir guides and histories append private slots.
Reference transport remains independent; never enable reservoirs in Reference mode.
Frames currently serialize because the merged GPU API serializes diagnostics.
After changing the sheen BRDF, run ara-pathtrace's `bake_sheen` example before
cooking; its 16 KiB directional-albedo asset participates in the shader cohort.
Release resources using `destroy` before dropping the external device.
See README for unsupported backend contracts and measured gates; do not claim
unmeasured refit, NRD, FSR2, DLSS, or async overlap.
