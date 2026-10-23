# ara-opus-sys

Owns the small safe encoder/decoder API required by Ara's importer and streaming
decoder. The upstream C API is private; `shim.c` gives the two variadic controls
fixed signatures. Handles move across workers but cannot be shared concurrently.

Sources are unmodified files from the pinned **libopus 1.5.2** release:
https://downloads.xiph.org/releases/opus/opus-1.5.2.tar.gz
SHA-256: `65c1d2f78b9f2fb20082c38cbe47c951ad5839345876e46941612ee87f9a7ce1`.
The committed subset contains the portable floating-point encoder/decoder source
groups, headers, upstream source manifests, README, AUTHORS and COPYING.
No network, configuration generator, CMake, Python or assembler is used by Cargo.
`cc` discovers the installed MSVC/Clang/GCC toolchain and builds a static library.
Upstream optional neural/experimental features and assembly/SIMD dispatch are off.

The upstream BSD license and patent notices are in `vendor/opus-1.5.2/COPYING`;
preserve them when redistributing binaries. Version updates require reviewing the
source manifests and re-running Ara's PCM/Opus/ADPCM cooking and streaming tests.
