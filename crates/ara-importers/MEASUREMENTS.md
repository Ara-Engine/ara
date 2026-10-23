# Asset import measurements

Measured 2026-10-23 on Islam's Windows 11 PC: Ryzen 7 7800X3D, 32 GB RAM,
Rust 1.99.1 MSVC. CARGO_BUILD_JOBS=2 throughout, with other production tracks
building concurrently. These are **unoptimized debug-profile measurements**, not
release throughput claims. Native ISPC kernels are the crate's precompiled kernels.
No GPU was used. No development server or persistent service was started.

## Real scenes

| Fixture | Process cook | Validated cache hit | Cooked bytes | Artifacts |
|---|---:|---:|---:|---|
| DamagedHelmet GLB | 63,219.972 ms | 23.897 ms | 26,687,272 | 1 mesh, 5 textures, scene |
| Fox GLB (final UTF-8 name-table format) | 5,619.816 ms | 3.970 ms | 1,634,121 | 1 mesh, 1 texture, 1 skeleton, 3 clips, scene |

Cook time includes process spawn, transport, decode, mesh optimization, mipmaps,
compression and worker/host format validation; it excludes writing the cache.
Each scene was cooked twice in independent worker processes and compared byte for
byte. Cache-hit figures average ten warm filesystem loads, including object hash
verification, format validation and equality comparison of complete output.
DamagedHelmet contains five 2048² textures: about **0.332 million base texels/s**
including its mesh and scene work at this debug quality preset. Fox's 1024²
texture plus skinned geometry/clips totals about **0.187 million base texels/s**.

An earlier run measured DamagedHelmet 62,168.842 ms / 25.467 ms hit and Fox
4,463.067 ms / 4.396 ms hit; concurrency and debug execution materially affect
timings. The final skeleton name table changed Fox's byte count by three bytes;
its texture bytes/quality are unchanged. Release speed and peak memory have not
been measured. Keep expensive filtering/compression, cache reads and validation
off the editor frame; the API already schedules them in background jobs/workers.

## BC7 quality

Independent BC7 decoder: texture2ddecoder 0.1.2. PSNR is RGB, base level, 8-bit source
code values: `10 * log10(255² / mean((source-decoded)²))`. Alpha is excluded;
sRGB textures are compared in their encoded domain, not linear radiance. These
numbers therefore compare block-codec distortion, not perceptual image quality.

| Input | Size | RGB PSNR |
|---|---:|---:|
| DamagedHelmet image 0, color | 2048² | 48.732 dB |
| DamagedHelmet image 1, linear metallic/roughness | 2048² | 44.834 dB |
| DamagedHelmet image 2, emissive color | 2048² | 57.159 dB |
| Fox image 0, color | 1024² | 61.477 dB |
| Generated RGB gradient | 128×96 | 52.038 dB |

On the gradient, Intel BC7 basic and slow both reached 52.038 dB. Direct native
kernel time was 7.834 ms basic versus 30.194 ms slow; the complete slow texture
cook (including all mips) was 52.006 ms. A smooth gradient does not distinguish
their quality, so it cannot justify a claim that slow always improves images.
Slow remains the conservative fidelity default, with real image quality recorded
above. Additional generated 64² fixtures decoded through bcdec_rs 0.2.1 measured
**BC4 52.992 dB** (R UNORM, peak 1), **BC5 58.770 dB** (normalized normal XY UNORM,
peak 1), and **BC6H 43.128 dB** (linear RGB radiance, peak 8). BC6H is decoded to
float32, retaining values above one; it is not measured after clamping/tonemapping.
These are codec-specific synthetic checks, not a representative real HDR corpus.
Normal and sRGB tests independently decode mip values to check
normal renormalization and linear-light filtering. The renderer must reconstruct
BC5 normal Z and normalize after sampling.

## Reproduce

Download these Khronos sample assets into `crates/ara-importers/.fixtures/`
(gitignored; regular tests never access the network):

- [DamagedHelmet](https://raw.githubusercontent.com/KhronosGroup/glTF-Sample-Assets/main/Models/DamagedHelmet/glTF-Binary/DamagedHelmet.glb)
  SHA-256 `a1e3b04de97b11de564ce6e53b95f02954a297f0008183ac63a4f5974f6b32d8`.
- [Fox](https://raw.githubusercontent.com/KhronosGroup/glTF-Sample-Assets/main/Models/Fox/glTF-Binary/Fox.glb)
  SHA-256 `d97044e701822bac5a62696459b27d7b375aada5de8574ed4362edbba94771f7`.

```powershell
$env:CARGO_BUILD_JOBS = '2'
cargo test -p ara-import-worker --test process real_khronos_scenes_are_deterministic_and_cacheable -- --ignored --nocapture
cargo test -p ara-importers --test cooks bc7_quality_is_measured_by_an_independent_decoder -- --nocapture
cargo test -p ara-importers --test bcn_quality -- --nocapture
```

Optional `ARA_FIXTURE=Fox.glb` or `DamagedHelmet.glb` selects one scene. The full
test imports twice; expect minutes in debug. Downloaded models are not committed.
The tiny generated glTF/FBX/PNG/JPEG/HDR/EXR fixtures need no third-party files.

## Focused correctness gates

Regular suite: key coverage/canonical settings; cache misses/hits/immutability,
hash corruption and GC; sidecars through kernel transactions/undo/reference
repair; cyclic dependency queries and stale-edge removal; complete NPOT mips in
all BCn usages; JPEG/HDR/EXR and signed/alpha/extreme-HDR fallbacks; generated glTF
skins/clips/materials/hierarchy and FBX geometry; deterministic LOD/meshlet cooks;
invalid section ranges, indices, UTF-8 names, cycles and clip spans; native worker
abort/panic/hang/cancellation; last-good generations, supersession, approval/idle/
Play policy; native source watcher and external glTF buffer invalidation.

Run scoped cargo check, cargo test, cargo clippy --all-targets -- -D warnings and
cargo fmt --check for all four crates. ASTC container acceptance is covered; mobile
encoding itself remains the requested plugin seam. Renderer/GPU upload and
animation playback belong to their production tracks and were not exercised here.
