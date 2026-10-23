# ara-asset-format

Small cooked-file readers and deterministic writers; no renderer/animation or
source-importer dependency. FORMAT.md is the wire contract, version 1.

Public API: Container/Builder, Mesh, Skeleton, Clip, Texture/write_ktx2.
All payload views borrow original bytes, suitable for mapped files and uploads.
Read LE scalars or use checked Pod casts on aligned little-endian hosts.
Never cast untrusted offsets directly, or expose bytes before cross-validation.

Run cargo check/test/clippy -p ara-asset-format (clippy --all-targets).
On this shared PC set CARGO_BUILD_JOBS=2.

Format changes require a new version and reader compatibility decision. Keep
reserved fields zero, validate all index spans, finite values and reference cycles.
