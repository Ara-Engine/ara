# Atelier

A small, clean comparison scene: a white-plaster studio with one terracotta wall, lit by
one sun through a 4.4 x 3.4 m doorway, a 5.6 x 3.2 m east window and a roof slot, opening
onto a limestone courtyard with a few plants. Built to be measured in several engines, so
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

Camera entities: `e_camstill0`, `e_camdolly0`/`1`, `e_camcut0`..`3` (one per clip key).
`preview/` holds the current stills (JPEG, replaced as the scene changes).

## Conventions

Right-handed, +Y up, metres. Cameras look along their local -Z (glTF). The room runs
toward -Z from its front wall at z = 0; the courtyard is on +Z. Left-handed engines
(Unity): negate X on import and flip triangle winding. Light travel direction is the
direction the light moves, not where it comes from.

## Exposure and display

| | |
|---|---|
| Exposure | fixed, **EV100 12.5**, no auto-exposure, no compensation (photometric `H = L / (1.2 * 2^EV100)`) |
| Tone mapping | Khronos PBR Neutral in Ara; for comparisons use linear HDR or each engine's neutral |
| Post | no bloom, vignette, grading, fog, decals or material graphs |

## Sky and sun

| | |
|---|---|
| Sky | gradient equirect, no sun disc: zenith (0.2, 0.36, 0.85), horizon (0.72, 0.82, 0.95), ground (0.28, 0.26, 0.23), falloff 3, scale 9000 cd/m2 |
| Sun | 75,000 lux on a surface facing it, colour (1.0, 0.91, 0.76), travel direction (-0.45, -0.438, -0.78) (about 26 deg elevation), angular diameter 0.5357 deg, casts shadows |

## Local lights

All in lumens, all with an explicit source radius (Ara's default of 0 is an ideal point).
Spots: cone half-angles in degrees. "Shadow" is the intended choice; Ara's `Light` has no
shadow flag yet (see FINDINGS).

| Light | Type | Position (m) | Lumens | Colour | Radius / length (m) | Range (m) | Cone | Shadow |
|---|---|---|---|---|---|---|---|---|
| Pendant over the table | point | 0.2, 2.5, -6.0 | 900 | 1.0, 0.82, 0.6 | 0.10 | 9 | | yes |
| Floor lamp reading spot | spot | -3.4, 1.7, -5.7 | 450 | 1.0, 0.8, 0.58 | 0.04 | 6 | 25 / 45 | yes |
| Picture light | spot | 1.6, 2.65, -8.7 | 350 | 1.0, 0.9, 0.75 | 0.02 / 0.6 tube | 3.5 | 30 / 55 | no |
| Shelf LED strip | point | -2.4, 1.2, -8.65 | 300 | 1.0, 0.93, 0.8 | 0.01 / 1.4 tube | 3 | | no |
| Courtyard wall lantern | point | 3.3, 2.4, 0.68 | 600 | 1.0, 0.78, 0.5 | 0.07 | 8 | | no |
| Table lamp glow | point | 0.75, 0.98, -6.0 | 120 | 1.0, 0.8, 0.55 | 0.03 | 3 | | no |

## Materials (all metallic-roughness)

| Material | Base colour | Rough | Metal | Notes |
|---|---|---|---|---|
| Cream plaster (walls, ceiling, courtyard walls) | 0.66, 0.62, 0.55 | 0.9 | 0 | |
| Terracotta plaster (west wall) | 0.55, 0.22, 0.12 | 0.88 | 0 | colour-bounce source |
| Marble floor | marble_01 x (0.96, 0.92, 0.85) | texture | texture | 1.5 m texture repeat |
| Courtyard stone | concrete_floor_02 x (0.78, 0.74, 0.68) | texture | texture | |
| Packed earth (ground) | 0.33, 0.28, 0.22 | 0.95 | 0 | |
| Black steel (frames, legs) | 0.05, 0.05, 0.055 | 0.42 | 1 | |
| Window glass, table glass | tint 0.88-0.9, 0.96, 1.0; alpha 0.14-0.16 | 0.04 | 0 | alpha blend, no texture |
| Chrome sphere | 0.95 | 0.03 | 1 | mirror-like |
| Brushed brass | 0.93, 0.72, 0.36 | 0.28 | 1 | |
| Brushed steel | 0.78, 0.79, 0.8 | 0.34 | 1 | |
| Red lacquer (table) | 0.5, 0.03, 0.025 | 0.1 | 0 | glossy |
| Leather (armchair) | fabric_leather_02 x (0.55, 0.36, 0.24) | texture | texture | |
| Limestone (plinth, planters) | 0.8, 0.77, 0.7 | 0.45 | 0 | |
| Dark oak, teak, books, rug, canvas | flat colours | 0.5-1.0 | 0 | |
| Emissive bulbs and lantern glass | 6000 / 5000 nits x (1.0, 0.8, 0.55) | | | KHR_materials_emissive_strength |

## Assets

All CC0, pinned in `samples/assets.lock.json` (sources and licences in
[THIRD_PARTY_ASSETS.md](../THIRD_PARTY_ASSETS.md)): Poly Haven textures `marble_01`,
`concrete_floor_02`, `fabric_leather_02`; Poly Haven models `potted_plant_01`,
`potted_plant_02`, `fern_02`, `shrub_04`, `lantern_01`, `vintage_radio_transceiver`;
Khronos `FlightHelmet` and `AntiqueCamera`; the Mesh2Motion mannequin. The room, furniture
and courtyard are generated glTF kits (`content/kit/`, git-ignored).

## Camera clips

`compare/camera_clips.json`: 30 fps, 1920 x 1080, position linear and rotation slerp
between keys, a key with `cut: true` ends its segment.

| Clip | Length | What |
|---|---|---|
| `still` | 1 frame | interior from the east side: sun patches, chrome sphere, table, armchair |
| `dolly` | 8 s | push from the courtyard through the doorway toward the table |
| `cut` | 6 s | 3 s low courtyard pan, hard cut to the chrome sphere |

## Engine-side extras (do not export)

The mannequin's idle animation (skinned meshes are baked at one pose by the exporter), the
dust particles in the sun shaft, and per-light shadow choices (see FINDINGS).
