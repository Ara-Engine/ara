//! CPU construction of compressed-wide BVHs (CWBVH) for Ara's software `trace()`:
//! one BLAS per mesh asset, built at import or on load from a mesh-hash cache, plus
//! the instance TLAS. Pure data in, flat GPU-ready buffers out; no GPU or asset I/O.
//! Used by `ara-importers` (import-time builds) and `ara-render` (`look::trace`).
