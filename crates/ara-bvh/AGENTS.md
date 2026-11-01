# ara-bvh

Purpose: CPU builder for compressed-wide BVHs (CWBVH) used by `ara-render`'s `look::trace`.
Inputs are triangle soups; outputs are flat GPU-ready buffers. No GPU, no asset I/O.

Owner: the render-v3 A1 track (council-v3 architecture, section 6.1).

Test: `cargo test -p ara-bvh`.
