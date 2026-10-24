# ara-foliage

Owns seeded scatter, surface filtering, compact instance extraction and manual
painting commands. Documents are authoring truth; extracted instances are
immutable renderer inputs, never gameplay state. Game entities/components own
interactive trees/rocks; this crate does not create hidden simulation objects.

Public API: register, commands::register, Rule, Surface, scatter, painted,
Instance, lod_visible. GPU instancing/wind/alpha/depth/impostors live in ara-landscape.

Rules operate on a global candidate lattice. Stable rule identity + seed + signed cell coordinates
determine jitter/acceptance/yaw/scale/wind phase. Half-open bounds avoid duplicates
across tiles; request ordering and worker cancellation do not change identities.
One candidate per cell; validate density * cell_size^2 <= 1. Split large requests
into tiles. Density multiplies normalized terrain layer weight and seeded noise;
height, slope, holes and exclusions filter candidates.

Manual paint includes all points from one gesture. One batch creates one undo
entry. Erase whole stroke records; rules referenced by paint cannot be deleted.
The host should register both terrain and foliage schemas/commands in its shared
SchemaDb/Registry, then invalidate extraction for affected tiles after op commit.
`foliage.exclude` accepts a stable `id` or batch symbol for reproducible builds.
No frame code performs regeneration. Spawn workers below normal priority and
publish last-good instance buffers at a D20-permitted boundary.

Set CARGO_BUILD_JOBS=2:
cargo check -p ara-foliage
cargo test -p ara-foliage
cargo clippy -p ara-foliage --all-targets -- -D warnings
cargo fmt -p ara-foliage -- --check
