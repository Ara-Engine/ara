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

## Atomic saves (directory watches)
The per-file watch only covers in-place saves. Editors that save by writing a temp file and renaming it over the source (JetBrains safe write, vim, many others) still waited for FSEvents (11.7 ms median, `watch_latency.cs`). `VnodeEvents` now also watches each source folder (the module folder and its subfolders, skipping `bin`, `obj`, hidden folders; folders created later are added from the directory watcher's `Created` event). A directory vnode event fires when entries are added, removed or renamed, so a temp file renamed over the source is seen in ~0.1 ms; `ModuleWatcher` then lists the folder's source files and reports those whose modification time or length differs from what it last saw (and watches the replacement). Rules:

- A file that is missing is not reported from here (left to the directory watcher), so a save that deletes the source and then renames the new one in is not compiled half done.
- The directory watcher's echo of a save (`Changed`, `Created`, `Renamed`) is dropped when a fast delivery for that file is under 100 ms old and either its vnode watch is alive (in-place) or the file still has the stat recorded at delivery (nothing changed since). A delete is never dropped.
- Directories count against the same descriptor budget as files (a quarter of the soft limit, at most 1,024).

`reload_bench --atomic-save` saves this way. 20 fresh processes per arm, mirrored order, 45 edits per process; median / p95, ms, save → visible:

| | before (#36) | after |
| --- | ---: | ---: |
| L1 | 35.2 / 61.6 | 25.3 / 51.8 |
| L2 | 42.6 / 62.6 | 34.3 / 53.8 |
| L3 | 142.6 / 168.9 | 129.5 / 157.9 |

Raw rows: `macos_vnode_dirs_mac_2026-11-11.json`.

### Review round (directory watches)
- **Folder renamed or deleted while watched:** when a folder's own watch reports delete or rename (or the directory watcher reports `Renamed`/`Deleted`), every entry under it is released, so no stale path holds budget or blocks a folder created under the old name; folders (with their subfolders and files) are armed from `Renamed` as well as `Created`.
- **Budget:** folders are armed before files, and files cannot take the last slots (an eighth of the budget, at least 2), so a small budget (64 slots at the macOS shell default of 256) cannot starve the folder watches.
- **Burst of entries:** the kqueue thread only marks the folder dirty; a separate scan thread rescans it after a ~1.5 ms debounce, so a burst of temp files neither holds up in-place saves nor costs a scan per event.
- **`_known`** is also updated from the directory watcher's events, so a slow-path change is not re-reported by the next folder scan. A replaced file's watch is replaced (`Replace`), and arming happens before the duplicate check, so an echo can never be the reason a file stays unwatched.
- **New files** are reported only once their length and time hold still for ~3 ms, and not while empty (an empty file compiles to nothing, and its first write raises a file event). A partial first compile would show as errors until the rest of the file landed.
Spot check, atomic saves, 8 fresh processes per arm against #36: L1 33.0 → 26.1 ms, L2 37.5 → 32.7 ms (the 20-process campaign above, before these changes, was 35.2 → 25.3 and 42.6 → 34.3).
