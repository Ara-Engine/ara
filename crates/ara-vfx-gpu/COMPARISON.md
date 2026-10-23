# Particle performance evidence

The two Ara workloads update and submit 1,000,000 sprites at 2560×1440, with
0.003 m and 0.05 m diameters. Both use the same camera, 6×3.6×1.6 m distribution,
motion, material, 64-layer coverage ceiling, 30 warmup frames and 120 measurements.
All particles fit the benchmark camera and start above the ground. Every measured
frame asserts that all 1M remain alive and are submitted for drawing. The report
includes conservative projected overdraw, p50/p95 for every GPU stage, driver,
device, effect and shader identities. See `captures/timings.json`.

GPU stage medians omit queue/barrier/host overhead. They are not whole-frame FPS.
The benchmark isolates simple unlit sprites; the gallery's textured effects,
scene transport, probes and fidelity bloom have separate capture timings.

## Closest published measurements found

| System / source | Workload and hardware | Published measurements | Comparison limits |
|---|---|---|---|
| [Chen et al., ISPRS 2024, tables 3 and 6](https://isprs-archives.copernicus.org/articles/XLVIII-2-2024/49/2024/isprs-archives-XLVIII-2-2024-49-2024.pdf) | UE5 Niagara / LumaAI, 1,196,163 Gaussian splats; RTX 3090 24 GB | Average simulation 1.2 ms; alpha blending 2.2 ms; view sorting 0.25 ms; Niagara marker 0.28 ms; whole frame 16.3 ms | Anisotropic Gaussian terrain and destruction, rather than an ordinary sprite effect; particle footprint and viewport resolution are not specified alongside these timings. |
| [Unity VFX Graph first-person profiling report, 2019](https://discussions.unity.com/t/feedback-wanted-visual-effect-graph/718892?page=16) | VFX Graph 4.8 / Unity 2018.3.2f1 HDRP; GTX 980 Ti; VR 1789×1988; 22 effects; 1,159,863 average particles, 2,288,908 peak | Maximum GPU whole-frame time 33 ms in the reported 15-second interval | Older GPU, VR, multiple effects, and a maximum rather than per-pass medians. Sprite diameter is unspecified. |
| [Puget's Niagara Particle Scalability methodology](https://www.pugetsystems.com/pugetbench/creators/unreal-engine/) | About 750k active GPU particles; 148k/s spawn, 5-second life; 1920×1080 editor viewport; 30 seconds | Count and measurement method are published; this methodology page does not provide an isolated 4070 SUPER particle-stage time. | Useful workload reference, not a numeric speed comparison. |

Research checked on 2026-10-23. These are primary research, first-person profiler
data and the benchmark author's methodology. None supplies a matched 1M ordinary
sprite test on a 4070 SUPER with a documented 0.05 m footprint. Their numbers are
recorded for context; no Niagara/VFX Graph speedup ratio is justified. A direct
comparison needs the same camera, footprint/overdraw, spawn/update modules,
blending, sorting, material, resolution, visibility and timing boundaries.
