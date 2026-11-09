Reference checkpoint for the editor rewrite, 2026-10-28. Implementation stopped at Islam's direction.

The branch preserves existing unverified panel drafts and the offscreen acceptance harnesses. Patches and partial Package sources are reference material, not a merge-ready product. Their original D:/AraBuild/ux-editor-target/orchestration copies remain untouched. No tests or captures were started to create this checkpoint.

Completed source checkpoints remain on their original local branch refs. The accepted-text source is 2df9a24c5643997c7e0e6448d49319537ee7cb5a: primitive 2, router 1, Inspector 3, full kit and normal goldens, 514 editor library tests, strict Clippy, diagnostic coverage and formatting passed. Windows native Collections rerun is outstanding. The previous exact82 native chain passed its first two Collection cases and failed focused text Undo on case three; retained evidence is under D:/AraBuild/ux-editor-target/acceptance/82eaead1/.

The kit feedback source is 96e73f91defcb9e9171e76287816fe61d9e2d85c; its matching native core16/import9/assets12 and 113 clean geometry reports are documented in reports/ux-editor-preview4.html and https://597a3e3172bbca1a.snist.dev/v/5.

The latest 60-second monkey baseline on96 passed final cleanup. No accepted 30-minute soak, no formal timing, and no completed remaining-panel acceptance is claimed.