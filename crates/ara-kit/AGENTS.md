# ara-kit

Ara's design system on [stillkit](https://github.com/Snowy7/stillkit) (D34), shared by the
editor and the Hub. Ara-specific look only: anything a generic app could use belongs in
stillkit or stillkit-kit as a PR, then reaches Ara through the pinned `ara` revision in the
root `Cargo.toml`.

## Public API

- `icons`: one typed constant per Lucide SVG in `icons/` (`icons::FOLDER_PLUS`), plus `ALL`
  and `named(name)` for data-driven callers. Generated; never edit `src/icons.rs` by hand.
- `look::Look::new(dark)`: the `assets/themes/` tokens (D18) as typed `Colors`, `Radius` and
  `Metrics`, plus the stillkit `Theme` they map to (dense type ramp on the platform UI face).
  Views read Ara tokens from `look.c`; stillkit components read the mapped theme.
- `ui`: components as plain functions taking `&Look` and returning `El` for the caller to key,
  size and give handlers: `button`/`icon_button`/`toggle_button` (Tone x Size), `field`,
  `search`, `select`, `switch`, `checkbox`, `segmented`, `form_row`, `chip`, `count`, `dot`,
  `kbd`, `card`/`hover_card`, `progress`, `spinner`, `section`, `empty`, `avatar`, `banner`,
  `menu_panel`/`menu_item`, `dialog_panel`, `toast`, `title_bar` (window-control keys for
  native chrome), `nav_item`, `page_header`. Controls `cascade_ink`, so labels and icons follow
  their colour and hover look; feedback washes come from the theme (`look.rs`).
- `ui::Material`: D31's six satin materials (Well, Raised, Pressed, Primary, Float, Dock) from
  `docs/design/theme.md`; `Material::Raised.paint(l, row())` gives fill, inner light and shade
  (stillkit `El::edges`) and the outer edge. Components are built from them.
- `art::cover(look, seed, w, h)`: generated mesh-gradient cover art (chart palette, grain,
  vignette) for projects and templates without a picture; `art::seed(id)` is stable.
  `--example covers` writes a sample sheet.
- `capture::png(&Image)`: a headless stillkit snapshot as PNG bytes (captures, tests).

## Icons

`icons/` holds Lucide SVGs at the commit in `icons/SOURCE.md`. To add one, download
`<name>.svg` from that commit, then `cargo run -p ara-kit --example gen-icons`, and look at
`cargo run -p ara-kit --example icon-sheet` (writes `target/ara-kit/icons.png`).

## Gallery

`cargo run -p ara-kit --example gallery` writes `target/ara-kit/gallery-{dark,light}.png`: every
component with real hover, press and focus states. Look at both after any component change.

## Test

`cargo test -p ara-kit`, `cargo clippy -p ara-kit --all-targets -- -D warnings`.
