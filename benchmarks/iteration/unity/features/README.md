# Real ECS feature fixtures

Status: `query_filter` passed real serial and parallel correctness smokes on
2026-11-09 with Bevy 0.20.0, Unity 6000.4.5f1 and Burst 1.8.29. Both engines matched
the independent 1,003-ID/five-step oracle across all 36,132 state bytes.
[Versions, hashes and validation evidence](query_filter_smoke_mac_2026-11-09.json)
retain `timings_accepted=false`; those smokes establish correctness only.
The other five cases passed all 50 planned serial/parallel correctness pairs on
the same installed versions, with exact full-state/oracle agreement. Their
[separate evidence](extended_smoke_mac_2026-11-09.json) preserves 22 earlier
controllers and 28 resumed checkpointed controllers, with one unchanged runtime
kernel source version and Bevy executable. These are correctness results;
`timings_accepted=false`. The [extended notes](extended/README.md) describe real
API coverage, storage checks, worker semantics and the recovery gate. They do not
establish Ara public C# buffer/JobHandle availability or shipping performance.

Separate [phase A timing evidence](feature_timed_phase_a_mac_2026-11-09.json)
now covers the first 12 of the planned 36 representative pairs: 100,000 entities,
120 warmups and 300 samples, with complete independent 420-step scalar references.
All state/control gates pass; six pairs meet the target of at least20% lower p50
and at most5% p95 regression. This is the first engine order only; remaining24
pairs include resize, 64-word buffers and reversed-order confirmation. The
initial query helper `.meta` import attempt is retained and excluded; both
families then passed fresh actual preflight smokes before all12 fresh pairs.

Phase B stopped at its first64-word-buffer Unity invocation after a native Burst
compiler crash. No Phase B pair was accepted. The durable checkpoint was saved
before mutation; explicit recovery restored the exact captured five options and
five preference keys, then cleared the matching guard after Editor exit.
[Failure/recovery evidence](feature_timed_phase_b_recovery_mac_2026-11-09.json)
preserves the failed attempt, source-bound control stages, full29.2MB independent
oracle, recovery receipt and completed small serial/partitioned oracle checks.
The first12 timings remain valid; the remaining24 await a separately authorized
fresh attempt. No benchmark was retried automatically and no quality flags,
licenses or caches were changed to bypass the crash.

| Case | Serial Bevy / Unity p50 (ms) | Parallel Bevy / Unity p50 (ms) | Target met, serial / parallel |
|---|---|---|---|
| Query,8 archetypes | 0.00625 / 0.0087 | 0.01133 / 0.0180 | Yes / Yes |
| Query,64 archetypes | 0.006625 / 0.0082 | 0.01133 / 0.0201 | No / Yes |
| Dirty,1% spread | 0.03621 / 0.1165 | 0.06808 / 0.0540 | Yes / No |
| Churn,10% | 0.2724 / 5.1572 | 0.2838 / 5.8661 | Yes / Yes |
| DAG,fork/join | 0.09004 / 0.0927 | 0.11121 / 0.0918 | No / No |
| Fixed buffer,4 words | 0.5146 / 0.4598 | 0.1223 / 0.0896 | No / No |

The failing rows remain open. Parallel dirty/DAG/buffers are26.1%/21.1%/36.5%
slower at p50, with9.5%/23.1%/14.2% p95 regressions. Serial buffers are11.9%
slower with14.0% worse p95; serial64-archetype query is19.2% lower at p50 and
serial DAG only2.9% lower. No fixture kernel or scheduling policy was changed to
hide these results. Native fixture coverage does not establish the same public
Ara C# API or performance.

Both engines create the same 8/64 tag archetypes, group IDs by archetype, and retain
their hot query. Unity uses `EntityQueryBuilder.WithAll/WithAllRW/WithNone`, a
Burst `IJobEntity`, explicit scheduling and completion. Bevy uses a retained
`QueryState` with `With<TagA>/Without<TagB>` and safe contiguous queries. Change
ticks are advanced, selected payloads/update counters are stored in components,
and initialization/export are outside the timed region. No raw-array surrogate
replaces either engine's query execution.

For one runner, Unity still calls `Schedule + Complete` with zero job workers;
Bevy executes its retained contiguous query directly on the caller. Their
scheduler overhead differs and remains timed. The serial row measures these
public execution paths, not isolated identical kernel invocation overhead.

The full binary export includes exact unique IDs, actual tag presence, values and
per-entity update counts. Both exporters audit IDs/tags/counts. The independent
Python oracle supplies expected bytes for smoke cases; the verifier compares all
bytes and checks inactive fields and header values. This closes the legacy S8
dump's omitted-ID gap for this new fixture.

Preparation creates a separate project under `target/perf-scale/uf`. Its control
wrapper is derived from the reviewed S8 wrapper by replacing exactly one call and
the CLI flag; the original source is unchanged. Template/derived/source hashes
are recorded. Every launch validates both current repository templates and the
complete prepared `Assets`, `Packages` and `ProjectSettings` file inventories,
including assembly definitions, metadata, manifest and resolved package lock.
An edit, removal or extra file requires preparation again. The exact input
manifest, canonical manifest hash and raw marker hash are embedded in launch
evidence and checked again after execution. Burst flags/preferences retain the same checked restoration. Unity
must report the requested runner budget; actual package versions remain in its
environment file. The configured defaults are installed Editor 6000.4.5f1,
Entities/Collections 6.4.0 and Burst 1.8.29. These are not shipping-player results.

Inspect preparation and exact commands without launching either engine:

```sh
PYTHONDONTWRITEBYTECODE=1 /usr/bin/python3 benchmarks/iteration/unity/features/run.py prepare
PYTHONDONTWRITEBYTECODE=1 /usr/bin/python3 benchmarks/iteration/unity/features/run.py build-command
PYTHONDONTWRITEBYTECODE=1 /usr/bin/python3 benchmarks/iteration/unity/features/run.py command --engine unity --smoke --count 1003 --archetypes 64 --warmup 2 --frames 3
```

Only in a granted compilation slot, build the standalone Rust fixture:

```sh
PYTHONDONTWRITEBYTECODE=1 /usr/bin/python3 benchmarks/iteration/unity/features/run.py build-bevy
```

The printed `build-command` is the exact Cargo command: release, two build jobs,
`target-cpu=native`, a private target directory, thin LTO and one codegen unit.
The explicit build action records source hashes, compiler/tool paths, lockfile and
executable hash, then saves an immutable executable copy by content hash.
Subsequent runs verify and use that copy so another Cargo build cannot silently
replace the measured binary. A changed fixture requires rebuilding; failed builds
do not replace the last successful provenance record.
Builds record competing tool/resource context without requiring an idle CPU;
their wall durations are provenance and are never accepted as benchmark timings.

Dependencies use Bevy 0.20 and the existing canonical Ara JSON writer for
post-measurement output; no root manifest or production engine API is modified.
The standalone crate has its own [instructions](rust/AGENTS.md).
The checked-in lockfile aligns all present Bevy 0.20-family entries to the
production root's 0.20.0 versions; manifests retain caret ranges. Preserve that
alignment when updating dependencies and record the actual graph.

In the next granted execution slot, use a partial-count smoke that exercises all
64 archetypes and both matching/nonmatching IDs:

```sh
PYTHONDONTWRITEBYTECODE=1 /usr/bin/python3 benchmarks/iteration/unity/features/reference.py \
  --case query_filter --entities 1003 --steps 5 --archetypes 64 \
  --output target/perf-scale/uf/results/query-smoke-01/reference.bin
PYTHONDONTWRITEBYTECODE=1 /usr/bin/python3 benchmarks/iteration/unity/features/run.py run-bevy \
  --smoke --count 1003 --archetypes 64 --runners 1 --warmup 2 --frames 3 --run query-smoke-01
PYTHONDONTWRITEBYTECODE=1 /usr/bin/python3 benchmarks/iteration/unity/features/run.py run-unity \
  --smoke --count 1003 --archetypes 64 --runners 1 --warmup 2 --frames 3 --run query-smoke-01
PYTHONDONTWRITEBYTECODE=1 /usr/bin/python3 benchmarks/iteration/unity/features/run.py verify \
  --smoke --count 1003 --archetypes 64 --runners 1 --warmup 2 --frames 3 --run query-smoke-01 \
  --reference target/perf-scale/uf/results/query-smoke-01/reference.bin
```

Repeat with `--runners 15 --run query-smoke-parallel-01` and the same oracle.
Bevy uses 14 pool workers plus the scope caller; Unity uses 14 job workers plus
the completing caller. The serial fixture uses no worker pool in Bevy and zero
Unity job workers. Actual simultaneous worker activity is not inferred from the
configured ceiling. A later matrix must reverse process/case order and preserve
power profiles separately. Both launchers use niceness 10 and resource guards.

`--smoke` is a correctness-only lane capped at 10,000 entities and 10 total
warmup/measured steps. It records busy tools and power context while preserving
all source/input, Burst, restoration and complete-state checks. It requires the
independent scalar reference. Its separate `_smoke` artifacts and launch evidence
always say `timings_accepted=false`; summaries omit percentiles, and successful
verification reports `quality_passed=true` with `timing_comparable=false`, even
when the machine happened to be quiet. Raw engine samples are diagnostics only.
The default lane without `--smoke` keeps strict busy-tool and power guards.
Smoke evidence cannot be relabeled as timed evidence by the verifier.

Installed API source checks are in `CONTRACT.md`. The first real compiler pass
confirmed that generated `IJobEntity` code requires an explicit Unity.Mathematics
assembly reference, now included. Runtime layout/export, source generation,
controlled Burst flags and preference restoration passed the focused smoke above;
new changes still require those checks.

On a new project, first import generates `.meta`, settings and package-lock files.
Reserve an initial Unity smoke under `--run query-bootstrap-01` for that setup.
If those files are created, the launcher saves `.unaccepted.json` and fails closed;
its samples are not accepted as measurements. Run `prepare` again to capture the
resolved input inventory, then execute the actual paired smoke under fresh run
names above. Preparation preserves valid importer metadata and Editor revision
information, updates changed templates, and removes stale owned source copies.
Any later prepared-input drift is rejected; it is not silently reclassified as
bootstrap. Verification compares the two embedded manifests as well as state.

Source-integrity regression tests require no engine or compiler:

```sh
PYTHONDONTWRITEBYTECODE=1 /usr/bin/python3 -m unittest discover \
  -s benchmarks/iteration/unity/features/tests -v
```
