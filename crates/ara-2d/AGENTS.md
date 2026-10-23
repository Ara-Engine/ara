# ara-2d

Reflected 2D vocabulary and pure tile geometry. No GPU, image decoding or physics.
Gameplay state lives in components; everything here is canonical text (A3).

## Public API

- Components (module `twoD`): `Sprite`, `Camera2d`, `Light2d`, `Flipbook`, `Tilemap`;
  asset metadata `Atlas`/`SpriteRegion`. `register(db)` / `register_sim(registry)`.
- `tilemap`: `key`/`coordinate` (canonical `x,y`), `apply_rules`, `chunks` (AR2C v2
  cache payloads), `colliders` (merged convex hulls for physics), `outline` (shadow
  silhouette: boundary edges only, counter-clockwise).
- `commands::register`: `tilemap.paint|erase|fill|rule.apply` (strokes, optional
  `autotile`), and validated setters `sprite.set`, `camera2d.set`, `light2d.set`,
  `flipbook.set`, `tilemap.set`. Diagnostics use ARA3620.
- `canonical`, `canonical_json`, `to_value` (f32 written by shortest decimal).

## Tile model

- A cell is one positive tile number (`"3,-2": 1`); zero/absent is empty. Terrain,
  solidity, lighting and an optional drawn size (tall walls/trees) live on the
  `TileDefinition`, so maps stay one line per cell.
- Rules: first match wins. Masks use `GridLayout::neighbors` order: grid/iso have 8
  neighbors (E, NE, N, NW, W, SW, S, SE, counter-clockwise, +Y up); hex has 6 axial.
  A rule's output tile must keep the rule's terrain, so one pass is exact.
- Rules read one immutable snapshot; outputs never cascade within a pass.
- Hex is axial pointy-top; isometric uses diamond coordinates. Chunk division is
  Euclidean (signed coordinates). Layer IDs are stable (`l_ground`); painter order is
  sorting layer, order, then identity; never array or ECS order.

## Commands

- One call, or one enclosing Batch, is one undo item: submit every pointer sample of a
  stroke together. Nothing is written if any sample is invalid.
- Each changed cell is its own op, so disjoint collaborative edits merge; unchanged
  cells emit no op. `autotile` re-resolves the stroke plus its neighbors in the same
  transaction. Fill is bounded (≤ 65536 cells) and edge-connected.

## Test

Set CARGO_BUILD_JOBS=2. `cargo test -p ara-2d`;
`cargo clippy -p ara-2d --all-targets -- -D warnings`; `cargo fmt -p ara-2d -- --check`.
