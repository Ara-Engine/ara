# ara-2d

Owns reflected sprite/camera/light/tilemap/flipbook components and pure tile geometry.
No GPU or source-image decoding dependency. Gameplay state lives in components.

API: register, register_sim, commands::register, Atlas, Sprite, Camera2d, Light2d,
Flipbook, Tilemap, chunks, colliders, apply_rules. All values use canonical JSON;
source tile cells remain text, chunks are disposable content-addressed cache data.
Tile coordinates are signed, chunk division is Euclidean, and zero is empty.
Layer IDs and sorting order are stable; do not use ECS traversal as draw order.
Hex uses axial pointy-top coordinates; isometric uses diamond coordinates.
One mouse stroke is one Batch of paint calls, or one paint call with many cells.
Do not commit partial strokes. Per-cell ops preserve disjoint collaborative edits.
Import and chunk cooking run in background workers, publish only at boundaries.

Set CARGO_BUILD_JOBS=2. Gates: cargo check/test -p ara-2d;
cargo clippy -p ara-2d --all-targets -- -D warnings;
cargo fmt -p ara-2d -- --check.
