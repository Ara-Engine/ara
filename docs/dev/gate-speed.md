# Rolling integration gates

Run `ara-dev check --json` on main after a committed merge batch. The default
`--base auto` selects changes since this checkout's last fully green check.
Other branches retain `main` as their default; any branch can request `auto`.
Explicit `--base REF` keeps merge-base selection. Nightlies use `--full`.

`target/ara-dev/last-green.json` records HEAD, the selected base, full/affected
mode, checkout identity, completion time and the saved `run-*/report.json`.
Only passing checks without observed known failures, beginning and ending clean
at the same HEAD, advance it. Plans and uncommitted merges never do. A failed
batch leaves the previous green receipt intact, so the next batch includes it.
Receipts publish by atomic rename under a short file lock. They are checkout-local,
even with an independent or shared Cargo output directory.

Missing, malformed, copied, rewritten or divergent receipts require a full
bootstrap. This deliberately avoids an empty `main..HEAD` pass on main. On first
deployment, the integrator can run the next batch with `--base <known-last-green>`;
its passing run records the receipt without repeating unrelated suites. Keep the
receipt when clearing incremental/build caches. `--plan --json` reports the
effective base, reason, full mode and selected packages without advancing it.

## Compilation and test policy

- Gate Cargo commands default to `CARGO_INCREMENTAL=0`, retaining normal Cargo
  fingerprints/artifacts. An explicit `CARGO_INCREMENTAL` or legacy
  `CARGO_BUILD_INCREMENTAL` overrides it. Focused `ara-dev cargo` and performance
  profiles keep their existing incremental policy. No existing directories are deleted.
- Gates detect optional sccache on PATH. Explicit `RUSTC_WRAPPER` takes precedence;
  `--no-compiler-cache` disables it. No tool is installed or required. The shared
  Cargo-intermediate lane keeps its exclusive fingerprint cache and bypasses the
  second jobserver. No server is stopped, reset, or moved to another cache directory.
- sccache 0.18 rejects `CARGO_INCREMENTAL=1` outright; that override uses direct
  Rust instead of automatic/configured sccache. Custom explicit wrappers survive.
  Mid-compile server I/O errors allow sccache's direct compiler fallback; startup
  failures can still require `--no-compiler-cache` and remain visible.
- Cargo job counts remain explicit or default to two, including `run.ps1`.
  Test workers default to at most four, half the CPU count, and one per 512 MiB of
  admitted RAM. Admission retains the 4096 MiB/half-free-RAM cap and suite weights;
  GPU, .NET and process groups each stay serialized. `ARA_TEST_JOBS` overrides the
  worker count. The existing one-time .NET prebuild is unchanged.

## Evidence and expected savings, 2026-10-27

Read-only extraction from the integrator's existing `D:/AraBuild/gate-b5*.log`:

| Run | Compile/test packages | Clippy | Test build | Harnesses | Total |
|---|---:|---:|---:|---:|---:|
| b50, failed | 104 / 104 | 117.1 s | 1840.1 s | 540.5 s | 2569.0 s |
| b51, green retry | 104 / 104 | 34.2 s | 227.8 s | 378.7 s | 683.7 s |
| b52, failed | 104 / 104 | 123.8 s | 1182.4 s | 314.3 s | 1666.8 s |
| b52b, green retry | 104 / 104 | 11.4 s | 64.3 s | 307.1 s | 425.3 s |
| b53, green | 61 / 23 | 173.0 s | 1017.6 s | 260.2 s | 1469.8 s |
| b54, failed | 104 / 104 | 116.2 s | 1262.7 s | 366.1 s | 1807.5 s |
| b55, green retry | 104 / 104 | 51.9 s | 210.3 s | 294.4 s | 598.4 s |
| b56 smoke, green | 1 / 1 | 93.5 s | 71.2 s | 4.5 s | 175.1 s |

These are historical workloads/retries, **not** a controlled before/after. The
largest expected gain is avoiding already validated work: a release-tool-only
batch should remain a minutes-scale one-package gate, rather than repeating the
104-package test build/execution. Broad foundational changes still select their
consumers and can remain expensive. Four workers can halve only parallelizable
harness time; serialized suites and compilation do not get that gain.

The installed machine-local ancestor Cargo config sets sccache as the wrapper;
the former documented `--no-compiler-cache` command bypassed it intentionally
after an older 20.70 s versus 1.02 s small-library probe. Current read-only stats
show 0 Rust hits / 47 misses and 14 incremental exclusions. Its running cache is
on C: (15 GiB); the old D: location in earlier notes is historical. Removing the
incremental exclusion makes library misses cacheable, but does not make final
test executable links cacheable or normalize cross-worktree source identities.
No cache speedup or new hit rate is claimed before a controlled gate comparison.

A permitted small **ara-dev-only** test-profile probe used two Cargo jobs, warm
dependencies, direct rustc (CLI wrapper override), and one unique root cfg:

| Root compilation | Incremental off | Incremental on |
|---|---:|---:|
| First build of that root configuration | 16.800 s | 18.006 s |
| Same content, source mtime changed | 16.608 s | 1.956 s |

Incremental state added 150,195,918 bytes (~143 MiB) in the owned probe target.
The first incremental build was about 7% slower here; a retained warm cache was
much faster. This supports a non-incremental gate default when the reported
~20 GiB workspace state is repeatedly deleted, **not** disabling incremental
for focused edit loops. It does not measure a workspace cold dependency build.
An initial probe with the inherited sccache wrapper rejected incremental mode;
that failed attempt is retained separately, excluded from the table.

Evidence: this worktree's `target/ara-dev/gate-speed/` (unit/Clippy/fmt logs,
`incremental-proxy.json` and per-case logs). No integration gate, GPU work,
engine build or .NET prebuild was run for this checkpoint. Self-building CLI
integration suites are deferred with the integration gate under the owner's
quiet-machine restriction; unit tests and all-target Clippy cover this checkpoint.

After the owner releases the machine, compare the same committed batch/base on
the old and new tools with matching jobs, output/cache warmth and retained logs.
Separate the incremental 0/1 comparison from automatic-base selection, and
record the effective base, package/harness counts, phase times, free space and
cache counters. The first mode switch may rebuild Cargo fingerprints; retain
that cost rather than crediting a later warm retry as a cold speedup.
