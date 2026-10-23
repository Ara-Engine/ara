# ara-import2d

Pure atlas and tile chunk cooks, installed in the ara-import2d worker executable.
API: registry, AtlasImporter, TilemapImporter, AtlasSettings, SourceRegion.
Source PNG is accompanied by dependency snapshots; no importer filesystem I/O.
Atlas uses deterministic height/name shelf packing with an extruded gutter.
Outputs atlas.json canonical metadata and atlas.rgba (AR2A v1 LE header + RGBA8).
PNG color channels remain sRGB; render upload uses SRGB for color, UNORM for normals.
Version changes are required whenever output algorithms change.
Tilemap importer reads canonical typed text and emits deterministic AR2C chunks;
cache publication and rollback roots remain ara-asset's responsibility.

Set CARGO_BUILD_JOBS=2. Gates: cargo check/test -p ara-import2d;
cargo clippy -p ara-import2d --all-targets -- -D warnings;
cargo fmt -p ara-import2d -- --check.
