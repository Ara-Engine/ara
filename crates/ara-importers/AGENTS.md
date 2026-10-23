# ara-importers

Production pure glTF/GLB, FBX and image importers for ara-import-worker.
Public API: registry, the three Importer implementations, cook_mesh/cook_texture,
MeshSettings/TextureSettings and Vertex. Host SourceAsset/snapshot live in ara-asset.

Run cargo check/test/clippy -p ara-importers (clippy --all-targets).
Set CARGO_BUILD_JOBS=2 on this shared PC. No dev server or build command needed.

Importer implementations must consume only ImportInput. No filesystem, clocks,
unseeded RNG or environment reads. Dependency discovery is also pure and runs in
the worker: even discovering FBX dependencies must never run in the editor.

Changing algorithms, dependencies or output policy requires an importer version
bump. Intel ISPC is pinned, uses slow quality presets and precompiled kernels.
ASTC belongs behind a mobile importer plugin; never emit BCn for mobile silently.
FORMAT.md in ara-asset-format and INTEGRATION.md here define runtime adoption.

Downloaded third-party fixtures live in .fixtures (gitignored). Tests should not
download from the network. See MEASUREMENTS.md for reproducible real-scene checks.
