# Diagnostic measurements

These are earlier runs, retained to explain the performance and verification
fixes. Use `../1440` and `../2560` for the final capture matrix and timings.

- `incorrect-window-extent-*`: SDL maximized a requested 1440 window to
  2560x1369. These samples cannot support a 1440 timing claim. Verification now
  sets and checks the exact physical window extent before constructing the UI.
- `1440-batching.json` and `2560-batching.json`: correct physical extents after
  grouping each card's shapes before its labels. The HiDPI run failed its p95
  budget. Its warmup accepted a last-good material preview while the current
  500-node shader was still compiling. The final route waits for the current
  preview and records every timing stage as well as the total.

No outliers were removed from these or the final measurements.

The `*-before-compact.json` runs passed at 3.1002 / 3.5037 ms p95 before the
short-drawer caption fix. `1440-compact-first-run.json` retains a 4.0371 ms p95
miss: UI compile/upload/submit p95 was 1.0721 ms, while serialized FIFO present
waiting rose to 2.8255 ms. Presentation and scheduling variability remain in all
samples; the editor does not subtract those waits from its input-to-present gate.

`1440-reflow-contended.json` preserves the next run at 11.2370 ms p95, with
22.5733 ms maximum. The immediate post-run sample showed 50% total CPU load and
eight rustc processes on this 16-logical-CPU host. Later observations ranged from
9% to 76% total CPU with 7–12 compiler processes. These observations are recorded
in `../load-observations.json`; they were not synchronized with individual frame
samples and must not be treated as per-frame attribution.

All results are **measured on a shared, loaded host**. Other agents and the
parent's workspace tests were not paused. The parent explicitly requested that
verification finish without waiting for an exclusive benchmark window; a quiet
machine sweep will follow integration. The already-started final matrix completed
at 3.1629 / 2.8820 ms p95. The complete history, including misses, remains here.
