# Editor UX v2, phase 1: evidence

Produced by `./crates/ara-editor/verify-ux.ps1 -Release` (`--verify-ux`) on the
RTX 4070 SUPER at exact physical 1440×900 and 2560×1369 (1x). Each step is
captured in both themes as `<step>-{dark,light}.png`. Floating windows are also
captured on their own as `<step>-window-{theme}.png`. The PNGs were losslessly
recompressed after capture, which removed the opaque alpha channel. The
per-capture layout reports stay in `target/`.

| Step | What it shows |
|---|---|
| `main` | The default layout: menu bar row, Hierarchy and Assets, Scene, Inspector |
| `menu-{file,edit,view,create,window}` | Menus, each with a submenu open: Edit ▸ Snapping, View ▸ View Mode, Create ▸ 3D Shapes, Window ▸ Layouts |
| `context-{hierarchy,viewport,field,tab}` | Context menus built from the registry |
| `palette`, `palette-entities` | Ctrl+K "fra" (titles, menu paths, ids) and the `@` entity mode |
| `settings`, `shortcuts` | The Settings window, and Keyboard Shortcuts with presets |
| `float-inspector` | The Inspector torn off into its own OS window |
| `tab-*` | Material, VFX, Animation, Profiler, Game, Console, Agents and Assets |
| `empty-inspector` | An empty state |

`ux-report.json` gives the viewport share and toolbar buttons without a registry
command, per step. `commands.json` is the registry export (272 commands: 155
enabled, 117 disabled with a reason). `timings.json` holds every raw sample.

| | 1440×900 | 2560×1369 |
|---|---:|---:|
| Viewport share, default layout | 54.0% | 57.2% |
| Viewport share, Inspector floating | 71.3% | 75.4% |
| Toolbar buttons without a command | 0 | 0 |
| Idle CPU per frame p50 / p95 | 0.080 / 0.123 ms | 0.080 / 0.117 ms |
| Idle frame interval p95 (144 Hz) | 6.652 ms | 6.642 ms |
| Warm camera → present p50 / p95 | 3.322 / 3.899 ms | 6.861 / 8.023 ms |
| Dirty render + present p50 / p95 | 2.876 / 3.191 ms | 6.412 / 7.330 ms |

Each size has 1,500 idle, 240 camera and 258 dirty-frame samples; no outliers
were removed. The host was shared: other worktrees were building, and CPU load
was 35–62% across the runs. These are observations, not a quiet-machine
benchmark. Camera → present at 2560 is dominated by the Lit · Fidelity viewport
render (about 2 MP of viewport).
