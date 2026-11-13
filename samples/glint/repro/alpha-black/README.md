# Alpha blend draws solid black: a minimal reproducer

An alpha-blended material whose base colour is black (or dark) draws solid black on Renderlab
whatever its alpha, instead of fading in. Found when a contact shadow made of black films
(alpha 0.09 and 0.17) drew as an opaque black disc. Reported from the Glint sample
(Ara #149); nothing here is engine code.

## The scene

`scenes/alpha.scene`, an ordinary Ara project (`ara check` clean), one fixed exposure, one sun,
a mid-grey lit floor, a black stripe under each row so see-through is visible:

| Row (front to back) | What |
|---|---|
| A | three glTF quads, `alphaMode BLEND`, `baseColorFactor` red, blue, white, alpha 0.5 (`content/models/quads.gltf`) |
| B | three `MeshRenderer` primitive planes, `color` alpha 0.5, red, blue, white |
| C | black planes with alpha 0.5, 0.1 and 0.1 with `castShadows` off, then the Glint contact-shadow kit with its two films set to black alpha 0.09 and 0.17 (`content/models/blob_black.gltf`) |
| D | that kit under a point light, and under a point light plus a chrome ball |
| E | primitive grey planes 0.1, 0.5, 0.9 at alpha 0.5 |

```sh
ara capture --root samples/glint/repro/alpha-black --scene scenes/alpha.scene \
  --camera e_camera --width 1280 --height 720 --ticks 30 --path alpha.png
# ARA_RENDERER=renderlab-nonrt for the non-RT path (the Mac cannot start it, see below)
```

## What it shows

[`renderlab-rt-mac.jpg`](renderlab-rt-mac.jpg) (whole scene) and
[`renderlab-rt-mac-crop.jpg`](renderlab-rt-mac-crop.jpg) (rows C and A at 2x).

- **Path: Renderlab RT on the Mac's ara-gpu (Metal) backend**, the only Renderlab path the M5
  can start (`capture.scene_hash` reads `renderlab-rt`). `ARA_RENDERER` unset, `renderlab-rt`
  and `renderlab` give the same picture. `ARA_RENDERER=renderlab-nonrt` fails with
  `ARA_RENDERLAB_UNAVAILABLE: Renderlab's Metal backend has the hardware-ray path only`; the
  non-RT picture has to come from the PC.
- Rows A and B: red, blue and white at alpha 0.5 blend, and the black stripe shows through
  (the red quad is 62 green over the stripe, 74 over the floor).
- Row C: **black at alpha 0.5, 0.1 and 0.1 are all the same solid near-black** (43, 41, 43), the
  same over the stripe as over the floor, with or without `castShadows`; the contact-shadow
  kit is the same black disc. Rows D: the same under a light and a ball.
- Row E: the grey ramp shows the rule: at alpha 0.5, grey 0.1 hides the stripe completely
  (96 over the stripe, 98 over the floor), grey 0.5 hides most of it (202, 210), grey 0.9 almost
  none (234, 233). How much of the background passes depends on the base colour's brightness,
  not on the alpha.

## Where the black comes from

Not an unmapped blend mode and not alpha going to the wrong field. The bridge maps a blended
draw correctly for Renderlab's API: `blends()` in `crates/ara-renderlab/src/scene.rs` is true for
`alpha_mode == 2` with an alpha factor below one, the alpha stays in `desc.base_color[3]`
(Renderlab multiplies it into coverage), and `Scene::material` declares the missing pane:
`if blend && desc.transparency.is_none() { desc.transparency = Some(Transparency { opacity: 1.0,
ior: 1.0, thickness: 0.0 }) }`. The black is in Renderlab's own transparent pass: for the
standard program it adds the surface's shade on top of the light behind it multiplied by
`transmittance()`, defined in Renderlab's `shaders/programs/standard.wgsl` as
`(1.0 - s.alpha) * (1.0 - fresnel) * s.base`. The light behind the pane is multiplied by the
**base colour** (a tinted-glass filter), so a base colour of 0 passes none of it at any alpha,
and the pane's own shade of a black surface is black: the result is opaque black. glTF, Unity
and Unreal define alpha blend as `alpha * colour + (1 - alpha) * behind`; Renderlab's standard
program defines it as coloured glass, and the two agree only for near-white colours. Whoever
owns the fix chooses: a different `transmittance` for plain blended materials in the standard
program (for example `mix(vec3(1), s.base, s.alpha)` times `(1 - fresnel)`), or the bridge attaching
a program of its own with glTF semantics for materials that are not meant to be glass. It also
darkens everything seen through any slightly tinted blend (Atelier's window glass passes about
78 percent of the light behind it on Renderlab where glTF alpha 0.14 passes 86 percent).
