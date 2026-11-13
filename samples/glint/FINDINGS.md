# Findings from building Glint

What building a small game through Ara's own interfaces showed: the `ara` CLI, `ara do`
through `ara-samples build`, `ara capture`, `ara_play` over `ara mcp` (start, step, read,
capture, captureAudio, events), the C# module, the native UI and the `--game` shell, on the
M5 Pro. Ranked by how much each cost. Each item says what was wanted, what happened, the
workaround in this sample, and the smallest engine change that would remove it.
Items marked *from reading* were not confirmed by running. Renderlab was not touched.

## What worked well

- **The agent loop is real.** A headless world started over MCP, stepped tick by tick with
  injected actions, read back by component path and captured to a PNG, was enough to write an
  autopilot that finishes the course and records a frame-exact replay. No window was opened
  at any point. `ara_play step` returning a state hash made "did my change alter gameplay" a
  one-line check.
- **C# iteration is fast.** The module (five behaviours, three query systems, ten
  components) compiles in a couple of seconds inside `ara check`, and a behaviour fault comes
  back as a diagnostic with a C# stack (`ARA0911`).
- **Typed commands for authoring.** Every scene, sidecar, mixer and score is written by
  `ara do` batches; `ara check` caught bad ids, bad ranges and unsupported CSS before a run.
- **Audio has evidence.** `ara_play captureAudio` returns loudness, peaks, a spectrum and the
  per-bus levels, which is how the roll loop and the score were balanced without speakers.
- **Physical light units.** A scene lit by a 95,000 lux sun and emissive in cd/m2 behaved as
  the numbers said it would.

## P0: cost the most

1. **A behaviour that reads a physics component on the first tick is disabled for good.**
   *Wanted:* the camera and effects scripts to read the ball's `Velocity`. *Happened:*
   `Velocity` and `PhysicsPose` appear on a body only after the first physics tick; the read
   threw `MissingComponentException`, the engine disabled both behaviours "until the next
   reload", and the only sign was one `ARA0911` event among the published type tables, so the
   camera simply never moved. *Workaround:* `World.Has<Velocity>` guards. *Smallest change:*
   add the physics components when the body is authored (zeroed), or retry a faulted behaviour
   next tick for this exception, and print behaviour faults in `ara_play step`'s result.
   **Engine PRs: #153 (a resting `Velocity` from tick zero), #154 (`faults` in a step's result).**
2. **Scripts had no contact events, ray casts or sweeps.** *Wanted:* "is the ball on
   something, and what is its normal". *Happened:* nothing to ask, so every walkable box
   carried a `Surface` component with its collider's half extents and the ball tested itself
   against those boxes each tick; the course was limited to boxes. **Engine PRs: #158
   (`Physics.Raycast`) and #159 (`Physics.Contacts`).** With them the ball is grounded by one
   ray straight down, landings come from its contacts, and the `Surface` component and the
   box maths are gone from the scripts. What that changed, with #155's damping field (no
   cannon velocity override): the scripts went from 947 lines to 891 (683 to 641 without
   blanks and comments), the scene lost 37 `Surface` components, and the autopilot finishes at
   the same tick (2998). The tick costs the same within noise: 3000 ticks in one headless
   step with the ball at rest took 0.19 to 0.23 ms a tick before and 0.18 to 0.23 after
   (three runs of three each, same binary, the Mac loaded); the whole recorded run, stepped
   four ticks a call, 2.33 to 2.39 ms a tick before and 2.37 to 2.39 after, nearly all of it
   the cost of a step call. Still missing: sweeps and overlaps.
3. **Withdrawn: a kinematic platform does carry what rests on it.** These notes said a body
   moved by writing its transform slides out from under a rider and has no velocity to read.
   Both were wrong. The write becomes a kinematic target, physics moves the body with a real
   velocity, mirrors it into `Velocity`, and friction carries a resting box (pinned by the test
   in **#157**). What dropped the ball was this sample's own controller, which sets the ball's
   velocity every tick and so cancelled what friction gave it. The controller now steers
   relative to the `Velocity` of whatever the ball rests on, and its analytic copy of the
   slider's motion is gone. Lesson kept here on purpose: the gap was assumed from a symptom
   and never tested against the engine.
4. **Free flight is damped and a scene cannot say otherwise.** *Wanted:* a cannon shot that
   lands where the ballistic solution says. *Happened:* the ball lost about 5 percent of its
   horizontal speed a second and landed 10 m short of a 97 m shot; `RigidBody` has no damping
   field. *Workaround, now removed:* the cannon set the velocity from the time since firing
   each tick. *Smallest change:* `RigidBody.linearDamping` and `angularDamping`, defaulting to
   what the physics engine uses now. **Engine PR: #155, merged.** The ball is authored with
   `linearDamping` 0 and the shot flies on physics alone; the replay finishes at the same tick.
5. **`castShadows: false` does not stop a mesh shading the scene on the traced path.**
   *Wanted:* a sun disc (the sky draws none): a lit sphere far along the sun's direction.
   *Happened:* the 52 m sphere put a 300 m long ellipse of the course in shadow, with
   `castShadows` off. Retested on a build of main `5ea43c0db` (Renderlab 191462db): still so;
   [`preview/defects/castshadows_false_still_shades.jpg`](preview/defects/castshadows_false_still_shades.jpg)
   shows the right-hand pad and slider in the disc's shadow. *Workaround:* the disc sits 2.6
   degrees high so its shadow passes over the course. *Smallest change:* honour the flag in the
   traced sun visibility, and give `Environment` a sun disc.
6. **`ara capture` could not show a UI screen, and nothing can click through the shell.**
   *Wanted:* stills of the title and the course page. *Happened:* a capture always drew the
   in-game HUD, so for five rounds the title was never seen; when it finally was, it had four
   hard shade bands across it and a heading that overflowed its panel. *Smallest change:*
   `ara capture --screen <main-menu|lobby|loading|settings|pause|hud>`. **Engine PR: #156**;
   the title, course page and pause stills in `preview/` were taken with it.

## P1: visibly wrong, or slow to work around

7. **The UI cannot edge or animate text.** No `text-shadow`, stroke, opacity, gradient,
   transform or transition. *Wanted:* HUD text that reads over both white cloud and deep blue
   sky, a title with a logo treatment, panels that fade. *Workaround:* each HUD text is two
   `text` nodes with the same binding, a white twin one or two pixels down and right of the
   navy one (generated, `ui.rs`); the fall fade is three stacked panels switched by bools; the
   title does not animate. *Smallest change:* `text-shadow: x y colour` (no blur needed).
8. **No number format in bindings.** *Wanted:* `0:07.52`. *Happened:* `{value}` prints an
   integer; `clock` is `m:ss` only. *Workaround:* the director writes minutes, seconds and
   hundredths as separate fields and the bindings carry `labels` maps for 0..59 and 0..99
   (five such clocks: 15 fields, 15 bindings). *Smallest change:* a `format` on a binding
   (`{value:02}`), or a `clock` that takes ticks and a pattern.
9. **CSS subset surprises.** `font-family` on the root does not reach `text` (only a rule on
   `text` applied the font); a hidden element still takes its place in the layout (three medal
   images pushed the results off screen until stacked absolutely); no grouped, descendant or
   compound selectors; no negative offsets; `text-align` is left or right only (no centre, and
   that one is reported at play time, not by `ara check`); every element needs an id. Each was found by a
   failed `ara check` or a wrong picture. *Smallest change:* list the supported subset in
   `ara docs` and make a hidden element leave the flow (or add `display: none`).
10. **Feedback sounds are heard at the entity that carries the counter, and cannot be
    verified headless.** A `sounds` route attenuates by the distance from the listener to that
    entity, so a counter on the run director (which has no place) is silent; the fall and
    finish counters live on the camera (`RunFeedback`) for that reason. `captureAudio` renders
    the scene's `AudioSource`s and the score but not these one-shots, so they are untested by
    ear or by evidence. *Smallest change:* a `spatial: false` on a route, and feedback
    one-shots in `captureAudio`.
11. **`ara capture` does not wait for model imports.** The first capture after a build draws
    magenta placeholders. *Workaround:* a throwaway capture first. *Smallest change:* wait for
    the scene's pending imports (or fail with a code that says so).
12. **The importer fails on a large flat mesh.** A 500 m slab failed with `patch charts:
    FragmentLimit` (250 m imported). *Workaround:* small kits scaled by the entity.
    *Smallest change:* a diagnostic that names the limit and the mesh, and a way to skip chart
    generation for a mesh that needs none. **Engine PR: #161** (the message; the budget is
    16,000,000 fragments of 0.25 m patches, about 500,000 m2 of flat surface).
13. **Emissive transmission glass does not assemble.** A glass material with an emissive
    factor failed with `ARA7300: a transparent material's program with Parameters is not
    assembled`. Not needed in the end.
14. **Strongly flattened spheres draw dark triangles.** High clouds scaled to a third of
    their height showed black facets where spheres meet; at 0.7 they are clean. Not chased.
15. **There is no local fog volume.** *Wanted:* a soft top layer on the cloud deck. `HeightFog`'s
    doc speaks of local fog volumes, but the schema has no such component (*from reading* the
    schema list), and particles do not draw here. The clouds are smooth lumps (one closed mesh
    each) softened by a thick, low height fog.
16. **The UI font's figures are not tabular and there is no way to ask for it.** The running
    clock is one fixed-width cell per digit, each bound to its own field.
17. **Particles do not draw on the Mac** (`ARA_RENDERLAB_UNSUPPORTED particle emitters`), so
    the spark trail exists in the scene and in no still. Known.
18. **No debug views on Renderlab** (`ARA_VIEW_MODE_UNAVAILABLE`), and no way to ask what
    occludes a point: the sun-disc shadow (item 5) was found by arithmetic on the scene file. *Smallest change:* `ara why <entity> in shadow`, listing the occluders of a
    point toward a light.

## P2: friction

19. **The `--game` shell is Outpost-shaped.** It needs `levels/menu.scene` with two points
    named `e_gamemenueye` and `e_gamemenutarget`; the loading label and window title say
    Outpost; a solo game passes through a co-op lobby page that needs a Ready click (restyled
    here as the course page); the UI actions and the input actions are fixed lists (restart is
    `Interact`); and the title's dark left-to-right scrim is painted by the player in Outpost's
    colour (`game_ui/decoration.rs`), whatever the project's stylesheet says. *Smallest change:*
    read the title and the menu scene from the project, let a project skip the lobby, and make
    the scrim a style.
20. **The menu click must be an Ogg file** (`MediaDecoder::open(.., "ogg")`); a synthesised WAV
    cannot be used and the tooling has no encoder, so Glint's menus are silent.
21. **`clean` in the sample builder does not know mixers**, so a second build failed with
    `ARA1602` until the mixer was created once and updated after.
22. **Gravity is not readable from a script** (*from reading*: nothing named gravity in
    `Ara.Runtime`), so the cannon's arc repeats 9.81 in the build and in C#.
23. **`FollowCamera` has no "follow the route" mode.** A script turns its yaw toward a
    polyline of waypoint entities (`CourseCamera`); spline or path components would do this.
24. **A script that writes one Euler component of a physics body's rotation gets the wrong
    rotation.** Physics writes a kinematic body's pose back to `Transform` as Euler angles, and
    past 90 degrees of yaw those are (180, 180 - yaw, 180). The spinner bars wrote only
    `Rotation.Y`, so each bar swung back and forth instead of turning, with a velocity spike at
    each flip (-64 rad/s read for one tick). Fixed by writing the whole vector. *Smallest
    change:* keep the authored Euler triple when the pose physics hands back is the pose the
    script asked for, or give `Transform` a yaw setter.
25. **Two captures of the same view are not byte-identical here.** On one build, two
    `ara capture` runs of the same camera and tick differ in up to 0.06 percent of pixels (most
    by one level, a few by up to 14). So a hash cannot say "this is the same frame"; comparing
    stills needs a pixel tolerance. (With the pipelined present on and off the replay's stills
    differ by no more than that, which is how "the still is the frame asked for" was checked.)
26. **Small ones:** `ara_play read` takes `paths`, `status` takes no world, `events` takes no
    limit (each rejected with a good message); entity ids must be `prefix_base32`;
    `HeightFog.falloff` has no stated unit in its range error; the capture JSON's pass list
    needs a script to read.

27. **A checkout without the C# tools built could not say so.** *Wanted:* the first run on a
    fresh Windows checkout to say "run `dotnet build dotnet/Ara.slnx -c Release`". *Happened*
    (reproduced on the Mac with the tools hidden): `ara play` and `ara capture` do say it
    (`ARA0904`), but `ara-player --game` printed it once and then one `ARA1106: unknown
    component` per scripted entity (19 for Outpost), so the last screen was about components;
    and `ara doctor`, which every such message points to, checked only the .NET 10 install and
    said OK. With a host built from another commit the stop is `ARA0921: C# rejected the ABI
    identity`, which names the fix. *Engine:* Ara #164 returns the project's error alone and
    adds an `Ara C# tools` check to `ara doctor`.

28. **The player says nothing about audio.** *Wanted:* to confirm sound on Windows from a log.
    *Happened:* on success the player printed nothing, and the `--game` shell printed nothing
    on failure either, so a run with sound and a run without looked the same. **Engine PR:
    #166** (one start-up line naming the device, driver and format, or the reason there is
    none). The mixer can be checked without a device: `ara-samples glint-replay --audio <wav>`
    renders three seconds at the end of the run and fails on silence (master bus peak
    -13.1 dBFS on the Mac). The one-shot sounds are still unverified (item 10).

29. **A respawn is a camera cut, and on the untraced path a cut is followed by slow frames.**
    Reported by the integration thread on the RTX: after a history cut the untraced path
    spends about 50 frames gathering GI before it is back to speed. In steady state at the
    start view it costs 16.4 ms of GPU with the camera still and about 22.7 moving, against
    11.2 and 12.5 traced (1080p, 300-frame clips; the README has the table). Glint
    never asks for a cut and scripts cannot: the Renderlab bridge cuts by itself when the
    view moves more than 20 m or turns more than 45 degrees in one frame
    (`crates/ara-renderlab/src/bridge.rs`, `jumped`). The follow camera has no lag, so that
    happens whenever the ball is put back: a fall (the ball returns to the last checkpoint,
    usually more than 20 m away; a shorter hop does not cut), E mid-run (the same), and "roll again" after the goal
    (back to the start). Each comes with a screen fade, 40 ticks after a fall (14 of them
    opaque) and 24 after E or a restart, so part of the slow stretch is hidden but not 50
    frames of it. The cannon does not cut: the ball is carried into the breech over a few
    metres and the shot is continuous (about 27 m/s, under half a metre a frame, the yaw
    limited to 110 degrees a second); nor do the results view or the pause.

30. **A world stepped to a tick could only be captured as a still.** *Wanted:* steady-state
    cost at each replay position. *Happened:* `ara_play capture` rejected `stability` and
    `video` (`ARA1101: PlayCaptureParams has no field stability`), though the player renders
    both for `ara capture`; a still is taken right after a history cut, so the first RTX
    numbers for the replay views were read inside the untraced path's GI backlog and were
    withdrawn. **Engine PR: #168** (the live capture takes the hold and clip parameters).
    With it `ara-samples glint-replay --captures <dir> --hold N [--travel METRES]` plays N
    live frames at each of the eleven positions (checked on the Mac at 24 held frames and as
    12-frame clips; the replay still ends at the same tick and hash).

31. **A sample build that overlaps a daemon of the same project can write a scene with a
    third of its entities missing, and says "built".** *Happened once* (observed, not
    isolated): `ara-samples build glint` started right after killing an `ara serve` that a
    capture had left running; the scene came out with 175 records instead of 252 (no deck
    colliders, most rails, clouds and islands gone), `ara check` inside the build passed, and
    the only symptom was the ball falling through the launch deck in the replay. The same
    build a minute later, with no daemon alive, wrote all 252. *Wanted:* the build to fail, or
    to take the project lock the daemon holds.
32. **The UI cannot fade.** The key hints were asked to fade out after the first input; with no
    opacity or animation in the UI subset (item 7) they switch off instead: shown until the
    first input of the session, and again only after seven idle seconds in a run.

## Where the engine was bypassed

- **Geometry** is written as glTF by the sample builder's `Kit` (boxes, cylinders, spheres),
  not modelled in Ara: there is no mesh authoring command.
- **Sounds** are synthesised to WAV by the builder: there is no procedural audio asset
  (`ara-audio` has `SignalSource`, but nothing authors one).
- **Icons** are rasterised by the builder from distance fields: the UI draws no shapes.
- **The UI markup** is generated text written to disk, not `ara do` commands.
- **The autopilot** is a Python script speaking MCP over stdio: `ara` has no command that runs
  a plan of timed inputs and writes a replay (`ara-samples glint-replay` is this sample's own).
- **The title screen** was first seen only once the engine could capture it (item 6).

## Renderer notes for the threads using this level

- Cost by view and what moved it are in the README. The indirect cache cost about 18 ms a
  frame while the clouds were clusters of 21 intersecting spheres (106 instances) and fell to
  about 1.6 ms when each bank became one closed mesh; the pylons changed in the same round, so
  the cause is not isolated. Worth a look: many overlapping closed meshes under the camera.
- Glossy surface on screen: the start view about 15 percent (the pad's polished disc and the
  ball), the straights 5 to 8 percent (the 1.1 m inlay and the ball), the goal about 20 percent
  (its mirror floor). Estimated from the stills, not measured.
- The plain alpha film (the ball's old contact shadow) is gone; nothing in the level uses
  `BLEND` now. The rails are transmission glass.
- **What changes every tick with the camera still** (all from `Movers.cs` and `BallFx.cs`;
  everything else, including the sky, the fog, the clouds, the isles and every other emissive,
  is static, and no emissive level is animated):
  - 43 orbs: each bobs 0.16 m and turns 140 degrees a second. They are the level's brightest
    emitters (90,000 cd/m2), so 43 small emissive instances move every tick.
  - 2 sliding platforms (kinematic bodies with lit edges, 3.2 m each way, period 270 ticks).
  - 3 spinner bars (kinematic bodies, turning all the time).
  - 1 trail under the ball: its transform is written every tick, with zero length while the
    ball is slow or in the air.
  - The cannon's carriage moves only while it loads and fires; the ball only when rolled.
  - The HUD clock redraws while a run is on.
- **A still is already the frozen case.** `ara capture` and `ara_play capture` do not tick the
  world: with a world stepped to tick 30, a capture left the tick at 30 and an orb's transform
  unchanged (checked on the Mac). So the replay stills measure a level in which none of the
  above moves: whatever a still costs, it is not their motion.
  A live run with them frozen is not one switch today (the sliders and bars stop with
  `periodTicks` 0 and `degreesPerSecond` 0 in the scene, but the orbs' bob is a constant in
  the script).
