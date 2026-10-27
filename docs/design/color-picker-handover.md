# Colour picker: design

Code: `crates/ara-editor/src/shell/color/` (`mod.rs` state and gestures, `markup.rs`,
`paint.rs`, `eyedropper.rs`, `math.rs`), `crates/ara-editor/src/app/eyedropper.rs`
(frame readback and Windows screen sampling), `Draw::Fill` in `ara-ui`.

## What it offers
Hue ring around a saturation/value square; RGB (0–255), HSV (degrees, percent) and
Linear (0–1) channel sliders with typed fields; sRGB and linear hex; alpha over a
checkerboard; New/Old swatches (Old restores); an eyedropper; project swatches
(`color.swatches`, project settings) and recent colours (`color.recent`, user
settings). Schema hints add sections: `editor(hdr)` an intensity in stops (−10…+10)
with −2…+2 exposure buttons, `editor(temperature)` a 1000–15000 K slider and presets.

## Colour maths
Documents store linear RGB(A). The picker edits display-space HSV of a base colour
(brightest channel ≤ 1) and stores `base · 2^stops`; sRGB conversions use the exact
piecewise curve everywhere (hex, swatches, the Inspector), never gamma 2.2.
Kelvin uses Krystek's Planckian-locus fit, through XYZ to linear Rec. 709.

## Rendering
`Draw::Fill` (`Fill::Linear`, `Fill::Hue`, `Fill::Checker`) is drawn analytically by
the UI shaders (pipeline mode 9) and the CPU reference. The SV square is a horizontal
white→hue gradient under a vertical transparent→black one, exact in sRGB space on the
UNORM UI target. Each painted part's draws are spliced right after its own box, so
labels and later popovers stay above; the eyedropper loupe is appended last.

## Interaction
Drags preview through an `@picker|target` draft marker (the Scene previews it like a
scrub) and apply one transaction on release; typed values, swatches, presets and
exposure steps apply one each. Escape cancels the eyedropper or a drag, else restores
the colour the picker opened with (one transaction) and closes. Enter or a click
outside keeps the colour and records it as recent. Pinned, the picker is an
`at='x,y'` popover moved by its title, ignores clicks outside and keeps editing the
entities it opened on. The eyedropper samples the editor's last frame around the
pointer (an 11×11 loupe); outside the window on Windows it samples the screen with
GDI and takes the global click. Offscreen runs never sample the desktop.
