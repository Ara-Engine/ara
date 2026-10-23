# Recast/Detour

Unmodified source from [Recast Navigation 1.6.0](https://github.com/recastnavigation/recastnavigation/tree/b4554541b658630816dba41466eb1cefb624519e),
commit `b4554541b658630816dba41466eb1cefb624519e`, zlib license in
`vendor/License.txt`. Only library headers/sources are included; no demos or deps.
The Ara shim is separate in `shim/`. Build uses `cc`, never network or CMake.

Recast is selected by A11. Its voxelization, region/contour/detail construction,
tile stitching, area-aware Detour queries, off-mesh links and DetourCrowd avoidance
are available behind one maintained dependency. Landmass provides Rust navigation
and avoidance but would require owning the geometry-to-navmesh baker; that would
move the largest quality/maintenance risk into Ara. We keep that risk upstream.

Strict floating point is enabled. Same-build replay determinism is tested;
cross-platform bit equality requires Windows/Linux/ARM verification and is not
claimed. Cache keys include target architecture, endian, revision and format.

Dynamic cut-outs use background incremental voxel tile bakes rather than the
DetourTileCache obstacle queue (limited to eight touched tiles per obstacle).
They support arbitrary tile spans and the same quality as the static bake.
DetourTileCache sources are retained for a future measured fast path.
