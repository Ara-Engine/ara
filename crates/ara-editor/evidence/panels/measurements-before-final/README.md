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
