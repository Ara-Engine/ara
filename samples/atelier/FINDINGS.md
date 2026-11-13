# Findings from building Atelier

What got in the way while building a small cross-engine comparison scene as a user of Ara
(`ara-samples`, the `ara` CLI, `ara capture`, Renderlab on the M5 Pro). Each item says what
was tried, what happened and what was done instead. Severity: **P0** blocks the work,
**P1** visibly wrong or slow, **P2** friction. House style: [`../FINDINGS.md`](../FINDINGS.md).
No engine code was changed for any of these.

## P1: visibly wrong or slow

1. **Interior frames cost 105 to 116 ms of GPU on the M5.** `ara capture` at 1280 x 720,
   Renderlab RT, settled (`External.Frame.Gpu`; the Mac was loaded by other threads, load
   average about 12, so these are upper bounds): still 115.6 ms, dolly end 113.2 ms, the
   chrome-sphere close-up 105.4 ms, and the courtyard view 35.1 ms. `External.Pass.Shading`
   is 93 to 104 ms of the interior frames (23.7 ms of the courtyard one). The scene has one
   sun and six local lights; the cost tracks how much of the frame is indoors (the courtyard
   view is a third of it). Draw counts were not measured.
2. **Flat plaster and the ceiling render with hand-wide blotches (not the material; GI is the suspect).**
   Crops: [`preview/defects/ceiling_2x.jpg`](preview/defects/ceiling_2x.jpg) and
   [`far_wall_2x.jpg`](preview/defects/far_wall_2x.jpg) (2x, from the committed still).
   Diagnosis on the M5 (Renderlab RT through the ara-gpu backend; the Mac runs only this
   path, so the non-RT path is untested here):
   - The plaster is a plain material (no texture): flat base colour 0.66/0.62/0.55, then
     0.9 neutral, then 0.85/0.84/0.80, all the same blotches, only their brightness and
     warmth change.
   - The pattern is **deterministic**: `--seed 5` gives a bit-identical image to seed 0 (and
     the same still twice is bit-identical), so it is a fixed spatial pattern of the indirect
     light, not frame noise.
   - **GI cannot be switched off on this backend**, so the (b) and (c) renders were not
     possible: `RENDERLAB_ABLATE=gi` and `ara capture --without global-illumination` both give a
     bit-identical image and an unchanged `IndirectCache` pass (2 ms). `ablated()` is applied
     in Renderlab's wgpu engine (`src/engine/mod.rs`) and I found no call to it in `backend_ara`. Whether the
     blotches vanish with GI off (so are the indirect light) is a question for the PC's
     RT and non-RT paths: render `e_camstill0` with `RENDERLAB_ABLATE=gi`.
   - **It does not converge away.** `ara capture --stability N` plays N live frames and writes
     `<name>.live.png` (N is capped at 600; the plain capture settles 48). The ceiling crop at
     48, 60, 300 and 600 frames ([`ceiling_settle_2x.jpg`](preview/defects/ceiling_settle_2x.jpg))
     has the same blotches; their amplitude (std of the luminance minus a 12 px blur, 0 to 255)
     is 3.74, 3.75, 3.83, 3.85. Frame-to-frame flicker does fall (mean change 0.042 at 60 frames,
     0.010 at 300, 0.007 at 600; 0.1% of 16 x 16 blocks flicker), so the picture is stable
     but the mottle is permanent. GPU 120 ms a frame throughout.
   - **It sticks to the surface, not the screen.** The same 1.5 x 1.35 m patch of the back wall,
     projected from the still camera and from the dolly-end camera and rectified
     ([`wall_two_angles_2x.jpg`](preview/defects/wall_two_angles_2x.jpg)), shows the same blotch
     layout in both. It is a spatial pattern of the world-space indirect light, roughly 10 to
     30 cm across, elongated along the wall.
   - Brightness: at EV100 11.5 the ceiling's display luminance is 0.20 (sRGB 152/118/86) against
     0.76 (250/222/181, partly clipped by the display transform) for the sunlit floor patch,
     about 1:3.7; as scene luminance about 700 against 2,600 cd/m2 or more. The far wall in
     shade is 0.17 and the floor in shade 0.16. At the first EV100 of 13 the same ceiling was 0.056.
   - The brown cast is warm bounce: the indirect light comes off a beige marble floor and the
     terracotta wall, under a warm sun; there is little blue sky fill in the room.
   Not hidden by art: the blotches stay in every render.
3. **No visible particles.** The dust motes (`VisualEffect` "dust" in the front sun shaft)
   do not appear in any Mac capture; the Metal backend's particle support is the known gap
   (`crates/ara-renderlab/AGENTS.md`). Kept in the scene and listed as an engine-side extra.
4. **No relaxed standing idle for the mannequin.** `anim::locomotion` needs two gaits (a single `Walk` fails with `ARA7300 ... 1D blends need at least two samples`),
   and with a controller but no `CharacterController` the mannequin showed a mid-stride pose.
   Using `Animator.idle` with a clip name works, but none of the library's idles is a relaxed
   upright stand: `Idle_A` leans forward on a staggered stance, `Idle_FoldArms` is a wide
   stance, `Idle_Talking` and `Idle_Torch` gesture, `Idle_Rail` is bent over. Atelier stands
   the mannequin (`Idle_Talking`) in the courtyard, in the dolly and cut clips and out of the
   still.

## P2: friction

5. **A scene cannot hold an HDRI.** HDRI skies are expressible only in capture look files, so
   a CC0 equirect cannot be part of the scene document. Atelier uses the `Environment`
   gradient (numbers) and records the same numbers in `compare/atelier.spec.json`.
6. **`Light` has no per-light shadow flag.** Which lights should cast shadows is recorded in
   the spec and README table but cannot be authored; the renderer decides (Ara #140 is the
   draft that adds the flag).
7. **`ara capture --camera` renders one camera entity.** A clip (poses over time) has no
   native path: `--video-move` / `--video-yaw` are a straight move and a turn. Atelier makes
   one camera entity per clip key (`e_camdolly0`, `e_camcut2`, ...) and keeps the full clips as
   `compare/camera_clips.json`; rendering a clip's frames is not automated yet.
8. **`ara-samples fetch` does nothing until the project directory exists**, and says
   `fetched and verified`. `build` creates the project (`ara init`), but the assets must
   already be fetched. First run: `ara init samples/<name>` by hand, then `fetch`.
9. **`fetch` and `add-polyhaven` use `samples/` of the binary's own checkout.** A binary
   built in another worktree writes that worktree's lock and cache; pass `--samples samples`.
10. **Entity IDs must be `prefix_base32`.** `e_camera_prop` and `e_cam_still0` are rejected
    (`ARA1202`) after a whole batch has been assembled; the rule is stated in the error but
    nowhere in the command schema (`entity.create`'s `id` doc says only "a concrete unused
    record ID").
11. **`ara schema` run in a project compiles its C# modules** (a `Salvage compilation`
    info stream on stdout while asking for a component schema). Harmless, but noisy.
12. **Texture repeat comes from the lock's `meters`.** `marble_01` repeats every 0.5 m, so a
    marble floor reads as small tiles; the kit's UV scale was multiplied by three.
13. **The exporter branch is a long way from main.** `origin/t3/unity-compare-export` is two
    commits on an older main (merge base `ee27f3e51`); its three files are additive but the
    bridge hook (`on_frame`) lives in files that have moved. Not run from the Mac: building
    `ara-player` with it in a scratch checkout needs several GB beyond the disk floor.
    What it can carry of Atelier, from reading `unity_export.rs`: geometry, PBR factors and
    textures, alpha blend and mask, emissive (as `KHR_materials_emissive_strength`), the sun
    and local lights (as candela, with Ara's lumens, radius and length in the sidecar), the
    gradient sky as `sky.hdr` (widened to 2:1), the fixed EV100 and the camera. It cannot
    carry: the idle animation (skinned draws are baked at one pose), the particles, a
    per-light shadow flag (it states Ara has none), source radius as a glTF property
    (sidecar only).
14. **A polished floor washes out to near-white in shade (observed, cause not isolated).**
    Same scene, same camera (`e_camstill0`), floor base colour 0.18, EV100 10, AgX: at
    roughness 0.36 the shaded floor in the foreground is sRGB 157/162/174 and 170/177/190;
    at roughness 0.9 the same two pixels are 104/117/141 and 101/111/134. The walls and
    ceiling are unchanged between the two (176/176/181 against 175/176/180). Going from base
    colour 0.3 to 0.18 at roughness 0.36 changed almost nothing, so at that roughness the
    floor's look is its reflection, not its colour. From that camera the mirror direction
    of those pixels is the shaded back wall and ceiling, which are darker than what the
    floor shows, so the glossy term looks too bright; not checked against a reference
    render or another engine. The floor is authored honed (roughness 0.7).
15. **A glowing object is dark in its reflections.** The `glow` still (`e_camglow0`,
    [`preview/glow.jpg`](preview/glow.jpg)) frames the table lamp's globe, an emitter at
    6000 nits in the table's shade, on a polished nickel tray (roughness 0.07) beside a
    chrome sphere (0.03). Seen directly the globe is sRGB 249/244/238. Its reflection in
    the tray is 124/130/139, darker than the tray around it (228/231/237), and in the
    chrome sphere 141/153/170: a grey ball where a glowing one should be. The chrome
    sphere's own reflection in the tray is bright (229/229/227), so the reflections
    themselves work; it is the emitter's light that is missing from them. On Ara main
    `f134c989f` (Renderlab `9ae2929d`). A renderer fix is on its way; this still is the
    check for it.
16. **The sun casts no shadow more than about 40 m from the camera.** The `shadow` still
    (`e_camshadow0`, [`preview/shadow.jpg`](preview/shadow.jpg)) looks at three 14 m masts
    42 to 44 m away, 8 m in front of a wall that faces the sun: no shadow on the gravel or
    the wall. With the camera moved to 25 m from the masts and nothing else changed
    ([`preview/defects/shadow_from_25m.jpg`](preview/defects/shadow_from_25m.jpg)) the
    three shadows are on the wall, sRGB 41/29/34 against 126/86/71 beside them, and on
    the gravel (both stills at the shadow still's own exposure, EV100 12). The same cause as Glint's finding 33 (pylons that cast nothing on the
    clouds); a renderer fix for the sun's shadow range is on its way and this still is the
    check for it. Same build as item 15.

## The look pass of 13 November

The first version read brown and dim: every wall and the floor were warm, the exposure was
a compromise between room and courtyard, and Neutral clipped the sun patches. What changed:

- **Exposure is set for the room** (EV100 11.5 to 10.0) and the sun is 45,000 lux (was
  75,000), so the white plaster in shade displays as light grey-white (sRGB about 176)
  instead of 152/118/86.
- **AgX instead of Neutral.** The sunlit floor is about ten times the walls in shade;
  Neutral turned every patch into flat white.
- **One accent.** Terracotta is one panel behind the sculpture, not a whole wall; the
  weathered-steel planters repeat it outside. Everything else is white, grey, black, walnut.
- **Dark where the sun lands.** The floor (0.18), the plinth (0.04), the outside render
  (0.065), the paving, the gravel and the mannequin are dark, because at the room's
  exposure a pale surface in full sun is several times display white.
- **The west wall has something on it**: a 2.2 x 1.5 m canvas and a walnut console with a
  brass bowl and books; the still now looks at that corner at 46 degrees instead of 56.
- Not fixed: the blotches of item 2 are as visible on white as they were on beige, and the
  sky is nearly white at this exposure.

## Two test stills (13 November, later)

Added for the renderer work on reflections and shadows; the three comparison clips keep
their cameras.

- **`glow`**: the table lamp was a brass sphere with a light inside it and nothing that
  glowed. It is now a glowing globe (0.1 m radius, the bulbs' emitter material) resting on
  a polished nickel tray with a small chrome sphere beside it, all in the table's shade so
  the globe's brightness is its own. The lamp's light moved 0.14 m to the globe's centre
  (0.65, 0.877, -6.03) and its source radius is the globe's (0.1, was 0.03); its 120 lm
  are unchanged.
- **`shadow`**: three masts and a boundary wall south-west of the building, outside every
  other camera's view except the first frames of the dolly, where the wall's end shows left
  of the building. The wall is very dark (0.06, 0.025, 0.014) for the reason the render
  is: at the room's exposure it still displays as a pale terracotta in sun. The still has
  its own exposure, EV100 12, from a 2 m box volume round its camera (`e_shadowev`), so
  the stripes are judged at a sensible level; the room's exposure is untouched and no
  other camera comes within 8 m of the box.
