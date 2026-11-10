# ara-curve
The curve value type: keys with values, slopes, tangent modes (Auto, Flat, Free, Broken) and
per-segment interpolation (Cubic, Linear, Constant), plus extrapolation before and after the
keys (Clamp, Loop, PingPong). Anything that maps one number to another over time holds a
`Curve`: VFX Size over Life, animation transition blends.

## API
- `Curve::evaluate(t)`: pure `f32` math, no state; deterministic for the same input.
- `Curve::slopes(i)`: a key's effective in/out slopes (Auto computed from neighbours,
  monotone so it never overshoots them; level at ends, peaks and dips).
- `insert`, `move_key` keep keys in strictly increasing time; `validate` says why a curve
  can't be used; `value_range` frames an editor; `Preset` gives starting shapes.
- Reflected through `ara-schema` as `ValueCurve`, `CurveKey`, `CurveTangent`,
  `CurveInterpolation`, `CurveExtrapolation` (the animation module already has a `Curve`).

## Test
`cargo test -p ara-curve`. GPU lowerings (VFX's Slang) must match `evaluate`; their tests
compare against it.

## Gotchas
- Free uses `out_slope` on both sides; Broken uses both fields.
- An empty curve evaluates to 0; validate before cooking.
