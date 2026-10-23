# ara-nav-sys

Pinned Recast/Detour sources and the Ara-owned C ABI. No generated bindings,
downloads, global callbacks or custom allocator hooks at build/runtime.

`src/lib.rs` declares only ABI PODs and unsafe calls; `ara-nav` owns safe validation
and lifetimes. The C++ shim catches exceptions and owns every native allocation.
Mesh reads can run concurrently using separate query objects; all mutation requires
exclusive ownership. Crowds borrow a mesh and must die before it.

Vendor sources are unmodified zlib-licensed Recast 1.6.0. See `VENDOR.md`.
Build with `CARGO_BUILD_JOBS=2`. Test/check/clippy both `ara-nav-sys` and `ara-nav`.
Do not change the root manifest. Keep C++ floating point strict (no fast math/FMA).
