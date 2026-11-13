# Exporting Glint to Unity and Unreal

Glint is the third scene of the three-engine row: an outdoor level in hard low sun, with a
cloud deck, emissive markings, a chrome ball and glass. Its numbers are in
[`glint.spec.json`](glint.spec.json) and its cameras in
[`camera_path.json`](camera_path.json); both are written by `ara-samples build glint` from the
tables the scene is built from, so they win wherever an export disagrees.

What the exporter does is *from reading* `origin/t3/unity-compare-export`
(`crates/ara-renderlab/src/unity_export.rs`); nothing was exported from here.

## Run it

```sh
ARA_UNITY_EXPORT=<dir> ara capture --root samples/glint --scene levels/glint.scene \
  --camera e_camover --ev100 13.5
```

- `--ev100 13.5` puts the scene's fixed exposure into the sidecar's `exposure.ev100Lock`.
- The exporter writes one frame (by default the third rendered). Use a review camera
  (`e_camover`, `e_camturn`, `e_camsliders`, `e_camcannon`, `e_camgoal`); the game's own camera
  follows the ball.

## Cameras

`camera_path.json` has the five review stills (one clip each, duration 0) and `flyover`, one
fixed 24 s pass through them in route order (position linear, rotation slerp, 30 fps). The
exporter's own clip (`ARA_UNITY_EXPORT_CLIP`) can only move and yaw at a constant rate, so the
fly-over has to be built from this file in each engine.

## What it carries

| Glint | Exported as |
|---|---|
| Decks, pylons, rings, arches, the great arch, the cannon and its gantry, goal, islands | glTF meshes, one node per draw with its world matrix; flat base colour, roughness and metallic factors (no textures in this level) |
| Chevrons, ring bands, orbs, goal lights, beacon | `emissiveFactor` plus `KHR_materials_emissive_strength` in nits: 45,000 to 90,000 (table in the spec) |
| Glass rails (launch deck, banked turn) | `KHR_materials_transmission` with the IOR (1.5) and thickness (0, a thin wall) in the sidecar |
| Chrome (ball, posts) and the polished navy inlay | metallic 1 roughness 0.06, and dielectric roughness 0.06 |
| Sun | a directional light (95,000 lux) and the sidecar's `sun` |
| Checkpoint and goal lights | point lights in candela (`lumens / 4 pi`), range, source radius in the sidecar |
| Sky | `sky.hdr` (the gradient, in cd/m2) and the sidecar's `environment` |
| Height fog | numbers only, in the sidecar's `heightFog` |
| Exposure, tone mapper | the sidecar's `exposure` and `display` |

## What it cannot carry, and what to do about it

1. **The cloud deck's softness.** The clouds are plain matte meshes (106 instances of three
   meshes); what turns them into a sea is the height fog, thick at the deck (density 0.012 at
   y = -61 m, scale height 15 m) and thin at the course. The export carries the meshes and
   the fog's numbers, not the fog. Each engine needs an exponential height fog set from the
   sidecar, or its cloud deck will look harder than Renderlab's.
2. **The sun's disc.** `e_sundisc` is a lit sphere (600,000 nits, 52 m, 2.4 km away, placed
   2.6 degrees above the true direction). glTF has no cast-shadow flag: in another engine it
   will shade whatever lies behind it unless its shadow casting is turned off by hand, and an
   engine that draws its own sun disc will show two. Delete the node or switch the engine's
   own disc off.
3. **Motion.** The sliders, spinner bars, orbs (bobbing and turning) and the ball are driven
   by C# from the tick; the export is one frozen frame of them. The comparison is of stills
   and a camera fly-over, not of the game running.
4. **Instancing.** The orbs (43), rings (7) and islands are instances of one mesh each in Ara,
   the cloud banks (106) of three. *From reading*, the exporter writes one node per draw; whether nodes
   share a mesh was not checked. Draw counts in the other engines may differ from Ara's.
5. **Emissive against a bright sun.** The emissive reads because bloom is on (0.16) and the
   exposure is fixed; an engine with auto-exposure or a different bloom threshold will show
   the chevrons flat. Lock exposure at EV100 13.5 and set emissive in nits.
6. **Transmission glass.** The factor and IOR export; how each engine draws a thin transmissive
   wall (refraction, tinted shadow) is its own. The rails are small in every view.
7. **Per-light shadow choice** (Ara #140): Renderlab shadows every light; so should the others.
8. **Below the horizon.** The camera looks down through gaps in the cloud at the sky's `ground`
   colour (0.72, 0.84, 1.0). If an engine's sky is the upper hemisphere of `sky.hdr` only,
   those gaps will be black or its own ground colour.
9. **Particles, UI and sound** are not in the export. Particles (the spark trail) do not draw
   on the Mac either.
10. **Units and handedness**: as in Atelier's notes (`../../atelier/compare/EXPORT.md`, item 7):
    right-handed, +Y up, metres, cameras look along -Z; choose one mirror conversion for every
    engine and state it.
