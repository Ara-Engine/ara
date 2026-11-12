# Findings from building Glint

Gaps hit while building a small rolling-ball game as a user of Ara (C# scripting, the `ara`
CLI, `ara capture`, `ara_play` over MCP, the native game shell, Renderlab on the M5 Pro).
What was tried, what happened, what was done instead. Severity: **P0** blocks the work,
**P1** visibly wrong or slow, **P2** friction. House style: [`../FINDINGS.md`](../FINDINGS.md).
No engine code was changed. Items marked *from reading* were not confirmed by running.

## What worked

- The C# module (six files: components, two behaviours, `[Query]` systems, helpers) compiled
  with no diagnostics on the first build, and a headless `ara_play` run steps it.
- Entities carrying the module's components are accepted by `ara do` once it compiles
  (`ara-samples build glint`, a couple of seconds).
- Native UI bound to component fields did the whole HUD and results panel: timer (`clock`),
  checkpoint and falls counters, a state label, and a panel shown by a bool.
- `ara_play step` returns a state hash; with the recorded inputs it gives an identical finish
  time and hash on every run (see README).
- The shell's route (title, Roll, Loading, start page, Ready, Game) runs on Glint.

## P1

1. **Scripts have no contact events, ray casts or impulses.** `Movement` has `Stop` and
   nothing else for physics bodies. The ball is a dynamic sphere, so grounding is a test
   against the course's own `Surface` components (same half extents as their colliders)
   and steering writes `Velocity`. A course of anything but axis-aligned boxes would need
   its geometry duplicated in script.
2. **A platform moved by writing its transform does not carry a ball.** The sliding platform
   is a kinematic body placed from the tick number, as the Outpost doors are. The ball
   resting on it stayed where it was and the platform slid out from under it (found by the
   autopilot falling at the platform's far edge). Worked around in script: the controller
   adds the platform's analytic velocity while the ball rests on it
   (`Course.MoverVelocity`). The engine's own kinematic-body velocity is not exposed either.
3. **`ara capture` does not wait for model imports.** The first capture after a build drew
   magenta placeholders (the whole course missing, a magenta cube where it should be); the
   same command a minute later, with the cooks cached, was right. Atelier needed the same
   second capture after a change to its kits.
4. **Particles do not draw on the Mac.** The smoke, sparks and goal swirl are in the scene;
   the player logs `ARA_RENDERLAB_UNSUPPORTED ... particle emitters: Renderlab's Metal
   backend leaves it out of the frame` and none appear in any still. Kept as engine-side
   extras. Known; listed here because the brief asked for particles at the goal.
5. **Script time is not reported apart from the simulation.** Outside the editor's profiler
   (`docs/dev/player-profiling.md`) the only numbers are `ara_play step` wall time and the
   renderer's `External.*` passes; "simulation with scripts" is one figure (0.27 ms a tick idle,
   0.48 rolling) and cannot be split into physics, the native sync and C#.

## P2

6. **The `--game` shell is Outpost-shaped.** It needs a `levels/menu.scene` with two points
   named `e_gamemenueye` and `e_gamemenutarget` (the fly-over camera) and fails with
   `authored native menu scene is missing` otherwise; the Loading screen's label and the
   window title are hard-coded to Outpost (`Ara | Outpost — Dusk shift`,
   `Outpost: Preparing assets: 3 / 6` on Glint's loading page); every solo game passes
   through a co-op Lobby page that needs a Ready click (Glint restyles it as the
   course's start page); and the actions are fixed (`solo`, `host`, `join`, `ready`,
   `resume`, `leave`, `settings`, `quit` on the UI side; `Move`, `Look`, `Jump`, `Sprint`,
   `Interact`, `Inventory` as input, so restart is E). The hidden `--bot --bot-solo` route
   drives the menus on any project, then plays Outpost's mission, which is how the route
   above was checked; there is no supported way to click through the menus or screenshot
   the title screen (the stills use `screencapture -l` on the player's window).
7. **`ara_play read` takes `paths` (a list), not `path`.** Rejected with `ARA1101`, with a
   good fix hint.
8. **`ara init` leaves an empty `content/scenes/main.scene`** that `project.setMain` then
   makes redundant (also in Atelier).
9. **Entity IDs must be `prefix_base32`** (`e_cam_a` is rejected, `e_camA` accepted); the
    rule is only in the error.
10. **`HeightFog.falloff` is limited to [1, 10000] with no stated unit**, and the density
    scale is not documented: density 0.004 turned the whole yard into orange haze, 0.0012 was
    still too thick, 0.0003 (falloff 30) is the subtle dusk haze used.
11. **Textured materials work, but `Material::textured` needs its sets fetched first.**
    `ara-samples fetch` does nothing until the project directory exists (`ara init` first),
    and a binary built in another worktree fetches into that worktree (`--samples`).
12. **The goal fired while the ball was still in the air** over the goal island (my script:
    a position test only). Fixed by requiring a grounded ball, which moved the recorded
    finish from 1929 to 1947 ticks. A useful accident: the replay caught the change.
13. **No albedo or other debug views on Renderlab.** `ara capture --view albedo` fails with
    `ARA_VIEW_MODE_UNAVAILABLE ... the renderlab-rt renderer draws the lit image only`, so a
    surface that renders brighter or darker than its material (the first dusk's orange wash
    over a 0.035-albedo yard) cannot be split into light and material; the fix was to change
    the light and look.
14. **`FollowCamera` framing is undocumented in effect.** `distance`, `height` and `pitch`
    interact so that the ball sat 75 to 90 percent of the way down the frame for the first
    three settings tried; (distance 5.5, height 1.4, pitch 20) puts it at about 65 percent.
    Found by capturing each setting; the schema docs do not say where the ball lands.
