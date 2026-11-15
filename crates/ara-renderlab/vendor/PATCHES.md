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

# wgpu-core bind-group tracking dedup patch

## Why

wgpu-core 30.0.1 records commands as a pass is encoded and replays them in
`CommandEncoder::finish`. On every compute dispatch the replay merges each resource of each bound
group into the dispatch's usage scope and then drains it (`flush_bindings`), so the CPU cost of a
dispatch grows with the number of entries its bind groups list. A bind group lists every binding
as written, repeats included. Renderlab's per-draw binding arrays hold one entry for each of
TRACED_DRAWS (256) draw slots, padded with a stand-in buffer, and draws that share a material,
texture or mesh repeat the same buffer or view; the previous-frame vertex array repeats the vertex
array for unskinned draws. One shading dispatch therefore walked about 1,027 geometry buffers, 512
material buffers and 768 material views on every scene.

The patch keeps each equal (resource, usage) pair once when the bind group is created (in
`optimize`, which already sorts these lists). Merging an equal pair into a usage scope a second
time changes nothing, and pairs that differ in usage are all kept, so every usage conflict wgpu
reports is still reported; no validation or safety check is skipped. Bind-group validation itself
runs on the descriptor, before these lists are built, and is unchanged.

## Measured

Entries per bind group, padded total against what the patch keeps (RENDERLAB_GROUP_STATS):

| scene | geometry buffers | material buffers | material views |
|---|---|---|---|
| Showcase | 1,027 -> 172 | 512 -> 90 | 768 -> 83 |
| Outpost | 1,027 -> 742 | 512 -> 478 | 768 -> 341 |
| Glint | 1,027 -> 130 | 512 -> 126 | 768 -> 3 |

In the game, same Renderlab revision (2805ad12) with and without the patch, interleaved in five
quiet-machine rounds: Showcase encode 0.07-0.17 ms lower with ray hardware and 0.10-0.15 ms lower
without (an earlier 0.5-0.8 ms reading came from rounds where the unpatched process happened to run
in a slow CPU state); Outpost not measurably different (its round-to-round spread that day was about
1 ms). Splitting each stage's `finish` showed Outpost's shading dispatch replay unchanged
(0.175 -> 0.176 ms) despite a third fewer entries, so these entries are not what that dispatch's
replay is made of. GPU time and frames unchanged: Showcase's moving frame byte-identical on both
paths, Outpost's within its own run-to-run differences.

Status: NOT RECOMMENDED for merging. Binding only the frame's draws on Renderlab's side
(PARTIALLY_BOUND_BINDING_ARRAY, Renderlab branch t3/shade-13) removes the padding, which is most of
the repeats on Showcase and Glint, and measured the same or better there (encode -0.1 to -0.3 ms)
without a vendored change; neither moves Outpost. This section and diff are kept for the record.

## Exact diff

Two files, `src/track/buffer.rs` and `src/track/texture.rs`, in the bind group states' `optimize`:

```diff
diff --git a/crates/ara-renderlab/vendor/wgpu-core/src/track/buffer.rs b/crates/ara-renderlab/vendor/wgpu-core/src/track/buffer.rs
index 8fdbe8101..4a9d26c89 100644
--- a/crates/ara-renderlab/vendor/wgpu-core/src/track/buffer.rs
+++ b/crates/ara-renderlab/vendor/wgpu-core/src/track/buffer.rs
@@ -54,6 +54,11 @@ impl BufferBindGroupState {
     /// accesses will be in a constant ascending order.
     pub(crate) fn optimize(&mut self) {
+        // Ara patch (crates/ara-renderlab/vendor/PATCHES.md): equal (buffer, usage) pairs, which
+        // binding arrays repeat, are kept once. Merging one into a usage scope again changes
+        // nothing, and pairs that differ in usage are all kept, so conflicts are still found.
         self.buffers
-            .sort_unstable_by_key(|(b, _)| b.tracker_index());
+            .sort_unstable_by_key(|(b, u)| (b.tracker_index(), u.bits()));
+        self.buffers
+            .dedup_by(|(b, u), (kept, kept_u)| Arc::ptr_eq(b, kept) && u == kept_u);
     }
 
diff --git a/crates/ara-renderlab/vendor/wgpu-core/src/track/texture.rs b/crates/ara-renderlab/vendor/wgpu-core/src/track/texture.rs
index 6cea59290..99738a653 100644
--- a/crates/ara-renderlab/vendor/wgpu-core/src/track/texture.rs
+++ b/crates/ara-renderlab/vendor/wgpu-core/src/track/texture.rs
@@ -153,6 +153,12 @@ impl TextureViewBindGroupState {
     /// accesses will be in a constant ascending order.
     pub(crate) fn optimize(&mut self) {
+        // Ara patch (crates/ara-renderlab/vendor/PATCHES.md): equal (view, usage) pairs, which
+        // binding arrays repeat, are kept once. Merging one into a usage scope again changes
+        // nothing, and pairs that differ in view or usage are all kept.
+        self.views.sort_unstable_by_key(|(view, u)| {
+            (view.parent.tracker_index(), u.bits(), Arc::as_ptr(view) as usize)
+        });
         self.views
-            .sort_unstable_by_key(|(view, _)| view.parent.tracker_index());
+            .dedup_by(|(view, u), (kept, kept_u)| Arc::ptr_eq(view, kept) && u == kept_u);
     }
 
```
