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
2. **Flat surfaces show low-frequency mottling.** Plaster walls, the ceiling and the
   courtyard ground are flat-colour materials but render with soft blotches (visible in every
   still; strongest on the ceiling and the packed earth). Headless captures settle 16 frames
   (`settle_frames: 16` in the capture JSON). Not tried yet: a longer settle or the windowed
   path. It would read as noise in any engine-to-engine comparison.
3. **No visible particles.** The dust motes (`VisualEffect` "dust" in the front sun shaft)
   do not appear in any Mac capture; the Metal backend's particle support is the known gap
   (`crates/ara-renderlab/AGENTS.md`). Kept in the scene and listed as an engine-side extra.
4. **The mannequin has no neutral idle through the locomotion helper.** `anim::locomotion`
   needs two gaits (a single `Walk` fails with `ARA7300 ... 1D blends need at least two
   samples`); with two, a mannequin with no `CharacterController` showed a mid-stride pose
   in the capture, not the idle (cause not investigated; the controller's `speed`
   parameter is fed by `CharacterMotion`, which it does not have). Used instead: `Animator.idle` set to a
   clip name of the model (`Idle_FoldArms`), which plays correctly and needs no controller
   document.

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
