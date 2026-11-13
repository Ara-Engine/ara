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
   **Fixed for the trail and the speckles (Ara #192, Renderlab #93; Ara main `a7a124dfa`):**
   the flag reaches the renderer. On the M5 the ball's spark trail no longer throws a shadow
   on the deck beside itself, and the faint warm speckles that covered the sunlit decks are
   gone: forcing the sun disc to cast again at run time brings them back across the start
   view, and switching it off removes them. All eleven replay views differ from the previous
   main by that much (0.1 to 0.8% of pixels by more than 8 levels; the same build twice,
   0.00%), at the same shading time. The disc's 300 m shadow was not re-tested: with the disc
   moved onto the sun's direction and forced to cast, today's renderer drew no such shadow
   either way, so the test says nothing. The 2.6 degree workaround is left in place. The
   committed stills were re-shot after it (item 33).
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
    cannot be used and the tooling has no encoder. Glint's menus were silent for that reason;
    they now use the click from Kenney's Interface Sounds (CC0, already pinned for Outpost),
    which also makes the shell open its audio device at the title. *Smallest change:* decode
    the click by its file's extension.
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
    *When the line prints for Glint:* not at the title. The shell opens a menu device only
    for a UI click sound, and Glint has none (item 20: the click must be an Ogg), so nothing
    is opened and, before this follow-up, nothing was said. `audio (play)` prints when a run
    starts (ROLL, then START): the device opens with the world, not with the window. A
    windowed start left on the title is silent for as long as it sits there. The shell now
    says so in one line.

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

33. **Fixed (Ara main `b958132fd`, Renderlab `fdf2fb05`: sun shadows to 1 km): the pylons
    cast no shadow on the clouds; the decks did.** On the M5 the pylons now stripe the clouds
    under the course, and the rings, the bridge lattice, the towers and the far decks reach
    them too. The same three views, with the pylons and then the course switched to cast no
    shadow at run time (`castShadows`, honoured since item 5's fix) and back:

    | View | Pylons not casting | Course not casting | The same frame twice |
    |---|---|---|---|
    | Overview | 4.9% of pixels by more than 8 levels | 1.8% | 0.5% |
    | Start | 1.9% | 3.7% | 0.3% |
    | Goal | 1.3% | 1.0% | 0.2% |

    Sheets, laid out as before:
    [`cloud_shadows_over_fixed.jpg`](preview/defects/cloud_shadows_over_fixed.jpg),
    [`cloud_shadows_start_fixed.jpg`](preview/defects/cloud_shadows_start_fixed.jpg),
    [`cloud_shadows_goal_fixed.jpg`](preview/defects/cloud_shadows_goal_fixed.jpg). The stills
    in `preview/` are re-shot on that main. Seen on the same shots (M5, traced path):
    - **The clouds are left as they are.** They are solid white mounds and take the shadows
      as such. The edge is the sun's: the renderer draws the penumbra of its 0.54 degrees
      (at 0.05 degrees the cloud shadows are crisper, at 2 degrees the lattice's dissolve
      into soft bands), so shadows thrown from 45 m up are already softer than the ones on
      the deck. A wider sun would soften the clouds' shadows and the deck's crisp ones
      alike, which is the course's look; Ara has no switch for a mesh that receives no
      shadow; and the Unreal and Unity copies already draw these shadows. Nothing changes
      in the level or its copies.
    - The inside of a big shadow on the cloud is mottled: blotches a few levels either way
      under the goal's deck, where the sunlit cloud beside it is even. The indirect light's
      blotches (as in Atelier), which the sun used to hide.
    - The row of small dashes between the hoops' shadows at the goal is the orbs' shadows:
      with the 43 orbs switched to cast none, the dashes go and nothing else changes.
    *As first found.* Asked by the Unreal
    side-by-sides, where hardware Lumen draws the bridge lattice's shadow on the right-hand
    cloud at the start and pylon shadows across the clouds at the goal, and Ara shows almost
    none. *What the scene asks for:* every course piece, pylon, ring and cloud bank casts and
    receives. Nothing in Glint turns either off except the sun's disc and the ball's trail
    (`castShadows: false`, which the traced path ignored when this was measured: item 5); no component has a
    shadow distance; the sun is one directional light of 95,000 lux, 0.54 degrees across, 20
    degrees up, so a deck 45 m over a cloud top throws its shadow about 120 m along the cloud.
    *What Renderlab draws* (M5, traced path, main `9a6541060`, Renderlab 99f5d395): with the
    course model moved 3 km up, its shadow leaves the cloud (the launch deck's grey
    parallelogram in the overview); with the pylons model moved 3 km down, nothing on any
    cloud changes except a faint soft darkening round where they stood, in the start, goal and
    overview stills. So the decks' shadows reach the clouds and the pylons', which stand in
    the clouds and should stripe them for 120 m each, do not. The sheets are
    [`cloud_shadows_over.jpg`](preview/defects/cloud_shadows_over.jpg),
    [`cloud_shadows_start.jpg`](preview/defects/cloud_shadows_start.jpg) and
    [`cloud_shadows_goal.jpg`](preview/defects/cloud_shadows_goal.jpg): the still, the still
    without pylons, the still without the course, and under the last two their difference
    from the first, six times amplified. The height fog is not the cause: with its density
    at zero the clouds gain contrast and still carry no pylon shadows. *Not isolated:* why.
    The pylons are one model of tapered columns 220 m tall whose bounds cover the whole
    course; the decks are boxes in another model. The ring and rail shadows the Unreal still
    shows were not tested separately. A renderer finding; nothing in the level is changed.

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
- **The goal is the dearest view, and it is a sum, not one thing.** The results view at the
  goal costs about a third more than the rest of the course on both paths (RTX, 1080p, 300
  held frames, before the great ring: 13.5 ms traced and 20.8 untraced against medians of 8.8
  and 15.0; M5, 720p, 60 held frames, with the ring: 32.1 and 32.4 ms against about 24).
  Removing one thing at a time on the M5 (traced path only, one run each, same binary):
  the goal's lit surfaces made unlit (floor rings, halo band, spire bands, beacon, the ring's
  inner band): 27.7 ms, 4.5 less; the mirror floor made matte and the chrome portal casing
  painted: 29.2, 3.0 less; the great ring removed: 30.2, 2.0 less (most of that is its lit
  band, so it overlaps the first). No single one brings the view to the course's level. It is
  the one place where the level's brightest emitters, its only mirror floor and the whole
  course in the background are in one frame. Not changed: it is where the run ends and it is
  meant to be the richest view.
  *A second pass, looking for anything free* (M5, traced, 720p, 60 held frames, on Renderlab
  614b15da; base 30.6 ms of GPU, 21.8 of shading, two runs agreeing within 0.1; the spinner
  deck in the same pass is 18.7):

  | One change at the goal | GPU ms | Returned |
  |---|---|---|
  | Beacon thin again (radius 1.4 to 0.5 m) | 30.6 | 0.0 |
  | Spires' lit fins rebuilt as surface strips, no emitter buried in the pillar (same picture) | 30.6 | 0.0 |
  | Floor rings unlit | 30.1 | 0.5 |
  | Portal and halo bands unlit | 29.8 | 0.8 |
  | Great ring's band unlit | 29.7 | 0.9 |
  | Spires' lit fins unlit | 29.5 | 1.1 |
  | Chrome portal casing painted | 29.2 | 1.4 |
  | Mirror floor at roughness 0.5 | 29.1 | 1.5 |
  | Mirror floor at roughness 0.2 | 32.8 | -2.2 |

  Nothing at the goal sits just under the 0.5 cut-off by accident: the only surfaces under
  it are the mirror floor (0.05) and the chrome (0.06), both meant. No emitter costs more
  than about a millisecond, emitting area that cannot be seen costs nothing (the thick beacon
  and the buried fins are free), and every change that returns time changes the picture. So
  there is no pair here that cannot be told apart; the goal's cost is the renderer's to
  bring down. One thing worth its knowing: a floor at roughness 0.2 costs 2.2 ms more than
  the same floor as a mirror, so between the cut-off and a mirror, rougher is dearer.
- **A still is already the frozen case.** `ara capture` and `ara_play capture` do not tick the
  world: with a world stepped to tick 30, a capture left the tick at 30 and an orb's transform
  unchanged (checked on the Mac). So the replay stills measure a level in which none of the
  above moves: whatever a still costs, it is not their motion.
  A live run with them frozen is not one switch today (the sliders and bars stop with
  `periodTicks` 0 and `degreesPerSecond` 0 in the scene, but the orbs' bob is a constant in
  the script).
- **Roughness 0.5 is a cliff, and the look pass fell off it.** The look pass put navy paint
  (roughness 0.45) on entry bands and the sliders' tops and coral lacquer (0.25) on pinstripes
  and kerbs the length of every deck. On the RTX the course median went from 8.8 to 10.4 ms
  traced and from 15.0 to 20.1 untraced, all of it in shading. One change at a time on the M5
  (traced path, 720p, 60 held frames, GPU / shading ms, same binary):

  | | Descent | Sliders | Spinner deck | Start |
  |---|---|---|---|---|
  | Before the look pass | 20.5 / 12.8 | 19.8 / 12.5 | 19.4 / 11.9 | 24.6 / 15.7 |
  | After it (navy 0.45, coral 0.25) | 22.9 / 14.8 | 22.0 / 14.5 | 20.3 / 12.7 | 26.0 / 16.9 |
  | Navy at 0.75 | 21.3 / 13.2 | 20.5 / 13.0 | 19.5 / 11.8 | 24.3 / 15.2 |
  | Coral at 0.75 | 21.8 / 13.7 | 21.3 / 13.8 | 19.4 / 11.7 | 25.1 / 16.0 |
  | Both at 0.75 | 20.0 / 11.9 | 19.7 / 12.2 | 18.5 / 10.9 | 23.6 / 14.5 |
  | Both at 0.60 | 20.0 / 11.9 | 19.6 / 12.2 | 18.6 / 11.0 | 23.6 / 14.5 |
  | Both at 0.50 | 20.0 / 11.9 | 19.7 / 12.2 | 18.6 / 10.9 | 23.6 / 14.5 |
  | Great ring unlit, thin beacon | 22.8 / 14.6 | 21.7 / 14.2 | 20.2 / 12.5 | 25.4 / 16.5 |

  So the cost is what the surfaces are: a surface under roughness 0.5 gets a traced glossy
  reflection wherever it is on screen, however faint the result; at 0.5 it gets none and
  costs the same as at 0.75. The lit ring is nearly free away from the goal. Glint's navy and
  coral are now satin at 0.5, like its white paint: the decks cost less than before the look
  pass (navy kerbs were already under the cliff) and the stills cannot be told apart
  (`preview/r8/`). The deliberate mirrors stay: the inlay, the pad discs, chrome, the goal's
  floor. *For the renderer:* a hard step in cost at one roughness, with no visible step in
  the picture, is easy to fall off without knowing; the untraced path pays more for the same
  surfaces (not measurable on the Mac).
- **The rule, and two more things that were on the wrong side of it.** As the renderer threads
  state it for Renderlab 614b15da: a surface under roughness 0.05 is a mirror traced with one
  ray; from 0.05 up to 0.5 it is a glossy lobe whose ray count rises with roughness (about 4
  at 0.05, 10 at 0.2) and is thinned by reflectance; at 0.5 and above nothing is traced. (A
  fade between 0.35 and 0.5 and a lower slope are being tried.) Against that rule, on the M5
  (traced path, 720p, 60 held frames, GPU / shading ms; base twice, within 0.1):

  | | Goal | Start | Descent | Kicker | Spinner deck |
  |---|---|---|---|---|---|
  | Base | 30.6 / 21.8 | 23.6 / 14.5 | 20.1 / 12.0 | 21.7 / 13.8 | 18.6 / 10.9 |
  | Goal floor 0.05 to 0.04 (three runs) | 30.4 / 21.6 | 23.6 / 14.5 | 20.1 / 12.0 | 21.8 / 14.0 | 18.7 / 11.0 |
  | Polished navy stone 0.06 to 0.04 (two runs) | 30.6 / 21.8 | 23.5 / 14.4 | 20.0 / 11.9 | 21.7 / 13.8 | 18.6 / 11.0 |
  | Chrome 0.06 to 0.04 | 29.7 / 20.9 | 22.9 / 13.8 | 19.9 / 11.8 | 20.9 / 13.0 | 18.6 / 10.9 |
  | Every emitter 0.4 to 0.6 (two runs) | 27.3 / 18.5 | 22.2 / 13.1 | 19.0 / 10.9 | 19.8 / 11.9 | 17.9 / 10.3 |
  | Emitters and chrome (shipped, three runs) | 26.3 / 17.6 | 21.5 / 12.4 | 18.8 / 10.7 | 19.0 / 11.1 | 17.8 / 10.1 |

  - **Emitters.** The sample builder's emissive material (`Material::emitter` in
    `crates/ara-samples/src/kit.rs`) is written at roughness 0.4, so every lit strip,
    chevron, stud, band and beacon was also traced as a glossy reflector, under its own
    light where no reflection can be seen. That was the "lit surfaces cost 4.5 ms at the goal"
    of the earlier table: not the light, the gloss. Glint's emitters are now written at 0.6:
    3.3 ms back at the goal and 0.7 to 1.9 on every other view, with no visible change. The
    builder's default followed (0.6 for every sample; Showcase's emissive ball keeps 0.4 as a
    material sample), so Glint no longer overrides it.
  - **Chrome** (posts, hoops, the bridge frame, the portal casing, the cannon barrel) was at
    0.06, a lobe of several rays. At 0.04 it is a one-ray mirror: 0.7 to 0.9 ms back where
    chrome is on screen. At 1:1 the portal casing's reflection is a little crisper
    (`preview/r10/crop_goal_floor.jpg`); elsewhere I cannot see a difference.
  - **The mirror floor and the polished discs do not follow the prediction on the M5.** One
    ray instead of four should have returned about a millisecond on the goal's floor; it
    returned 0.2 at most, and the polished stone nothing. Both also look harder at 0.04 (the
    reflected edges lose their slight blur: the `decide_*` crops in `preview/r10/`), so they
    are left at 0.05 and 0.06. Worth one RTX run before deciding: if it does return a
    millisecond there, the pair is the trade.
