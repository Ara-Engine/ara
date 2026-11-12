# Exporting Atelier to Unity and Unreal

Read from `origin/t3/unity-compare-export` (`unity_export.rs`, `unity_export_mesh.rs`,
`unity_gltf.rs`): the bridge writes the frame it hands Renderlab as `scene.gltf` (+ `.bin`,
`textures/`) and a `scene.json` sidecar. Atelier's own numbers are in
`atelier.spec.json` and `camera_clips.json` here; they come from the same tables the
scene is built from, so they win wherever the two disagree.

## Run it

```sh
ARA_UNITY_EXPORT=<dir> ara capture --root samples/atelier --scene levels/atelier.scene \
  --camera e_camstill0 --ev100 11.5
```

- `--ev100 11.5` matters: the sidecar's `exposure.ev100Lock` is the capture's locked EV100.
  Atelier's own manual exposure sits in its post-process volume; without the flag the sidecar
  may say `null` ("the bridge meters").
- `e_camstill0` is also the scene's main camera now (priority 0; the other camera entities
  are priority 10), so the exported `camera` is the still.
- The exporter writes one frame (default the third rendered). The mannequin is baked at the
  pose of that frame; Atelier's still does not show the mannequin.

## What it carries (Atelier's contents)

| Atelier | Exported as |
|---|---|
| Room, furniture, courtyard kits and the CC0 models | glTF meshes, one node per draw with its world matrix; double-sided as authored (kits are double-sided, as Renderlab draws them) |
| Marble, concrete, leather-free palette, plants | base colour (tinted factors folded), normal map, metallic-roughness baked into a mask map (R metallic, A smoothness; factors folded), plain PNGs decoded from the cooked textures |
| Window glass, table glass | alpha mode BLEND (opacity < 1), no refraction |
| Plant leaves | alpha MASK from the texture (binary alpha), cutoff from the model |
| Emissive bulbs, lantern | `emissiveFactor` normalised plus `KHR_materials_emissive_strength` (peak, in nits): the model's emissive factor times the entity's `MeshRenderer.emissive` (6000 nits furniture, 5000 courtyard) |
| Sun | a directional glTF light (peak lux as intensity, colour normalised) and the sidecar's `sun` (lux RGB, direction toward the light, angular radius) |
| Local lights | punctual lights in candela (`lumens / 4 pi`), spot cones, range; the sidecar adds lumens, source radius and length |
| Sky | `sky.hdr`, equirectangular Radiance RGBE in cd/m2 (the gradient map, widened to 2:1, 128 px high) and the numbers in the sidecar's `environment` |
| Exposure, tone mapper | the sidecar's `exposure` and `display` blocks |

## What it cannot carry, and what to do about it

1. **Per-light shadow choice.** Ara's `Light` has no shadow flag (Ara #140 adds one), so
   Renderlab shadows the sun and every local light, and the sidecar says so. The spec's
   intended choice (shadows on for the pendant and the floor lamp, off for the rest) cannot be
   in the export. For a fair comparison either shadow every light everywhere, or follow the
   spec in the other engines and accept that Renderlab's picture differs.
2. **Source size as geometry.** glTF has no light radius: the sidecar's `sourceRadius` is the
   only record. Every Atelier light is a sphere (the two tube lights were made spheres in this
   round: Renderlab cannot draw a tube, `ARA_RENDERLAB_UNSUPPORTED tube lights`, so a tube would
   have differed from the render being compared). Unity needs sphere/area lights built from the
   sidecar; the glTF light is a point.
3. **Animation.** Skinned draws are baked at one pose; the mannequin's idle clip does not
   export. Atelier lists it as an engine-side extra.
4. **Particles.** The dust motes are `VisualEffect` entities, not draws (`stats.effects` in the
   sidecar). They do not draw on the Mac either.
5. **Camera clips.** The exporter writes the frame's camera, plus a clip only for
   `--video`-style moves (`ARA_UNITY_EXPORT_CLIP=frames,mx,my,mz,yaw`), which cannot express
   the dolly or the cut. Use `camera_clips.json` (position and rotation keys, `cut`, fov).
6. **Occlusion maps.** Not exported (the ARM maps' red channel); Renderlab ignores them too,
   occlusion comes from traced GI. Unity and Unreal will add their own AO unless it is off.
7. **Units and handedness.** Right-handed, +Y up, metres, cameras look along -Z. Unity builds
   negate Z (X in `atelier.spec.json`'s note: use one convention and say which). Light units:
   Ara stores point-equivalent lumens (4 pi times candela); the glTF carries candela and Unity's
   spot normalisation differs from Unreal's.
8. **Sky resolution and orientation.** The widened gradient is 128 px high, row 0 the zenith,
   "the orientation Renderlab's `Sky::Map` takes"; there is no sun disc in it by design.
9. **The Mac's picture is its own.** Renderlab on the M5 draws neither particles nor tube
   lights; the exported scene is the bridge's frame, not the picture.
