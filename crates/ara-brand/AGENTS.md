# ara-brand

Dev-only tool (never shipped) that turns the Ara logo SVG masters into derived
files and compresses README media. Brand rationale lives in
[`docs/design/brand.md`](../../docs/design/brand.md).

## Commands

- `cargo run -p ara-brand -- export`: from `assets/brand/*.svg`, writes
  `png/ara-icon-N.png` and `png/ara-mark-N.png` (N = 16…1024), `ara.ico`
  (16/24/32/48/64/256) and `favicon.ico` (16/32/48). Sizes ≤ 24 px use the
  optical `*-small.svg` master when one exists.
- `cargo run -p ara-brand -- render <svg> <png> --size N [--zoom Z]`: preview one
  SVG; `--zoom` upscales nearest-neighbour to inspect 16 px pixels.
- `cargo run -p ara-brand -- media <in> <out.jpg> [--width 1920] [--quality 84]`:
  Lanczos downscale (never upscale) plus baseline JPEG, for `docs/media/` (keep
  each file ≤ 400 KB).

## Gotchas

- PNG/ICO files are generated: edit the SVG master, then re-run `export` and
  commit both. `ara-platform` embeds `png/ara-icon-64.png` as the window icon;
  `ara-editor`/`ara-player` embed `ara.ico` through `assets/brand/ara.rc`.
- resvg runs without its `text` and `raster-images` features: masters must be
  plain paths (the wordmark is set as paths) and must not reference images.
- resvg renders `currentColor` as black, so mono masters export as black.

## Test

`CARGO_BUILD_JOBS=2 cargo clippy -p ara-brand --all-targets -- -D warnings` and
`cargo fmt -p ara-brand -- --check`. The behavioural check is `export` plus a
look at the 16 px output with `render --zoom`.
