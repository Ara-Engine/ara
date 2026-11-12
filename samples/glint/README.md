# Glint

A rolling-ball game and Ara's internal test case for scripts, physics, native UI, audio and
the renderer together. One course, "Skyward": a route of pale stone decks carried on pylons
high above a cloud deck, in low warm sun. Steer a chrome ball with momentum; a good run is
50 to 90 seconds.

The course, in order: a wide straight (teaches steering), a gap (the first jump), a descent and
a kicker that throws the ball over a second gap, a climb, a split (a narrow beam paved with
orbs, or a wide detour), a banked turn that descends, two sliding platforms that hand over
to each other, three spinner bars to jump, a ramp up the cannon tower, and the cannon: it
draws the ball in, winds up and fires it 80 m through two rings onto the goal plaza. The goal's
beacon is in view from the start line. Falling off fades the screen and puts the ball back at
the last checkpoint arch. Times: gold 1:00, silver 1:15, bronze 1:40.

## Play it

On a fresh clone (Windows, PowerShell, from the repository root), one line:

```powershell
cargo build --release -p ara -p ara-import-worker -p ara-player -p ara-samples; .\target\release\ara-samples fetch glint; .\target\release\ara-samples kits glint; .\target\release\ara-player --game --project samples\glint --windowed
```

`fetch` downloads the one pinned third-party file set Glint uses (the Kenney UI font, CC0);
`kits` writes the generated, git-ignored files (kit geometry and the synthesised sounds).
Everything else (scenes, scripts, UI, import settings) is committed. Not run on Windows from
here; on the Mac `fetch`, `kits`, `build` and the headless replay were run with debug binaries,
and the windowed player was not opened (a shared machine).

Controls: WASD roll, Space jump, E back to the last checkpoint (after the goal: roll again),
Esc pauses. The camera follows the route by itself, so forward on the stick always means
"along the course".

```sh
ara-samples build glint                                 # re-author everything from the course table
ara-samples glint-replay                                # replay the recorded run, check the result
ara-samples glint-replay --captures target/glint-shots  # also write the stills listed in the replay
python3 samples/glint/replay/autopilot.py --ara <ara>   # play the course again and rewrite the replay
```

## How it is made

Everything comes from one table, `crates/ara-samples/src/build/glint/course.rs`: a turtle walks
the route and emits the walkable boxes, the glass rails, the sliders, bars, checkpoints, orbs,
rings, the cannon's ballistic solution, the camera's centre line and the autopilot's plan. The
kits (`kits.rs`), the colliders, the scripts' `Surface` components, the UI's numbers, the
comparison files (`compare/`) and the replay's route all read that table, so the look, the
physics and the scripts cannot disagree about where a piece is.

- **Scripts** (`content/scripts/Glint`, C# as a user would write it): plain `[Component]`
  structs; `Behaviour`s for the ball, the cannon, the camera, the run director and the ball's
  effects; `[Query]` systems for the sliders, spinners and bobbing orbs. Gameplay state lives in
  ECS components only.
- **UI** (`content/ui`): native UI bound to component fields. The markup is generated
  (`ui.rs`) because each HUD text needs a twin for its edge; the stylesheet is by hand.
- **Sound** (`content/audio`): synthesised in `sounds.rs` (no third-party audio): the roll loop
  (its volume and pitch follow the ball's speed), jump, landing, orb, checkpoint, the cannon's
  wind-up and shot, fall, finish, and a two-stem score (a pad, and a pulse that comes in as
  the run's pace rises).
- **Third-party**: only the Kenney Future font (CC0, pinned in `samples/assets.lock.json`).
  All geometry, icons and sounds are generated here.

## Light and materials (fixed, documented)

Physical units, one fixed exposure, no auto-exposure. The same numbers are in
[`compare/glint.spec.json`](compare/glint.spec.json).

| | |
|---|---|
| Exposure | fixed **EV100 13.5**, Khronos PBR Neutral, bloom 0.16 |
| Sun | 95,000 lux, colour (1.0, 0.84, 0.64), 20 degrees up, light travels (-0.163, -0.342, -0.926): from ahead and the left of the start, from behind after the turn |
| Sky | zenith (0.04, 0.17, 0.62), horizon (0.62, 0.8, 1.0), a warm horizon glow, 7,000 cd/m2 scale |
| Haze | height fog, density 0.012 at the cloud deck (61 m under the start), scale height 15 m: thick over the cloud, light at the course |
| Checkpoint lights (3) | point, 90,000 lm, radius 0.3 m, range 16 m, coral |
| Goal light | point, 160,000 lm, radius 0.8 m, range 40 m |
| Emissive | chevrons 45,000 cd/m2, rings, arches, cannon and platforms 50,000, goal 70,000, orbs 90,000 (a sunlit deck is about 7,000) |

Surfaces: stone (0.68, 0.67, 0.64) at roughness 0.75 and white paint at 0.5 are matte; the
polish is deliberate and limited to the navy inlay down each deck (roughness 0.06), a disc on
each pad and the goal's floor; chrome on posts and the ball; the rails are transmission glass
(IOR 1.5). One accent colour: coral (1.0, 0.33, 0.12).

## The deterministic replay

`replay/skyward.json` is a full run: 672 injected actions at exact fixed ticks, from the first
move to the goal, taking every orb. `replay/autopilot.py` produced it by playing the course
closed-loop in a headless world through `ara mcp` (`ara_play` start, step, read), following
`replay/route.json`, which the build writes from the course table. The replay file holds only
the inputs, so replaying never reads the world.

`ara-samples glint-replay` starts a headless world (seed 7, paused), steps it with the file's
inputs and compares three things with the file's `expect`: the run clock at the goal
(**2996 ticks, 49.93 s**), the falls (0) and the world state hash after the last tick. Every
run on one machine gives the same numbers; `--record` rewrites `expect`. Changing decoration
changes the hash but not the finish time, which is how each look pass was checked against the
gameplay.

How determinism is guaranteed:

- Time is the tick: the engine steps exactly N fixed ticks per call, with no wall clock,
  frame rate or device in the loop. Input is injected by action and tick, not by key.
- Gameplay state is ECS-only. Sliders, spinners and orbs are pure functions of the tick
  (`tick.Frame`, `tick.Elapsed`); the ball controller reads only components, the injected
  actions and the camera's yaw; the cannon's shot is a function of the ticks since it fired.
  There is no C# randomness.
- The physics step is fixed-rate and its state is in the replay snapshot.

What could break it:

- A different CPU, build or physics version: `MathF.Sin`/`Cos` and the solver's floating-point
  order are only promised identical on one machine and build. The state hash is not portable;
  on another machine the finish time may drift by a tick or the run may not finish, and the
  autopilot must record again.
- Anything that reads the wall clock or presentation state in `OnTick`.
- Script hot reload mid-run, a different tick rate or seed, or any edit to a collider, a
  slider, a bar or the ball's tuning (the inputs steer one particular course).
- The window path: a windowed player ticks at the fixed rate but device events arrive between
  ticks, so real-time input is not frame-exact. The replay uses `step` and the headless world.

## Cost (M5 Pro, 1280 x 720, Renderlab traced path; the Mac loaded by other work)

These are from this branch's captures with an `ara` binary built before today's Renderlab
pins, so treat them as relative; the Mac GPU thread measures the branch head separately.

| View | GPU ms | Shading | Indirect cache |
|---|---|---|---|
| Start (follow camera) | 50.0 | 24.7 | 18.3 |
| Overview | 36.0 | 13.6 | 18.0 |
| Turn | 45.8 | 20.7 | 19.9 |
| Sliders | 47.9 | 21.1 | 21.1 |
| Cannon | 38.0 | 15.4 | 18.1 |
| Goal | 42.8 | 19.2 | 18.4 |

What moved it since the empty-sky round (start 34 ms, mid-course 13.5, goal 10): the world.
With a cloud deck, islands and pylons, nearly every pixel now lands on geometry, and the
indirect cache costs about 18 ms in every view where it cost almost nothing over open sky.
Glossy surface on screen is small by design (the inlay, the pad discs, the ball, the goal's
floor); the start view has the most, a 6 m polished disc under the camera.
The simulation with scripts is about 0.5 ms a tick.

## Files

- `content/` the project: `levels/glint.scene` (the game), `levels/menu.scene` (the title's
  backdrop), `scripts/Glint`, `ui/`, `audio/`, `kit/` (generated geometry).
- `replay/` `skyward.json` (the replay), `route.json` (the autopilot's plan), `autopilot.py`.
- `compare/` numbers and camera path for the three-engine comparison, and export notes.
- `preview/` stills (1280 x 720 JPEG).
- `repro/alpha-black/` the minimal alpha-blend reproducer (Renderlab #65).
- [`FINDINGS.md`](FINDINGS.md) what building this through Ara's own interfaces showed.
