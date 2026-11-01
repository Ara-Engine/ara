# Render-v3 stress lab

`ara-dev lab gate` runs an explicitly selected, prebuilt headless worker. It does
not start/restart a lab daemon, change a renderer default, or build another checkout.
`cases.v1.json` is the versioned scene recipe catalogue; each run writes its exact
60 Hz frame script to `sequence.json`. Both runs consume the same case definition
and A3's `ARACHG01` binary stream (`changes.bin`). The 57 recipes include all five
R5-5 contact heights, the pole sweep and eight separate shadowed sources.

Build the worker and lab in your own target directory, after checking the overnight
quiet-window lock. The task coordinator's builder rules govern admission.

```powershell
cargo build -p ara-dev
cargo build -p ara-render-fidelity --example render_v3
.\target\debug\ara-dev.exe lab gate --list
.\target\debug\ara-dev.exe lab gate --gate G1 --runner .\target\debug\examples\render_v3.exe --width 2560 --height 1440
.\target\debug\ara-dev.exe lab gate --gate all --report-only
```

The default output is `D:/AraBuild/render-v3/gates`. Each `G<n>` contains
`RESULT.md`, `RESULT.json`, and captures (at most 1920x1080). Metrics use the full
native images, including when the report PNG is reduced. Raw per-frame runs and
logs live in `cases/<case>/<uncapped|capped>/`. `--output` makes campaigns independent.
Use 1920x1080 on the 760M and 2560x1440 on the 4070 SUPER. Other sizes retain evidence
but do not pass a native-resolution gate. `--cases` selects work, not a smaller gate:
reports still enumerate every required case and leave unrun rows unavailable.

`--timing-only` measures the complete case list in one quiet window. The lab takes
an atomic overnight lock for at most 44 minutes, enters ara-dev's machine GPU queue
and foreign-load guard, and releases the overnight lock before references start.
It enforces a 20-minute gap between T1 windows. `--quality-only` performs captures
and references without taking a quiet window; it respects someone else's window.
Worker timeouts terminate only the owned process tree. Old receipts are retained
as `previous-*`; a failed attempt cannot reuse an earlier run as its new result.

`--reference-samples 256` is a finite undenoised budget, not a convergence claim.
Every still has a reference; motion cases sample pre-event, F, F+1 through F+4, and
adjacent late frames. `--all-references` scores every capped frame. D29 is the maximum
16x16 block RMS of `(test[t]-test[t-1]) - (reference[t]-reference[t-1])`, in linear
luminance normalized by the recorded radiometric white. Partial edge blocks use their
actual pixel count; gaps between sampled references never become adjacent samples.
The existing `lab reference` Cycles workflow remains available for supported exported
scenes. Procedural fixtures use ara-pathtrace's native reference view with the exact
same geometry, material extensions and finite light ranges. No shader changes are
needed: its local-light sampler already applies the renderer's range window.
Reference PNGs and provenance are archived beneath
`ARA_LAB_REFERENCES/render-v3/<worker-sha256>/`, defaulting to
`D:/AraBench/report/images/render-v3/`. `revision` identifies the worker executable
by SHA-256, so a later source commit cannot silently relabel an existing binary.
The authored range sweep scales power by range squared; dense cases raise EV100
by log2(LPP/2.4) to keep the receiver in the measurable exposure range. Both runs
and references use the identical radiometric white recorded in the receipt.
Receiver coverage uses the actual captured entity-ID domain. Rare raster edge
pixels whose analytic centre ray misses stay in that domain: `lpp_lower` and
`lpp_upper` bound their contribution, `lpp_unresolved_edge_pixels` counts them,
and scalar `lpp` stays null. Gates use the conservative bound on every frame.

## Worker and instrumentation contract

The standalone worker accepts:

```
render_v3 CASE.json uncapped|capped OUTPUT WIDTH HEIGHT SAMPLES [all-references]
```

`contract.rs` is shared source compiled by the lightweight lab and the worker. It
uses only serde and the standard library, so ordinary ara-dev commands do not link
the engine. `RunResult` is the version-1 wire format. Stage names are stable keys in
`FrameResult.stages`. Each `StageCounters` exposes `gpu_ms`, `cap_ms`, `demand`,
`debt`, `work_unit`, `oldest_debt_frames`, edge lag in page texels (all/near),
`deadline_misses`, and `resident_bytes`. Unavailable fields serialize as null and
render as n/a. Rows are directly readable by diagnostic/ara-why style consumers.
No diagnostic codes have been introduced by this tool.

Wave-2 stages must fill these complete-path rows, including preparation, discovery,
maintenance, selection, marking and cold publication:

| Stage | Required evidence |
|---|---|
| `local_lights` | Build + prep + select + shade GPU ms, including omission work; actual LPP/receiver coverage, E3 leakage/pop/highlight/shadow retention, conservative omitted-energy bound |
| `local_pages` | Uncapped GPU demand, runtime cap, queued work/debt, per-frame edge lag in that light's page texels |
| `sun_pages`, `dynamic_pages` | Uncapped GPU demand, average and max; capped lag and half-texel near-ring lag, true-angle image and secondary-view measurements |
| `link_repair`, `event_solve` | GPU ms and persistent debt age, three-frame deadlines; demand must not reset at a new event |
| `event_total` | Discovery + forcing + repair + solve, measured together; do not charge readout or steady patch work here |
| `bounce_readout`, `patch_sources` | Separate readout/steady source work; retained for attribution |
| `transport_memory` | Peak resident bytes across links, reverse links, index and channels |

Quality fields additionally require exact-solution max error (not a residual),
region-FLIP excess over the exact control, per-frame D29 blocks, penumbra width,
contact displacement, missing >=2 px shadow features, and secondary-view coverage.
`evidence` carries index bytes/link, links+reverse+index bytes, channel bytes,
direction-comparison completion, current-geometry transport, per-source-sample
combine, real-time sample-count declaration, and G5 constituent quality receipts.
G5 requires a complete-frame GPU span, CPU critical-path time and submission stalls;
summing pass timestamps never creates those measurements. G5's failed-workload
allocation charges are attribution only and cannot produce a passing verdict.

The default worker is today's native raster High, with hardware RT disabled for the
control. Its uncapped mode removes exposed local/VSM redraw quotas but cannot remove
all internal limits. It explicitly reports `uncapped_supported: false`. Raw GPU pass
times are useful A/B control observations, not certified uncapped-demand numbers.
Today's renderer has no transport debt or exact error counters: they remain n/a.
Startup probe surfaces are acquired offline with the existing fidelity baker and
relit during replay. This worker does not connect the Play host's asynchronous CPU
surface reacquisition: geometry and material edits retain their startup surfaces.
It is therefore a renderer-core control, not a complete Play-host/G3 control. No
discovery/repair correctness or cost is inferred from it. Offline acquisition uses
hardware RT before measurement; scored raster frames never use hardware RT.
The control table reports the measured GPU span across update/render submissions,
including submission gaps. Repeated pass labels are summed instead of overwritten.
Finite references remain `reference_converged: false`; image and temporal numbers
remain observations, not pass/kill decisions, until convergence is established.
A measured cost can still kill while quality evidence is unavailable. A complete
gate needs both GPU targets. G1 uses its E3/excess lines (no invented absolute FLIP
line); G4's 0.05 kill applies to overlap, hand and stained glass, as specified.

Polar loads the actual exported model and existing cooked compressed textures from
`ARA_LAB_POLAR` (default `D:/AraScratch/AraPolar`) and a pinned cache action selected
by `ARA_LAB_POLAR_ACTION`. The model buffer is SHA-256 pinned; each cooked artifact
is checked against its BLAKE3 content key. Node transforms and model-space geometry
are retained. This is a controlled finite-range light rig, not a reproduction of
the editor project's fog, sky and post-processing. No project files are written.
Foliage consists of deterministic moving leaf cards; body motion is a scripted
stress trajectory rather than a physics simulation. The sealed flicker case has
200 closed cells and observes one cell from inside; all 200 sources change.

`inspect.ps1 -Root <campaign> -Output <directory>` creates labelled contact sheets
from every raw capture/reference pair. It records no automatic visual approval;
the operator must open and inspect the sheets and any suspicious native PNGs.

## Focused verification

```powershell
cargo clippy -p ara-dev -p ara-render-fidelity -p ara-pathtrace --all-targets -- -D warnings
cargo test -p ara-dev lab::stress
cargo test -p ara-render-fidelity --example render_v3
```

The tests exercise percentile/average/peak boundaries, missing evidence, catalogue
coverage, real mesh triangle counts/winding, valid native scenes across event poses,
receiver intersections and reference-subtracted temporal RMS. They create no captures
and never write tracked files or the real user profile.
