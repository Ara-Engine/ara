# Owner-reported Play bugs on Outpost (fixed in 91996473 and f0dc65af)

Live raster frames as Play draws them (TAA, jitter and half-resolution history
running, no capture reset), High at 1440p, captured with `ara-samples`'
`ARA_CAPTURE_LIVE` sequences. Before = the shaders at 1f6c6ab6, after = 91996473.

## 1. Dark flickering band at the fence base and the hangar roof edge

- `fence-ao-walk.webp`: the camera walks and turns past the compound; before on
  top, after below.
- `fence-ao-still.webp`: one held frame, zoomed on the fence line, with an
  amplified after-minus-before panel: the bright strips are the band that is gone.
  (From this camera the fence base lies in a mountain shadow, so the band is
  subtler than in the owner's sunlit view; the difference panel shows it plainly.)

Cause: screen-space AO read the alpha-tested chain-link wires in the depth buffer
as a solid wall over the ground, and thin wires came and went at half resolution
with jitter and motion. Cutouts now receive AO but cast none, and the
half-resolution AO, GI and reflection upsamples no longer blend across depth
edges (the thin line along the roof's top edge).

## 2. Hard seam on the mountains that moves with distance

- `mountain-fog-dolly.webp`: a 25 m/s dolly toward the mountains; before on top,
  after below.
- `mountain-fog-dolly-still.webp`: four frames of the same dolly side by side.
- `mountain-raster-vs-pathtraced.webp`: the path-traced reference shows the same
  warm band.

Cause: not GI. It is sunlit height fog: the sun, 4 degrees up, shines through the
valley onto the low air in front of the mountains, and the forward-scattering fog
glows (the path tracer agrees). Its edge was decided by one shadow lookup per fog
slice, 90-140 m deep at that range, so it jumped as the camera moved. Each slice
now uses the lit share of four points along it, so the edge moves smoothly. The
band itself stays: removing it would be a change to Outpost's authored fog.
