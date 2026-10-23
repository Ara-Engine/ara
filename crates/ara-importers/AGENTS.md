# ara-importers

Production pure glTF/GLB, FBX and image importers for ara-import-worker.
Public API: registry, the three Importer implementations, cook_mesh/cook_texture/cook_texture_with_preset,
MeshSettings/TextureSettings and Vertex. Host SourceAsset/snapshot live in ara-asset.
The builtin registry also installs ara-geo::GeoImporter (`ara.geo`) for extracted
single-material SourceMesh snapshots. Its versioned ARAG Custom artifact is
validated by ara-geo; its reflected settings are VirtualGeometrySettings.

Run cargo check/test/clippy -p ara-importers (clippy --all-targets).
Set CARGO_BUILD_JOBS=2 on this shared PC. Release benchmarks are explicit cargo run --release examples; see MEASUREMENTS.md.

Importer implementations must consume only ImportInput. No filesystem, clocks,
unseeded RNG or environment reads. Dependency discovery is also pure and runs in
the worker: even discovering FBX dependencies must never run in the editor.

Changing algorithms, dependencies or output policy requires an importer version
bump. Intel ISPC is pinned and precompiled. CookPreset explicitly selects quality; HQ
is the shipping default. Keep output byte-identical across Rayon thread budgets.
ASTC belongs behind a mobile importer plugin; never emit BCn for mobile silently.
FORMAT.md in ara-asset-format and INTEGRATION.md here define runtime adoption.

Downloaded third-party fixtures live in .fixtures (gitignored). Tests should not
download from the network. See MEASUREMENTS.md for reproducible real-scene checks.
