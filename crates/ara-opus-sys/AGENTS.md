# ara-opus-sys

Pinned libopus 1.5.2, built from committed C sources by `cc`. No downloads,
CMake, bindgen, Python, assembler or generated headers during Cargo builds.
`Encoder`/`Decoder` uniquely own native state; bounded slices enforce FFI buffers.
The two variadic controls we need are wrapped by fixed-signature C functions.
Floating-point portable code is used on every target; no target CPU assumptions.

Check: `CARGO_BUILD_JOBS=2 cargo test -p ara-opus-sys -p ara-audio`.
Media round trips and malformed packets live in ara-audio/tests/media.rs.
Vendor changes require a pinned release, archive hash and license retention.
