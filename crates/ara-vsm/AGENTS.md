# ara-vsm

Virtual shadow address spaces, deterministic LRU allocation and invalidation.
API: Cache, PageKey, Projection, Light, DirtyPage. The GPU consumer is
ara-render-geo: it marks pages from resolved depth and rasterizes dirty pages
with the same cluster geometry. No dependency back to the renderer.

16k-square virtual projections use 128-square pages. Directional clipmap levels,
spot projections and six point-light faces have separate page identities. A page
is valid only after its submitted render completes. Light/caster revisions and
world-space bounds drive invalidation; a camera moving within a snapped clipmap
cell does not invalidate static pages. Never sample an unrendered assignment.

The host owns frame/timeline boundaries and must retain old assignments until
their last use completes. CPU cache methods run at completed diagnostic/offline
boundaries; no hidden GPU waits, clocks or authoring mutations occur here.

Gates: CARGO_BUILD_JOBS=2; cargo test -p ara-vsm;
cargo clippy -p ara-vsm --all-targets -- -D warnings;
cargo fmt -p ara-vsm -- --check.
