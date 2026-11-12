# ara-render-fidelity

The authored look as numbers a renderer consumes. No GPU work happens here: Renderlab draws
(`ara-renderlab`), the editor and the player hand it what this crate resolves.

## Public API
- `environment::FrameEnvironment::resolve(&frame.look, camera, env.exposure)`: the scene's
  `SceneLook` (sky atmosphere, height fog, post-process volumes blended at the camera, colour LUT)
  as one value: sky and atmosphere lights (`set_atmosphere_lights`), fog, exposure (manual EV100
  or automatic metering limits), compensation and its curve, bloom, lens (depth of field, motion
  blur), grading and tone mapper. `post()` gives the `PostSettings`; `manual_exposure` the linear
  multiplier; `fog_volumes()` sorts local fog volumes (priority, then distance, at most
  `ara_frame::look::MAX_FOG_VOLUMES`). `raster()` is the look record other renderers read.
- `colour`: `ToneMapper` (PBR Neutral, Hill's Filmic, AgX and its looks, the ACES fit), `Grading`
  (white balance, contrast, saturation, lift/gamma/gain), `AutoExposure` and `ExposureState`.
- `settings::{PostSettings, Quality, exposure_from_ev100}`: exposure and display processing in
  physical units; `exposure_from_ev100` uses the saturation-based calibration Lmax = 1.2 * 2^EV100.
- `atmosphere::Atmosphere`: the `SkyAtmosphere` component's parameters in SI units;
  `validate` checks every field.
- `material::MaterialDetail`: typed coat, cloth and thin-leaf parameters over the reserved
  `ara_frame::scene::Material` flag words.
- `presets`: `QualityChoice`, `QualityPreset` (Low, Medium, High, Epic; `raster_quality` is its
  `ara_frame::look::Quality`, `raster_scale` its render scale), `Capabilities::of(gpu)`,
  `RenderOptions { ray_tracing }`, and `Plan::resolve(choice, caps, display, options)` giving the
  preset that runs, the one asked for, the `RasterPlan` (resolutions and quality) and
  `rt_fallback` (ray tracing asked for on a GPU without verified ray queries).

## Gotchas
- Physical units throughout: look values are linear until the SDR transfer.
- No authoritative gameplay or authoring state lives here.

## Gates
`cargo test -p ara-render-fidelity`, `cargo clippy -p ara-render-fidelity --all-targets -- -D warnings`,
`cargo fmt -p ara-render-fidelity -- --check`.
