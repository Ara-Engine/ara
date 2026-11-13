# Atelier

A small, clean comparison scene: a white-plaster studio with one terracotta panel and a dark concrete floor, lit by
one sun through a 4.4 x 3.4 m doorway, a 5.6 x 3.2 m east window and a roof slot, opening
onto a concrete courtyard with a few plants; the building's outside is charcoal render. Built to be measured in several engines, so
everything the comparison depends on is a plain number or plain glTF.

Authored by `crates/ara-samples/src/build/atelier/` (`ara-samples build atelier`). Its
tables are the source of `compare/atelier.spec.json` and `compare/camera_clips.json`; the
scene is built from the same tables, so the files and the scene cannot drift.

```sh
ara init samples/atelier --name Atelier        # first checkout only: fetch needs the directory
ara-samples fetch atelier                      # ~100 MB of pinned CC0 files
ara-samples build atelier
ara capture --root samples/atelier --scene levels/atelier.scene --camera e_camstill0 --ticks 90
```

Camera entities: `e_camstill0`, `e_camdolly0`/`1`, `e_camcut0`..`3`, `e_camglow0`, `e_camshadow0` (one per clip key).
Export notes (what the Unity/Unreal export carries and cannot): [`compare/EXPORT.md`](compare/EXPORT.md). `preview/` holds the current stills (JPEG, replaced as the scene changes).

## Conventions

Right-handed, +Y up, metres. Cameras look along their local -Z (glTF). The room runs
toward -Z from its front wall at z = 0; the courtyard is on +Z. Left-handed engines
(Unity): negate X on import and flip triangle winding. Light travel direction is the
direction the light moves, not where it comes from.

## Exposure and display

| | |
|---|---|
| Exposure | fixed, **EV100 10.0**, set for the room (walls in shade read as white), no auto-exposure, no compensation (photometric `H = L / (1.2 * 2^EV100)`) |
| Tone mapping | AgX, Medium High Contrast look, in Ara (the sunlit floor is about ten times the walls in shade, and Neutral clipped it to flat white); for comparisons use linear HDR |
| The `shadow` still | **EV100 12.0**, its own: it looks at the sunlit exterior, which the room's exposure washes out. In the scene a 2 m box volume round its camera (`e_shadowev`) sets it; `camera_clips.json` gives every clip its `ev100` |
| Post | no bloom, vignette, grading, fog, decals or material graphs |

## Sky and sun

| | |
|---|---|
| Sky | gradient equirect, no sun disc: zenith (0.2, 0.36, 0.85), horizon (0.72, 0.82, 0.95), ground (0.4, 0.4, 0.39), falloff 3, scale 9000 cd/m2 |
| Sun | 45,000 lux on a surface facing it, colour (1.0, 0.91, 0.76), travel direction (-0.45, -0.438, -0.78) (about 26 deg elevation), angular diameter 0.5357 deg, casts shadows |

## Local lights

All in lumens, all spheres with an explicit source radius (Ara's default of 0 is an ideal point; the two tube lights were made spheres because Renderlab cannot draw tubes).
Spots: cone half-angles in degrees. "Shadow" is the intended choice; Ara's `Light` has no
shadow flag yet (see FINDINGS).

| Light | Type | Position (m) | Lumens | Colour | Radius / length (m) | Range (m) | Cone | Shadow |
|---|---|---|---|---|---|---|---|---|
| Pendant over the table | point | 0.2, 2.5, -6.0 | 900 | 1.0, 0.82, 0.6 | 0.10 | 9 | | yes |
| Floor lamp reading spot | spot | -3.4, 1.7, -5.7 | 450 | 1.0, 0.8, 0.58 | 0.04 | 6 | 25 / 45 | yes |
| Picture light | spot | 1.6, 2.65, -8.7 | 350 | 1.0, 0.9, 0.75 | 0.02 | 3.5 | 30 / 55 | no |
| Shelf LED strip | point | -2.4, 1.2, -8.65 | 300 | 1.0, 0.93, 0.8 | 0.01 | 3 | | no |
| Courtyard wall lantern | point | 3.3, 2.4, 0.68 | 600 | 1.0, 0.78, 0.5 | 0.07 | 8 | | no |
| Table lamp glow | point | 0.65, 0.877, -6.03 | 120 | 1.0, 0.8, 0.55 | 0.10 | 3 | | no |

The table lamp is a glowing globe of 0.1 m radius on the table; its light sits at the globe's centre with the globe's radius.

## Materials (all metallic-roughness)

| Material | Base colour | Rough | Metal | Notes |
|---|---|---|---|---|
| White plaster (interior walls, ceiling) | 0.85, 0.84, 0.8 | 0.9 | 0 | plain, untextured |
| Terracotta plaster (one panel on the west wall, z -3.8..-1.0) | 0.55, 0.22, 0.12 | 0.88 | 0 | the one accent colour; the doorway sun lands on it |
| Concrete floor (2 x 1.8 m tiles, dark joints 0.05) | 0.18, 0.176, 0.17 | 0.7 | 0 | honed, not polished: see FINDINGS 14 |
| Charcoal render (2 cm outer skin, courtyard walls) | 0.065, 0.062, 0.06 | 0.9 | 0 | dark so it keeps modelling outside at the room's exposure |
| Courtyard stone | concrete_floor_02 x (0.34, 0.33, 0.31) | texture | texture | |
| Dark gravel (ground) | 0.2, 0.19, 0.18 | 0.95 | 0 | |
| Boundary wall (48 x 7 m, its sunlit face at z = -44) | 0.06, 0.025, 0.014 | 0.9 | 0 | very dark so a shadow on it reads at the room's exposure |
| Mast paint (three 14 m masts at z = -36, x = -22, -16, -10; 0.4, 0.2, 0.1 m across) | 0.012, 0.012, 0.013 | 0.6 | 0 | |
| Weathered steel (planters) | 0.13, 0.055, 0.03 | 0.85 | 0 | repeats the terracotta outside |
| Black steel (window frame, bench legs, lantern) | 0.05, 0.05, 0.055 | 0.42 | 1 | |
| Blackened steel (table legs, picture frames, console legs) | 0.07, 0.07, 0.075 | 0.4 | 1 | |
| Window glass, table glass | tint 0.88-0.9, 0.96, 1.0; alpha 0.14-0.16 | 0.04 | 0 | alpha blend, no texture |
| Chrome (the sculpture's sphere, 0.33 m radius; a 0.075 m sphere on the table's tray) | 0.95 | 0.03 | 1 | mirror-like |
| Polished nickel (0.65 x 0.54 m tray on the table) | 0.72, 0.7, 0.66 | 0.07 | 1 | a near-mirror, under the glowing globe |
| Brushed brass | 0.93, 0.72, 0.36 | 0.28 | 1 | |
| Walnut (table top, console) | 0.3, 0.18, 0.1 | 0.4 | 0 | |
| Graphite leather (armchair) | 0.09, 0.09, 0.1 | 0.45 | 0 | |
| Black stone (plinth) | 0.04, 0.04, 0.045 | 0.4 | 0 | in full sun it reads mid-grey at this exposure |
| Dark oak, teak, oatmeal rug (0.66, 0.6, 0.5), books (cream, charcoal, tan, terracotta), canvas (white, charcoal, oatmeal, terracotta) | flat colours | 0.5-1.0 | 0 | |
| Emissive bulbs and lantern glass | 6000 / 5000 nits x (1.0, 0.8, 0.55) | | | KHR_materials_emissive_strength |

## Assets

All CC0, pinned in `samples/assets.lock.json` (sources and licences in
[THIRD_PARTY_ASSETS.md](../THIRD_PARTY_ASSETS.md)): Poly Haven texture `concrete_floor_02`
(`marble_01` is still fetched with the group but the floor no longer uses it); Poly Haven models `potted_plant_01`,
`potted_plant_02`, `fern_02`, `shrub_04`, `lantern_01`, `vintage_radio_transceiver`;
Khronos `FlightHelmet` and `AntiqueCamera`; the Mesh2Motion mannequin (a mid-grey clay suit, 0.3, 0.29, 0.27). The room, furniture
and courtyard are generated glTF kits (`content/kit/`, git-ignored).

## Camera clips

`compare/camera_clips.json`: 30 fps, 1920 x 1080, position linear and rotation slerp
between keys, a key with `cut: true` ends its segment.

| Clip | Length | What |
|---|---|---|
| `still` | 1 frame | interior from the doorway's east side toward the south-west corner (3.1, 1.5, -0.6 looking at -1.5, 1.6, -5.0, 46 deg): terracotta panel, chrome sphere, large canvas and console, armchair, table |
| `dolly` | 8 s | push from the courtyard through the doorway toward the table |
| `cut` | 6 s | 3 s low courtyard pan, hard cut to the chrome sphere |
| `glow` | 1 frame | test still at the table's end, in shade (1.85, 1.3, -5.7 looking at 0.8, 0.8, -6.13, 34 deg): the glowing globe on the nickel tray beside the small chrome sphere, so one emitter is seen directly, in a near-mirror (roughness 0.07) and in chrome (0.03) |
| `shadow` | 1 frame | test still from the gravel west of the courtyard (-8.5, 1.29, 6.0 looking at -17.5, 5.0, -44.0, 26 deg): three masts 42 to 44 m from the camera, whose shadows cross the gravel and climb the boundary wall 50 to 54 m away as stripes 0.46, 0.23 and 0.12 m wide; exposed at EV100 12 |

## Engine-side extras (do not export)

The mannequin (stands by the courtyard bench at -4.7, 0, 3.3, turned toward the doorway; seen in the dolly and cut clips, not the still) and its idle animation (skinned meshes are baked at one pose by the exporter), the
dust particles in the sun shaft, and per-light shadow choices (see FINDINGS).
