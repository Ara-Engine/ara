# Current Mac comparison lane

This new lane preserves `../bench.py`, `../MacBatch.cs`, the original S8 kernels and all earlier evidence. It derives a checkpointed wrapper and a post-timing ID export in a separate owned `target/perf-scale/us8` project. The marker is explicitly `s8-current-v1`, never a query fixture impersonation. Its preference checkpoints participate in the same registry as the six feature fixtures: unresolved state in either family blocks every owned Unity launch.

Status: 11 focused current-lane tests and the existing38 feature tests pass. Actual C# compilation and all4 paired S8 smoke gates pass after excluded first-import bootstrap/reprepare: W1/W2/W3 use1003 entities and W4 uses100000, each2warmup+3steps at15-runner ceiling. Full numeric hashes and independent exact IDs/components match, and durable preference restoration is proven. See `current_smoke_mac_2026-11-09.json`. All20 fresh strict timed pairs now pass their state/control gates, recorded separately in `current_timed_mac_2026-11-09.json`. Earlier50 feature pairs remain correctness evidence under their original per-record controllers; they are not timing samples for this lane.

W3 particles meets the target in both orders/budgets: Ara serial p50 is2.878–2.884ms versus Unity4.378–4.622ms, and parallel8192 is0.324–0.329ms versus0.435–0.439ms; p95 is lower in both orders. W4 also meets the fixture target with its bookkeeping difference disclosed below. W1 and the explicitW2 batch4096 configuration miss the target. W1 parallel p95 regresses12.6% in round2 despite a lower p50. W2 batch4096 is not the previously measured best128 configuration; a separate fresh128 comparison is required before describing bestW2 performance. The overall target remains unmet;8of20 pairs meet it, all fromW3/W4. Feature timings and additional tuning remain pending.

Use the installed arm64 Unity 6000.4.5f1 with Entities6.4.0, Collections6.4.0, Burst1.8.29 and Mathematics1.3.3. Both engines run at niceness10. This is Rust native versus Unity Editor Burst on the Mac CPU; Windows, GPU, player builds and Unreal remain unmeasured. Runtime/public Ara C# API support is separate from native fixture coverage.

Preparation launches no tools:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 benchmarks/iteration/unity/current/run.py prepare
```

First import may create `.meta`, package-lock or settings files. Its output stays unaccepted. Reprepare, then use a fresh bounded actual smoke (`--smoke --warmup 2 --frames 3`) before trusted timings. W4 retains its required100000 entities even for smoke. Do not retry a native crash automatically. Retain its log/checkpoint/PENDING; explicit `recover-unity --checkpoint <exact-known-path>` uses attempt-specific receipts and the short `us8-t` IPC folder, without a benchmark or guessed state.

Root supplies a successful current native build record with `exit_code`, command, compiler, environment, root lock, `executable_sha256`, executable/immutable binary and `source_sha256` for root Cargo files, `ecs.rs`, every support kernel and any additional compiled inputs. The runner checks every recorded source against current files, snapshots binary and build record by content, and checks sources again after execution. Native fixtures must report `stable_ids_asserted=true` after asserting exact IDs and required components for every workload.

Print strict plans without running them:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 benchmarks/iteration/unity/current/plan.py s8 --prefix current-01 --build-record target/perf-win-2026-11-09/native-build.json
PYTHONDONTWRITEBYTECODE=1 python3 benchmarks/iteration/unity/current/plan.py features --prefix feature-timed-01
```

The S8 plan covers W1 and W3 at1M, W2 at20k and50k, W4 at100k, each serial and15-runner ceiling; two fresh reversed rounds produce20 pairs/40 engine processes. W1/W2/W4 use120 warmup frames; W3 uses3000; each has300 samples. W3 uses the previously measured8192 batch setting; other Ara cases use4096, with each config retained in evidence. The feature plan uses100k/120warmup/300samples and all six cases, with explicit representative variants. `--all-variants` expands to every unique existing config (five/six-step resize smoke configs collapse to one420-step timed config). Print plans first and execute commands sequentially only in the granted CPU slot; no hidden build or engine download occurs.

Each pair requires exact numeric state hash equality and independent post-timing ID/component assertions. Unity exports all12 floats per logical slot plus exact signed IDs. Ara currently emits a full numeric BLAKE3, rather than numeric bytes: equality is a complete cryptographic hash comparison, not a bytewise dump claim. W4 epochs are checked against the latest group replacement frame independently on each side. The new Unity export adapter leaves the original measured loop and numerical kernels unchanged. W4 includes Unity's100k slot reconstruction versus Ara's direct slot updates; report that bookkeeping difference with any result. Serial Unity Schedule+Complete versus direct Bevy loops also includes different dispatch costs.

Actual worker ceilings are recorded separately: W1–W3 parallel Ara14 Bevy pool workers plus eligible caller versus Unity14 workers plus completing caller. W4 uses15 Rayon workers with caller blocked in the pool, versus Unity14 plus caller. Feature retained-Schedule cases use their separately probed15-worker/no-caller configuration; direct cases use14pluscaller. These are potential ceilings, not a claim every worker ran concurrently on every frame.

Prepared copies, current templates, configs, results, dump/ID bytes, environment and checkpoint/receipt are hash-bound in launch evidence. The runner checks resources before/after and samples competing tools every2s during each engine process. A contention/power-source change retains raw data but rejects timing. Sampling is not an exhaustive OS scheduling trace. Global Burst compilation is enabled, safety and force-safety/debug are explicitly false, and all captured options/five preference keys are restored with durable proof. No quality/safety flags are weakened to reach the requested target.

The target is Ara p50 at least20% below Unity with no more than5% p95 regression in the same valid pair. A pair that misses it reports the loss. Two rounds must be reviewed separately; no claim of an all-engine win follows from a subset, a smoke, or an alternative workload/model.
