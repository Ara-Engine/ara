# wgpu-core acceleration-structure dependency patch

## Upstream source

`wgpu-core` is the complete **wgpu-core 30.0.1** crate source from crates.io, at upstream commit
[`40f4a34ebaf56f9a046231f54125ad046239d3f3`](https://github.com/gfx-rs/wgpu/tree/40f4a34ebaf56f9a046231f54125ad046239d3f3)
(`wgpu-core/` in that repository). Its published manifest, lockfile and provenance metadata are
unchanged, and the directory includes upstream `LICENSE.MIT` and `LICENSE.APACHE`. Only this
crate is patched; every other wgpu crate resolves to 30.0.1 from crates.io. The Cargo registry
cache marker `.cargo-ok` is not source.

Published crate archive SHA-256 checksum (also the one in the original lockfile):

- `wgpu-core`: `14c018fce9b6270aa203c2fdd56f3cce996713534bd757e4ea58c8560b121f14`

## Why

When a top-level acceleration structure (TLAS) is built, wgpu-core 30.0.1 records one dependency
entry per instance, even when many instances share a bottom-level structure (BLAS). Every later
submission walks that list once for each use of the TLAS in its command buffers. Outpost has
6,338 instances of 253 distinct meshes and Renderlab uses the TLAS in 32 passes a frame, so wgpu
walked about 200,000 entries per frame inside `queue.submit`: 3.0-3.8 ms of CPU, which made the
scene CPU-bound with ray hardware.

The patch records each BLAS once per TLAS. With it the walk is about 8,000 entries a frame.
wgpu's development branch already builds this list from a `seen_dependencies` set; no release
contains it yet (30.0.2 does not). Remove this vendored crate and its `[patch.crates-io]` line in
the workspace `Cargo.toml` once Ara moves to a wgpu release that has the fix.

Measured in the game, Outpost with ray hardware, same Renderlab revision, three runs each:
frame 10.73-12.96 ms unpatched against 9.30-9.57 ms patched; CPU time in submit 3.36-3.78 ms
against 0.49-0.59 ms; GPU time unchanged. Showcase and Glint are unaffected.

## Exact diff

One file, `src/command/ray_tracing.rs`, in `build_acceleration_structures`:

```diff
         let mut dependencies = Vec::new();
+        // Ara patch (crates/ara-renderlab/vendor/PATCHES.md): each BLAS once. Submissions walk
+        // this list for every use of the TLAS, so one entry per instance costs
+        // O(instances x uses) per submit.
+        let mut seen_dependencies = crate::FastHashSet::<crate::track::TrackerIndex>::default();
@@
             let blas = &instance.blas;
-            state.tracker.blas_s.insert_single(blas.clone());
+            let is_new_dependency =
+                seen_dependencies.insert(crate::resource::Trackable::tracker_index(&**blas));
+            if is_new_dependency {
+                state.tracker.blas_s.insert_single(blas.clone());
+            }
@@
-            dependencies.push(blas.clone());
+            if is_new_dependency {
+                dependencies.push(blas.clone());
+            }
```
