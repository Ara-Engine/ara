# ara-kit

Ara's design system on [stillkit](https://github.com/Snowy7/stillkit) (D31), shared by the
editor and the Hub. Ara-specific look only: anything a generic app could use belongs in
stillkit or stillkit-kit as a PR, then reaches Ara through the pinned `ara` revision in the
root `Cargo.toml`.

## Public API

- `icons`: one typed constant per Lucide SVG in `icons/` (`icons::FOLDER_PLUS`), plus `ALL`
  and `named(name)` for data-driven callers. Generated; never edit `src/icons.rs` by hand.
- `capture::png(&Image)`: a headless stillkit snapshot as PNG bytes (captures, tests).

## Icons

`icons/` holds Lucide SVGs at the commit in `icons/SOURCE.md`. To add one, download
`<name>.svg` from that commit, then `cargo run -p ara-kit --example gen-icons`, and look at
`cargo run -p ara-kit --example icon-sheet` (writes `target/ara-kit/icons.png`).

## Test

`cargo test -p ara-kit`, `cargo clippy -p ara-kit --all-targets -- -D warnings`.
