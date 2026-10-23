# Reference and fidelity verification

`Reference` progressively accumulates the shared transport kernel without GI
probes, denoising, history rejection or upscaling. Every sample is independent;
GGX/diffuse mixture PDFs and roulette weights preserve transport energy. Point
and directional visibility uses the same physical units and masked hit semantics
as hybrid rendering. Emissive surfaces are found through BSDF sampling; explicit
area-light next-event estimation is not implemented yet.

Run from the repository root (all commands scope Cargo to two jobs):

```powershell
$env:CARGO_BUILD_JOBS='2'
cargo run -p ara-shader-cook -- crates/ara-render-fidelity/shaders/pipelines.json crates/ara-render-fidelity/cooked
crates/ara-pathtrace/run.ps1 -Mode Check
crates/ara-pathtrace/run.ps1 -Mode Capture
crates/ara-pathtrace/run.ps1 -Mode Timings
```

The runner isolates Windows implicit Vulkan layers exactly as ara-render does;
Khronos API and synchronization validation remain enabled. Captures are explicit
GPU readbacks; ordinary render frames do not copy image data to the CPU.
Goldens update only with process-local `ARA_UPDATE_FIDELITY_GOLDENS=1`.

`Check` runs crate tests, clippy with warnings denied, and rustfmt checks. The GPU
test compares seven committed 320x180 goldens at 1-SSIM <= 0.005, checks finite HDR
and white-furnace energy, verifies light edits retain all BLAS/TLAS identities,
checks a same-topology deformation rebuilds only one BLAS and retains its scene
ID, verifies an instance mask hides it, and resizes through odd FSR dimensions.

`Capture` uses 96 fixed-seed hybrid settle frames and a 4096-sample, 12-bounce
undenoised reference of the same camera. Linear RGB relative RMSE is RMSE divided
by RMS reference radiance, including visible background. Display 1-SSIM uses the
existing ara-render comparison. A separate 2048 -> 4096 linear error quantifies
finite-reference noise; the reference is not asserted to be fully converged.
The current residual is approximately 0.15% in the furnace and 1.5-1.9% indoors.

`captures/` contains same-view hybrid/reference/diff PNGs, the baked fallback,
12-frame moving-emitter/time-of-day/disocclusion sheets, and 1440p native/FSR
readbacks. Temporal tests compare continuous accumulation against a separate
history-reset view with matching probe evolution in the central viewport region.
Global radiance history intentionally rejects lighting edits; disocclusion retains
history elsewhere. This is a bounded diagnostic, not a perceptual ghosting oracle.
The four-stop light-step test measures the probe field itself over 24 frames,
against a 48-frame target, and requires 90% response within 12 frames.

`measurements/` contains canonical JSON: per-case reference/convergence errors,
sequence errors, probe step response, the baked probe field, device/driver identity,
and pass timing distributions. `gpu-timings-before.json` records the original
global-read denoiser; the current pass uses a cooperative shared-memory halo.

`Timings` uses 60 samples after 12 warmup frames at 2560x1440 and 1707x960 ->
2560x1440. The scene has seven instances, 4618 triangles, one point light and a
directional light, a 600-probe volume and a 16384 primary-probe ray budget. Post
includes auto exposure, bloom, fog/sky and grading; cinematic lens effects are
quality-gated off. GPU clocks are uncontrolled and validation is on. All queues
are serialized; sums are GPU work duration, not application frame latency.
Budget success on this diagnostic scene does not establish complex-content or
Unreal visual/performance parity. See `../ara-render-fidelity/README.md` for the
remaining native-refit, vendor SDK and production Raster/editor integration work.
