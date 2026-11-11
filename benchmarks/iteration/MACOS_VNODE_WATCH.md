# macOS file-event delivery (Mac, 2026-11-11)

`FileSystemWatcher` on macOS rides on FSEvents, which delivered an in-place save 11.4 ms (median, 60 saves, min 1.8, p95 12.2) after the write: a third of an L1 save → visible, and a fixed cost of every level. A kqueue vnode watch on the file delivers in 0.12 ms (median, p95 0.32). `VnodeEvents` keeps one `O_EVTONLY` descriptor per source file (up to 1024; skipped: `bin`, `obj`, hidden folders) and reports in-place writes (truncate and write, as most editors save) at once; the directory watcher stays for everything else. Rules that keep it correct:

- A delete, rename or revoke only drops the file's watch. A replaced file (an atomic save) is reported by the directory watcher, as before, so a half-replaced file is never compiled early; the replacement is watched again from that event.
- The directory watcher's echo of an in-place write (`Changed`, and `Created` which FSEvents adds) is dropped while the file's vnode watch is alive and a vnode write was seen within 100 ms. After a replacement the watch is gone, so its `Created` passes. Deleted and Renamed events are never dropped.
- Failure to create the queue, to open a file or a full descriptor budget falls back to the directory watcher alone.
- Windows and Linux are unchanged (the class is only created on macOS).

20 fresh processes per arm, mirrored order, 45 edits per process (300 per level per arm). Median / p95, ms, save → visible:

| | before | after |
| --- | ---: | ---: |
| L1 | 35.0 / 55.2 | 24.3 / 49.3 |
| L2 | 44.4 / 61.9 | 32.8 / 50.6 |
| L3 | 233.7 / 263.3 | 223.0 / 260.5 |

Raw rows: `macos_vnode_watch_mac_2026-11-11.json`.
