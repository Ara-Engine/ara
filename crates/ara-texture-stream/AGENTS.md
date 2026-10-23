# ara-texture-stream
Native BC material residency and software virtual texture pages.
Virtual pages have 128x128 interiors, wrapping gutters, a bounded RGBA8 pool
and GPU derivative/min-mip feedback.
Public API: Source::{rgba8,from_cooked}, Catalog, Residency, Binding, Stats.
`NativeSource` retains validated cooked BC4/BC5/BC7 blocks without CPU decode;
`mip`, `resident_bytes` and `stage` expose complete native mip tails for residency.
`NativeResidency` deduplicates caller-provided cooked content hashes, uploads
BC tails in batches, and publishes a fresh mapping table after successful upload.
`NativeConfig` budgets compressed published payload and bytes per refinement;
initialization and a single indivisible tail may exceed the refinement target.
Temporary image/staging allocations and device alignment are outside payload stats.
Demand/priority are aggregated by the caller before request(); every map retains
an actual authored mip tail. Mipless textures cannot be coarsened to save memory.
Register/initialize at content boundaries, refine after submitted() completion,
declare resources() in sampling passes, and destroy() before dropping the device.
No decode, atlas, I/O, source hashing or GPU allocation occurs on idle refinement.
NativeBinding rows are 32 bytes: slot, first mip, source width/height, encoding
(0 BC7, 1 BC4, 2 BC5 XY), resident levels, two reserved words.
Use native_shader_source() for offline consumers; it corrects resident LODs,
uses hardware sRGB/trilinear filtering and reconstructs BC5 Z before normalScale.
The existing RGBA virtual page API remains separate.
Preparation/decoding belongs on the extraction worker. Completed frame boundaries
own feedback consumption, LRU planning and uploads; never wait in record().
The caller submits begin_frame(), then its material sampling passes with Binding,
then end_frame(). Declare Binding.resources() in every sampling graph pass.
Mark submitted completion after end_frame; prepare() refuses an in-flight slot.
Sample helper is shader_source(); copy it into the consumer's source directory
before offline cooking, so its source hash covers the exact sampling contract.
Pinned mip tails guarantee a valid fallback. Children publish only when their
parent was resident in an earlier frame; protect current requests and ancestors.
Cold starts/camera cuts may initially be coarse. Record desired-vs-resident error
and same-surface temporal mip delta; never infer pop-in quality from policy alone.
Tests: cargo test -p ara-texture-stream; clippy --all-targets -- -D warnings;
cargo fmt -p ara-texture-stream -- --check. Set CARGO_BUILD_JOBS=4.
