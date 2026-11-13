# Atelier: what it asks the renderer to trace

Every surface in `levels/atelier.scene` whose roughness is under 0.5, the value at which
Renderlab stops tracing a glossy reflection (under 0.05 a one-ray mirror; between the two a
lobe that, from Renderlab 99f5d395, fades out between 0.35 and 0.5). Read from the kit glTF the
builder writes and from the third-party models as fetched; the previews are shot on Ara main
`f134c989f` (Renderlab `9ae2929d`).

## Set by a number

| Surface | Roughness | Metalness | Rule |
|---|---|---|---|
| Chrome sphere on the plinth (0.33 m radius) | 0.03 | 1 | one-ray mirror |
| Chrome sphere on the table's tray (0.075 m radius) | 0.03 | 1 | one-ray mirror; 0.26 m from the glowing globe |
| Polished nickel tray on the table (0.65 x 0.54 m) | 0.07 | 1 | glossy lobe at its sharp end: a near-mirror, under the glowing globe |
| Polished nickel plate behind the courtyard wall lantern (0.6 x 0.6 m, on the front wall) | 0.07 | 1 | the same near-mirror, in daylight: it shows the lantern's glowing glass 0.25 m in front of it. **On today's main (`f134c989f`) the reflection shows the lantern almost unlit**: glass 186/174/158 seen directly, 49 to 59 grey in the plate, no brighter than the plate beside it (58). That is the "before" |
| Window glass (5.6 x 3.2 m pane) | 0.04 | 0 | alpha blend at 0.14 opacity, not transmission |
| Clear glass (the objects on the table) | 0.04 | 0 | alpha blend at 0.16 opacity, not transmission |
| Brushed brass (bowl, pendant shade, floor lamp, the disc under the sculpture) | 0.28 | 1 | glossy lobe |
| Walnut (table top, console) | 0.40 | 0 | glossy lobe, in the fade |
| Blackened steel (table legs, picture frames, console legs) | 0.40 | 1 | glossy lobe, in the fade |
| Black stone plinth | 0.40 | 0 | glossy lobe, in the fade |
| Black steel (window frame and mullions, bench legs, lantern frame) | 0.42 | 1 | glossy lobe, in the fade |
| Graphite leather (armchair) | 0.45 | 0 | glossy lobe, near the end of the fade |

Not traced, at or above the cut-off: **the concrete floor (0.70)**, dark oak (0.50), teak
(0.60), the warm bulbs, the table's glowing globe and the lantern's glass (emitters, 0.60),
the masts' paint (0.60), plaster, render and the boundary wall (0.88 to 0.90), the rug, books and canvases (0.70 to 1.0), gravel, soil, weathered steel, and the
mannequin's clay suit (a material graph: roughness 1, metalness 0).

## Set by a texture

Per texel from a metallic-roughness texture (factors 1.0), sampled at 256 x 256.

| Surface | Mean | Lowest | Under 0.5 | Under 0.05 | Metal (mean) |
|---|---|---|---|---|---|
| Antique camera body | 0.18 | 0.00 | 99% | 18% | 0.18 |
| Flight helmet lenses (transmission) | 0.24 | 0.10 | 100% | 0% | 0.00 |
| Potted plant 02 leaves (alpha mask) | 0.39 | 0.30 | 98% | 0% | 0.00 |
| Antique camera tripod | 0.40 | 0.00 | 93% | 1% | 0.40 |
| Lantern brass, and its panes (alpha blend) | 0.36 | 0.07 | 85% | 0% | 0.63 |
| Flight helmet glass and plastic | 0.43 | 0.05 | 67% | 0% | 0.00 |
| Flight helmet leather | 0.49 | 0.24 | 64% | 0% | 0.00 |
| Shrub 04 (alpha mask) | 0.46 | 0.28 | 62% | 0% | 0.00 |
| Flight helmet hose, rubber and wood | 0.49 | 0.22 | 52% | 0% | 0.06 |
| Radio transceiver (two materials) | 0.50 | 0.14 | 48 to 51% | 0% | 0.06 to 0.25 |
| Flight helmet metal parts | 0.53 | 0.17 | 37% | 0% | 0.66 |
| Potted plant 02 pot | 0.54 | 0.24 | 30% | 0% | 0.00 |
| Potted plant 01 pot | 0.57 | 0.31 | 18% | 0% | 0.00 |
| Courtyard stone (14 x 8.6 m paving) | 0.52 | 0.49 | 14% | 0% | 0.00 |
| Potted plant 01 leaves (alpha mask) | 0.55 | 0.39 | 5% | 0% | 0.00 |
| Fern 02 (alpha mask) | 0.94 | 0.44 | 0% | 0% | 0.00 |

## The night scene: what it asks of the lights

`levels/atelier_night.scene`, the same geometry and materials after dark. **Not yet a valid
measure**: until the renderer honours `castShadows: false`, every light below is blocked by
its own fitting and the frame is lit by glowing glass alone (FINDINGS 17).

| Lights | Count | Kind | Each |
|---|---|---|---|
| Wall lanterns: the day's courtyard lantern and its twin on the building's front wall, 18 on posts along the courtyard walls | 20 | point, shadowed | 600 lm, colour 1.0, 0.78, 0.5, source radius 0.07 m, range 8 m |
| The room's lamps, as by day (pendant, shelf strip, table globe: point; floor lamp, picture light: spot) | 5 | 3 point, 2 spot, shadowed | 120 to 900 lm |
| Moon | 1 | directional, shadowed | 0.25 lux |

That is **25 shadowed local lights, 23 of them point lights**, about 12,000 lm over a 120 m2
courtyard, at EV100 5. Ara's `Light` has no shadow switch, so "shadowed" is the renderer's
automatic choice: every local light within its budget (128), which at 25 is all of them.
On the path without ray hardware a shadowed point light takes six layers of the local
shadow atlas where a spot takes one tile, so this scene asks for the six-layer atlas (about
384 MiB against 64 MiB at today's fixed size, by the renderer unit's inventory).

**Fittings that cast no shadow** (`MeshRenderer.castShadows: false`), in both scenes,
because each encloses its own light:

| Model | Meshes | Light inside |
|---|---|---|
| `atelier_lamps` | the table lamp's globe; the pendant's brass shade and bulb; the floor lamp's bulb | Table lamp glow; Pendant over the table; Floor lamp reading spot |
| `atelier_lantern_glass` | the courtyard wall lantern's glass | Courtyard wall lantern |
| `atelier_night_glass` (night only) | the nineteen night lanterns' glass | Night lantern 0 to 18 |

The lanterns' steel frames, the lamps' stems and the pendant's cord still cast shadows. The
glass and bulbs still glow (5000 and 6000 nits), so once the lamps light the room the glow
adds to them through the indirect light; the lamp intensities have not been rebalanced for
that and will be looked at when it can be seen.

## What this means for a reflection test

- **The floor is not a reflector.** It was polished concrete at roughness 0.36 until the look
  pass of 13 November; at that value it washed out to near-white in shade whatever its colour
  (FINDINGS 14), so it was authored honed at 0.70, which the renderer does not trace. The
  room's largest surface therefore asks for no reflection. If Atelier is to be a reflection
  test, the floor is the surface to bring back under 0.5 once that wash-out is understood;
  at 0.36 it was a full glossy lobe on about a third of the still.
- **The glass is a blended film.** The window pane and the glass on the table are alpha-blended
  at roughness 0.04, not transmission glass with an index of refraction.
- **An emitter in two reflectors.** The `glow` still frames the table lamp's globe (an
  emitter, in shade) on the nickel tray beside the small chrome sphere: the same glowing
  object directly, in a glossy lobe at roughness 0.07 and in a one-ray mirror.
- **An emitter in a reflector in daylight.** The courtyard wall lantern (an emitter at 5000
  nits on a base colour of 0.05, so in full sun its own light is about nine times what it
  reflects) has a polished nickel plate on the wall behind it, and the `lantern` still
  looks at the two from above and to the right, where the lantern's reflection sits beside
  the lantern. The indoor globe is the same test in shade. Nothing else near the lantern
  would show it: the doorway is an open hole, the east window's pane is round the corner,
  and the frame, the planter's leaves and the courtyard stone are all faint lobes
  (roughness 0.42 to 0.55).
- **What does reflect** is small on screen: the chrome sphere (a one-ray mirror), brass, the
  steel frames and legs, the walnut table top and console, the plinth and the armchair, most
  of them inside the fade.
