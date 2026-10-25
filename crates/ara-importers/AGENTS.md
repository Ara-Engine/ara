# ara-importers

Production pure glTF/GLB, FBX, image and audio importers for ara-import-worker.
Public API: registry, Importer implementations, cook_mesh/cook_texture/cook_texture_with_preset,
MeshSettings/TextureSettings and Vertex. Host SourceAsset/snapshot live in ara-asset.
`commands::register` exposes `import.initialize` and `import.configure` to the
shared CLI/MCP registry. It selects the importer from the source extension and
records its installed version; callers provide source, optional stable ID and settings.
Audio settings include `usage`, `codec`, `opusBitrate` and `decodedBudget` (resident
PCM bytes, 64 MiB default, 64 KiB minimum). Music/dialogue cook incrementally to
Opus; effects remain resident. Compressed data and codec/meter scratch are separate.
The builtin registry also installs ara-geo::GeoImporter (`ara.geo`) for extracted
single-material SourceMesh snapshots. glTF/FBX also emit one ARAG v1 Custom
artifact per static material primitive automatically; MeshSettings.virtualGeometry
opts out. Scene mesh entries link virtualGeometry artifact/cacheVersion/diagnostics.
Skin and morph deformation retain the classic path with a manifest note.
glTF generation 6 retains per-slot KHR_texture_transform (including UV selection),
emissive strength and transmission; transmission/AO cook as linear data. Missing
tangents use the normal slot's selected/transformed UVs; supplied tangents survive.
MeshSettings.opacityMaps maps a glTF image URI (or #index) to a companion URI.
Its red channel multiplies existing color alpha, resized with triangle filtering;
opaque materials become masked, while authored blend/mask modes remain. Discovery
includes these inputs. FBX rejects this glTF-only setting (shared settings ABI v6).
Static classic LOD chains default to eight levels, targeting 2% of LOD0 or 200
triangles. Attribute-aware meshoptimizer preserves UV seams and open borders;
alpha foliage also retains at least 90% area per connected card. Irreducible
cards keep their last good level. Skin/morph chains retain the four-level/1% cap.
Retain usable legacy LODs: replacing geometric errors with combined attribute
errors can make runtime selection choose finer foliage despite deeper chains.
ARAG Custom artifacts are
validated by ara-geo; its reflected settings are VirtualGeometrySettings.

Run cargo check/test/clippy -p ara-importers (clippy --all-targets).
Set CARGO_BUILD_JOBS=2 on this shared PC. Release benchmarks are explicit cargo run --release examples; see MEASUREMENTS.md.

Importer implementations consume ImportInput and explicitly supplied memo services.
ImportServices also supplies an optional integer block encoder. Keep device setup
and filesystem access in the worker service. GPU/CPU identities never alias; normal
maps retain ISPC BC5 after measured GPU angular outliers. Packing, preset and mip
order must stay deterministic; raw HDR inputs use finite positive half codes.
No other filesystem, clocks,
unseeded RNG or environment reads. Dependency discovery is also pure and runs in
the worker: even discovering FBX dependencies must never run in the editor.

Changing algorithms, dependencies or output policy requires an importer version
bump. Intel ISPC is pinned and precompiled. CookPreset explicitly selects quality; HQ
is selectable; Balanced is the import default. Keep output byte-identical across Rayon thread budgets.
ASTC belongs behind a mobile importer plugin; never emit BCn for mobile silently.
FORMAT.md in ara-asset-format and INTEGRATION.md here define runtime adoption.

Downloaded third-party fixtures live in .fixtures (gitignored). Tests should not
download from the network. See MEASUREMENTS.md for reproducible real-scene checks.

Project quality is a typed ara.imports document (import.project.configure). Sources
inherit it unless preset is explicit; TextureSettings.preset and MeshSettings.texturePresets
override individual images. Presets/version/settings key the intermediate memo; changing
one texture reuses geometry and the other texture tasks.
