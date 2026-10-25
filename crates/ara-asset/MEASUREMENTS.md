# Import admission, 2026-10-25

## Stage profile before changing admission

The v4 Balanced real-project path is `ara hub create`, then `hub open --headless`,
ending when all 33 Showcase / 115 Outpost assets have published. Installed art and
kits are present; project/action/object/intermediate and profile pipeline caches
start empty. OS file pages and the driver's disk cache are not flushed.

The following are **summed concurrent elapsed spans**, not additive wall time.
LOD is nested in mesh; cache sync is nested in stores; initialization is nested
in compression. Process startup/input is spawn-to-import-entry, including IPC.

| Stage, seconds | Showcase | Outpost |
|---|---:|---:|
| Import wall | 11.484 | 34.441 |
| Worker startup and input | 1.312 | 4.290 |
| GPU device creation | 6.535 | 32.048 |
| glTF/model parse | 0.018 | 0.024 |
| Mesh, including LOD | 3.191 | 18.342 |
| LOD, nested | 2.488 | 11.681 |
| Texture decode and mips | 3.757 | 7.342 |
| Hashing | 0.570 | 1.249 |
| Cache stores, final and intermediate | 3.256 | 12.313 |
| Cache sync, nested | 2.033 | 7.550 |

Final-asset paths explain why scheduling matters. Showcase's FlightHelmet waited
5.452 s for admission, then cooked in 3.402 s. Outpost's Lonelyhood audio was
queued at 3.257 s, entered its worker at 29.095 s, cooked for 5.294 s, and published
at 34.441 s. Its snapshot/discovery span was 2.495 s, native startup/input 0.027 s,
final cache store 0.023 s, and cache-to-publication tail 0.025 s. These stages
overlap other assets. Metadata already has a reserved admission slot; plain FIFO
alone would not fix this audio tail (enqueue rank 105, launch rank 108).

## Paired admission experiment

Control: frozen `92e0b1a3` CLI/worker binaries, incorporating the v4 codec checkpoint
and batch 24. Treatment: identical source plus the CPU/encoded admission change.
The frozen player, art, Balanced preset, build profile and feature roots match.
Both binary sets live in the owned D: target; no main-target executable is used.

ABBA ran under one `ara-dev gpu` lock, with a fresh project and private engine
profile for every sample. No owned builds, shader cooks or synthetic CPU burners
ran during the timers. Other agents remained active; CPU snapshots before the
four phases were 30/30/37/36 percent. Two repetitions per arm establish an observed
paired result, not a contention-free benchmark or a statistical confidence bound.

| Import wall, seconds | Showcase | Outpost |
|---|---:|---:|
| A1 control | 17.356 | 39.268 |
| B1 fair admission | 12.992 | 35.403 |
| B2 fair admission | 14.509 | 37.576 |
| A2 control | 16.032 | 39.046 |
| Control mean | 16.694 | 39.157 |
| Treatment mean | 13.750 | 36.489 |
| Observed reduction | 17.6% | 6.8% |

The historical v4 11.484/34.441 s measurements remain separate. Repeated controls
are slower on this loaded session; subtracting the new treatment from an old
outlier or presenting the historical baseline as a contemporaneous control would
overstate the result. Showcase <10 s and Outpost well under 30 s remain unmet.

The targeted mechanism is clearer than overall wall time: Lonelyhood's admission
wait changed from **28.183 / 15.575 s** in A1/A2 to **1.422 / 0.370 s** in B1/B2.
Its process spawned at 30.461/20.033 s versus 3.419/3.459 s. Its actual cook stayed
5.572/5.858 versus 5.589/5.662 s. The final asset became a model in every run;
removing the audio tail does not remove device startup, mesh work or cache I/O.

All eight runs published every expected asset, used GPU compression and reported
zero backend fallbacks. Every final action key and every artifact name, kind,
length and content hash matches v4: 592 asset comparisons and 7,160 artifact
references. Scheduling does not enter import keys or result bytes.

The earlier v5 Showcase run took 60.173 s and included a **30.947 s cache.install
stall**. That I/O/off-CPU outlier is retained separately and excluded from this
ABBA. Its enclosing scope does not distinguish filesystem latency from scheduling
starvation. None of the eight ABBA runs had a cache.install span over 5 s. There
is still ordinary cache-store variability, which is retained in the raw results.

## Contract and verification

Native concurrency remains four by default. Queued metadata retains one slot.
When CPU-only and encoded cooks compete, each class retains one slot; empty lanes
reserve none. A single-core override retains serial admission. The short mutex
transaction never includes process spawn, importer work, I/O or a wait.

Admission remains non-FIFO: an unpolled pending request cannot block a later
polled request in the same class. Cancellation removes reservations. The existing
supervisor still holds a permit until confirmed child exit, independently of
consuming a completed reply. Importer versions, presets, wire requests and cache
identities are unchanged.

The real-child `worker_fairness` fixture fails against old admission with
"queued encoder stole the slot while a CPU cook was waiting". It passes after
the change, along with metadata priority, cancellation, a completely unpolled
completed reply and a completely unpolled pending request. Both it and the
single-slot unpolled-completion fixture pass 200/200 stress iterations (412.227 s
combined). Two synthetic CPU burners ran initially, then stopped when total CPU
was saturated; other builds remained active. Focused `ara-asset` tests (17 standard
tests plus three custom worker harnesses), strict all-target Clippy and fmt pass.
The native cold-batch harness observes peak four/cap four over 28 imports.

Canonical [`measurements/admission-v6.json`](measurements/admission-v6.json)
contains binary hashes, all eight results, per-asset admission/stage timings and
artifact checks. Raw traces, commands, source-ID maps and cache manifests remain
in the worktree's ignored `target/ara-dev/import-speed/v6-*` directories. The
frozen control and treatment are retained for the next device-reuse experiment;
completed project copies can be deleted after archiving their metadata.

The subsequent device-reuse matrix retains both frozen admission arms and
isolates local/pooled routing on one current binary. Its clean Outpost pair is
33.018 -> 26.425 s, with a 24.615 s pooled repeat; Showcase means are 13.260 ->
11.441 s. Repeated original/fair walls are close in this later matrix, so the
earlier admission percentages above are observations, not a universal speedup.
See [`ara-texture-gpu/MEASUREMENTS.md`](../ara-texture-gpu/MEASUREMENTS.md#leased-device-reuse-2026-10-26)
for all raw arm times, I/O outliers, remaining stages and lifecycle verification.
