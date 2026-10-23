# ara-pathtrace

Offline/editor reference controller and M3 verification scenes.
Transport itself lives beside hybrid transport in ara-render-fidelity, so
material evaluation, alpha rejection, light units and scene IDs cannot drift.
The reference never uses DDGI, spatial filters, temporal clipping or upscaling.

## API
- `Reference`: progressive finite sample/bounce budget with explicit reset.
- `scenes`: furnace, thin wall, moving emissive, disocclusion, glossy, alpha foliage.
- `metrics`: linear radiance RMSE and temporal residual metrics.

## Verification
Set CARGO_BUILD_JOBS=2. `./run.ps1 -Mode Check` runs local tests/clippy/fmt.
`./run.ps1 -Mode Capture` records all stills, reference errors and temporal checks.
`./run.ps1 -Mode Timings` measures native 1440p and 960p -> 1440p FSR1 separately.
`-Mode Quality` enforces linear RMSE limits against a 4096-spp independent reference.
`fetch-sponza.ps1` fetches pinned local content; `-Mode Content` records its honest
per-pass timing distributions separately from the diagnostic budget gate.
Use `-Mode Smoke` for a bounded 160x90 hardware/validation integration check.
`fetch-beauty.ps1` obtains pinned CC0 PBR assets; licenses live in THIRD_PARTY_NOTICES.md.
`-Mode Beauty` is the main authored-content golden; `-Mode Many-Lights` traces
the 1000-light corridor independently. Matching *-Timings modes measure full frames.
`-Mode Night-Noise` records 16x16 block and per-pixel luminance deltas and a strip.
Night references use 32768 spp; the 1000-light reference uses 65536 spp and 32 RIS
candidates per vertex. Report lower-half residuals as well as whole-image error.
Timing reports retain chronological frame samples, median, p95, maximum and load.
Goldens update only with `ARA_UPDATE_FIDELITY_GOLDENS=1`; regular tests compare.
All output/provenance goes under this crate; root files/docs remain untouched.
Runtime shader cooking is forbidden. Recook fidelity offline before testing edits.
Never substitute denoised reference images for independently converged radiance.
