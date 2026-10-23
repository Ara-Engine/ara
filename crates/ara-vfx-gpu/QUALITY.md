# Capture review

Reviewed the six 960x540 mid-life captures and all four frames in each sequence
strip on the RTX 4070 SUPER. These are useful procedural starter effects, but
they do **not yet match Niagara showcase sample quality**.

| Effect | Verified behavior | Remaining art/rendering limits |
|---|---|---|
| Campfire | Blended turbulent fire sheet, embers, density-shaded smoke, HDR bloom, flickering light on ground and bronze prop | White core obscures individual tongues; simple hearth geometry and repeated smoke structure |
| Sparks | Hot-to-cool color, velocity-aligned shutter streaks, plane collisions | Sparse uniform streak population; needs more authored breakup and impact variation |
| Magic swirl | Four coherent orbit streams, curl perturbation, cyan/magenta wisps, glowing ground ring | Repeated arc silhouettes; lacks layered secondary trails and richer temporal choreography |
| Smoke | 3D-density-baked flipbook, blended frames, probe/sun lighting and opacity shading | Visible billboard layering; no inter-particle volumetric light extinction or shadows |
| Muzzle flash | Short textured jet, HDR bloom, decaying light visibly illuminating ground and bronze | Some scene shadow grain near the barrel; simple block prop and limited flash silhouette variation |
| Rain | Stretched falling drops, GPU collision-to-splash events, blended ground ripples on a wet material | Ring population is regular; no water displacement or authored splash crown |

`captures/<effect>.png` is the representative frame;
`captures/<effect>.sequence.png` is the four-frame strip. Exact tick values are in
the adjacent sequence JSON; `captures/sequences/` retains every full-size frame.
The test independently repeats the mid-life frame byte-for-byte and checks all
twelve committed still/strip goldens. No captured pixels are painted or retouched.

The gallery uses fidelity scene transport, probes, shadows and the actual shared
post stack. The texture cooker is original deterministic Rust noise/density code,
with no external media or build-time texture generator. The million-particle
benchmark intentionally uses simple sprites, not these richer gallery materials.
After merging main through `41cbc25`, scene shadow grain was substantially reduced;
all twelve rebuilt captures were inspected again before refreshing baselines.
