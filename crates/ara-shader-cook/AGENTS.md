# ara-shader-cook

Offline Slang compiler orchestration; never a runtime renderer dependency.

## API and artifacts

`cook(manifest, output, Toolchain)` compiles pipeline manifests into:
- SPIR-V validated against Vulkan 1.3;
- SM6.6 DXIL for Metal Shader Converter;
- separate Slang reflection JSON for each target;
- versioned cooked pipeline descriptions with stage/state/format/layout hashes.

Sources/includes, exact compiler executable and supporting compiler DLLs,
compiler version, recipe/flags, binding ABI and pipeline manifest enter the key.
Artifacts live below that content key. A failed new source revision preserves
the previous pipeline manifest and its artifacts. Intact unchanged stages are
reused; corrupt SPIR-V/DXIL are rebuilt. Includes must live within the source tree.
SPIR-V entry names are `main`; DXIL keeps source entry names. Vertex SPIR-V inverts
Y, DXIL does not. Both targets use column-major matrices and 32-byte root data.

## Commands

Set `CARGO_BUILD_JOBS=4`.
`cargo run -p ara-shader-cook -- shaders/pipelines.json crates/ara-render/cooked`
from the repository root. Optional third argument is the Vulkan SDK directory;
otherwise bundled tools beside the executable, VULKAN_SDK, then PATH are used.

Run `cargo check -p ara-shader-cook`, `cargo test -p ara-shader-cook`,
`cargo clippy -p ara-shader-cook --all-targets -- -D warnings`, and
`cargo fmt -p ara-shader-cook -- --check`.
The SDK-backed test checks reuse, corrupt-artifact recovery and preservation of
the last good cook after compiler failure. It runs the real Slang/DXIL tools.

No runtime compilation, no Slang Metal target. The Mac backend consumes DXIL
through Apple's converter and adds its metallib/reflection artifacts after S1.
