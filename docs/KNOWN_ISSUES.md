# Known issues: Windows preview 13 (0.0.1-alpha)

The engine installer, standalone Ara Hub, side-by-side engine installs and global
`ara mcp` are available. The editor has an in-editor Game preview and embedded Play;
Build Current Profile exports a runnable game. This is still an early Windows alpha.

- **Editor interaction latency:** dragging still exceeds the 16.7 ms p95 target.
  Scene rendering runs synchronously on the UI thread, delaying input and presentation.
  Moving that work off the UI thread is in progress.
- **Game preview and Play can look different:** the preview uses the Scene renderer's
  exposure, while Play has its own exposure history. Play's early frames can differ.
- **Game preview VFX:** particles are not composited into the stopped Game preview yet.
- **Cold imports are slow:** the installed Outpost first-hour run waited about
  **2.3 minutes** for 115 imports. Texture compression remains a CPU cost; a GPU codec
  is in progress. Previously imported content uses the project cache.
- **The first Build can be slow:** the installed Outpost journey's first export took
  about **130 seconds**, including runtime verification and private symbol processing.
  This is an observed run, not a fixed duration for every project or machine.
- **Unsigned downloads:** neither the engine nor Hub setup has Authenticode signing.
  Windows SmartScreen may warn. Release manifest signatures and download hashes do
  not replace Windows executable signing.

The import and Build timings above come from preview 13's offscreen installed-build QA
on the shared Ryzen 7 7800X3D / RTX 4070 SUPER Windows PC. They are not quiet-machine
benchmarks. The packaged smoke also checks Starter export and runs the exported game;
passing it does not establish renderer fidelity or interaction performance.

These are outstanding work, not scope cuts ([D7](decisions.md)). Other platforms,
GPU vendors, manual screen-reader use and the full range of Windows IMEs have not
been validated by this installed-build QA.
