# Showcase: what it asks the renderer to trace

Every material in `levels/showcase.scene` whose roughness is under 0.5, the value at which
Renderlab stops tracing a glossy reflection for a surface (under 0.05 it traces a one-ray
mirror; between the two a lobe whose ray count rises with roughness and, from 99f5d395, fades
out between 0.35 and 0.5). Read from the kit glTF the builder writes and from the third-party
models as fetched, on Ara main `4ea696bf2`.

## Set by a number (no roughness texture)

| Surface | Roughness | Metalness | Rule |
|---|---|---|---|
| Clear glass ball | 0.00 | 0 | transmission (IOR 1.5, 0.6 m thick) |
| Tinted glass ball | 0.00 | 0 | transmission (IOR 1.5, 0.6 m thick, green attenuation) |
| Camera lens (`camera_01`, `Camera_01_lens`) | 0.00 | 0 | transmission |
| Chrome ball | 0.04 | 1 | one-ray mirror |
| Glossy red plastic ball | 0.12 | 0 | glossy lobe |
| Gold ball | 0.18 | 1 | glossy lobe |
| Copper ball | 0.30 | 1 | glossy lobe |
| Turntable (1.6 m radius, under the mannequin) | 0.35 | 0 | glossy lobe, at the start of the fade |
| Satin white plastic ball | 0.38 | 0 | glossy lobe, in the fade |
| Emissive ball | 0.40 | 0 | glossy lobe, in the fade; kept on purpose (a material sample) |
| Brushed aluminium ball | 0.42 | 1 | glossy lobe, in the fade |

At or above the cut-off, so not traced: skin-like ball (0.50), plinths (0.55), black rubber
(0.85), velvet (0.90), the mannequins' suit (a material graph: roughness 1, metalness 0).

## Set by a texture

The roughness and metalness of these come per texel from a metallic-roughness texture
(factors 1.0). Sampled at 256 x 256: mean roughness, the lowest texel, the share of texels
under 0.5 and under 0.05, and the mean metalness.

| Surface | Mean | Lowest | Under 0.5 | Under 0.05 | Metal (mean) |
|---|---|---|---|---|---|
| Wood ball | 0.25 | 0.08 | 100% | 0% | 0.01 |
| Boombox speakers (alpha mask) | 0.25 | 0.20 | 100% | 0% | 0.52 |
| Flight helmet lenses (transmission) | 0.24 | 0.10 | 100% | 0% | 0.00 |
| Antique camera body | 0.18 | 0.00 | 99% | 18% | 0.18 |
| Rusted steel ball | 0.42 | 0.38 | 95% | 0% | 0.00 |
| Antique camera tripod | 0.40 | 0.00 | 93% | 1% | 0.40 |
| Binoculars, and their glass (alpha blend) | 0.30 | 0.05 | 90% | 0% | 0.07 |
| Lantern brass, and its panes (alpha blend) | 0.36 | 0.07 | 85% | 0% | 0.63 |
| Camera body (`camera_01`) | 0.32 | 0.09 | 84% | 0% | 0.15 |
| Camera lens body (`camera_01`) | 0.44 | 0.13 | 71% | 0% | 0.10 |
| Boombox | 0.48 | 0.30 | 71% | 0% | 0.94 |
| Leather ball | 0.46 | 0.16 | 70% | 0% | 0.02 |
| Flight helmet glass and plastic | 0.43 | 0.05 | 67% | 0% | 0.00 |
| Flight helmet leather | 0.49 | 0.24 | 64% | 0% | 0.00 |
| Water bottle (emissive label) | 0.50 | 0.00 | 55% | 0% | 0.41 |
| Flight helmet hose, rubber and wood | 0.49 | 0.22 | 52% | 0% | 0.06 |
| Radio transceiver (two materials) | 0.50 | 0.14 | 48 to 51% | 0% | 0.06 to 0.25 |
| Sci-fi helmet | 0.54 | 0.04 | 45% | 0% | 0.30 |
| Marble ball | 0.51 | 0.49 | 44% | 0% | 0.00 |
| Brass blowtorch | 0.61 | 0.11 | 43% | 0% | 0.45 |
| Flight helmet metal parts | 0.53 | 0.17 | 37% | 0% | 0.66 |
| Gas mask | 0.60 | 0.05 | 36% | 0% | 0.43 |
| Camera strap (`camera_01`) | 0.57 | 0.17 | 28% | 0% | 0.02 |
| Tread plate ball | 0.59 | 0.27 | 15% | 0% | 0.39 |
| Stage floor (30 x 30 m) | 0.52 | 0.49 | 14% | 0% | 0.00 |
| Denim ball | 0.81 | 0.64 | 0% | 0% | 0.01 |

Two things for whoever tunes the renderer against this scene:

- The stage floor and the marble ball sit on the cut-off (0.49 to 0.52): whether they are
  traced is decided texel by texel.
- The lantern has no emitter and no light. Its panes are an alpha-blended film that shares
  the brass's metallic-roughness texture, not glass with an index of refraction.
