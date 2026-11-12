# Glint

A small rolling-ball game and Ara's internal test case for scripts, physics, native UI and
the renderer together. One course so far, "Foundry Steps": a catwalk over a foundry yard at
dusk. Steer a chrome ball with momentum, jump, ride a sliding platform, clear two spinner
bars, pass two checkpoints, reach the beacon. The clock starts on your first move, falling
puts you back at the last checkpoint, the results panel shows time, best and falls, and E
rolls again.

Game logic is C# written the way a user would write it (`content/scripts/Glint`): plain
`[Component]` structs, two `Behaviour`s (the ball, the run director) and `[Query]` systems for
the movers. Gameplay state lives in ECS components only. The HUD, title, start page, pause
and results are native UI bound to component fields (`content/ui`). Authored by
`ara-samples build glint` (`crates/ara-samples/src/build/glint`); look and gaps are in
[`FINDINGS.md`](FINDINGS.md).

```sh
ara-samples fetch glint                                 # three CC0 texture sets, hard links from the cache
ara-samples build glint                                 # kits, game scene, menu backdrop
ara-player --game --project samples/glint --windowed    # play it: title, Roll, start page, course
ara-samples glint-replay                                # replay the recorded run, check the result
ara-samples glint-replay --captures target/glint-shots  # also write start, mid-course and goal stills
```

Controls: WASD roll, Space jump, right mouse drag orbits the camera, Esc pauses, E restarts
after the goal. The shell's action set is fixed (see FINDINGS), so restart is `Interact`.

## The deterministic replay

`samples/glint/replay/foundry.json` is a full run: 483 injected actions at exact fixed ticks, a
deliberate fall off the beam (respawn at checkpoint 1), the goal, and a restart on E. It was
produced by a closed-loop autopilot that played the course in a headless world; the file
holds only the inputs it sent, so the replay never reads the world.

`ara-samples glint-replay` starts a headless world (`ara_play start`, seed 7, paused),
steps it with the file's inputs (`ara_play step`) and compares three things with the file's
`expect`: the run clock at the goal (**1947 ticks, 32.45 s**), the falls (1) and the world state
hash after the last tick. Every run gives the same numbers; `--record` rewrites `expect`.
Adding decoration changes the hash but not the finish time, which is how the look pass was
checked against the gameplay.

How determinism is guaranteed:

- Time is the tick: the engine steps exactly N fixed ticks per call, with no wall clock,
  frame rate or device in the loop. Input is injected by action and tick, not by key.
- Gameplay state is ECS-only. The movers and spinners are pure functions of the tick number
  (`tick.Frame`, `tick.Elapsed`); the ball controller reads only components, the injected
  actions and the camera's yaw. There is no C# randomness (the context RNG is never used).
- The physics step is fixed-rate and its checkpoints live in the replay snapshot.

What could break it:

- A different CPU, build or Jolt version: `MathF.Sin`/`Cos` in the movers and the solver's
  floating-point order are only promised identical on one machine and build. The state hash
  is not portable; the finish time may drift by a tick or fail to finish at all.
- Anything that reads the wall clock or presentation state in `OnTick` (`OnFrame`, `Frame.Dt`).
- Script hot reload mid-run, a different `tickHz` or seed, or editing a collider or a mover
  (the recorded inputs steer a particular course; a change needs a new recording).
- Physics sleeping and contact order if a second dynamic body is added (the ball never sleeps).
- The window path: a windowed player renders at its own pace but ticks at the fixed rate;
  replaying real-time input there is not frame-exact because device events arrive between
  ticks. The replay therefore uses `step` and the headless world.

## Frame times (M5 Pro, 1280 x 720, Renderlab RT, the Mac loaded by other work)

| | |
|---|---|
| Simulation with scripts (headless `step`) | 0.27 ms a tick idle, 0.48 ms a tick while rolling and bouncing (includes the MCP call), against a 16.7 ms budget |
| Render, start view (`External.Frame.Gpu`) | 33.8 ms GPU, 38.9 ms wall, shading 26.4 ms |
| Not split | the engine does not report script time apart from physics outside the editor's profiler |
