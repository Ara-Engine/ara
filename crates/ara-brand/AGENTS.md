# ara-brand

Dev-only tool (never shipped) that turns the Ara logo SVG masters into derived
files and compresses README media. Brand rationale lives in
[`docs/design/brand.md`](../../docs/design/brand.md).

## Commands

- `cargo run -p ara-brand -- export`: from `assets/brand/*.svg`, writes
  `png/ara-icon-N.png` and `png/ara-mark-N.png` (N = 16…1024), `ara.ico`
  (16/24/32/48/64/256) and `favicon.ico` (16/32/48). Sizes ≤ 24 px use the
  optical `*-small.svg` master when one exists. Also regenerates the three program icons.
- `cargo run -p ara-brand -- icons [--check]`: regenerates `icons/{engine,game,hub}/`
  PNG/ICO outputs, or compares committed decoded pixels without writes. Program icons
  use the small drawing through 32 px, with ICO frames at 16/24/32/48/64/128/256.
  Original outputs come from Islam's design repo; PNG compression may differ. No Node needed.
- `cargo run -p ara-brand -- render <svg> <png> --size N [--zoom Z]`: preview one
  SVG; `--zoom` upscales nearest-neighbour to inspect 16 px pixels.
- `cargo run -p ara-brand -- media <in> <out.jpg> [--width 1920] [--quality 84]`:
  Lanczos downscale (never upscale) plus baseline JPEG, for `docs/media/` (keep
  each file ≤ 400 KB).

## Gotchas

- Commit SVGs and generated outputs together. `icons/{engine,game,hub}/` are the
  program designs: engine = editor/setup, game = player/export, hub = Hub/updater.
  Each has its own `.rc`; hosts select their 64 px PNG with `Window::set_icon_png`.
  Legacy `ara.ico`, `ara.rc`, `favicon.ico` and `png/ara-icon-*.png` stay compatible.
- resvg runs without its `text` and `raster-images` features: masters must be
  plain paths (the wordmark is set as paths) and must not reference images.
- resvg renders `currentColor` as black, so mono masters export as black.

## Test

`CARGO_BUILD_JOBS=2 cargo clippy -p ara-brand --all-targets -- -D warnings` and
`cargo fmt -p ara-brand -- --check`. `cargo test -p ara-brand` checks all program PNG/ICO pixels against Rust resvg,
required ICO sizes and each program's build-script resource mapping.
