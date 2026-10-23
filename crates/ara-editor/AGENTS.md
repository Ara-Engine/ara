# ara-editor

Native M2 editor shell, in-process project authority and Engine Protocol server.
The UI is ara-ui; its display list goes to the Vulkan adapter in `ara-editor-gpu`.
The viewport samples ara-render's offscreen image on that same device.

Public boundaries:
- `Editor`: host, navigation commands, content batches, review, deferred agent policy.
- `state`: navigation/layout op document; deliberately separate from content undo.
- `world::EditWorldView`: replace the document extraction with ara-sim later.
- `inspector`: schema-driven fields, mixed values, units and provenance.
- `shell`: retained panel tree and semantic interaction helpers.
- `gpu`: 64-byte instance ABI, raster/MSDF atlases, ordered draws and clip masks.
- `transport`: authenticated localhost EP and MCP; no project state on IO threads.

Every content change uses cmd.apply, including gizmo commits and diagnostic fixes.
Gizmo preview is transient extraction data, never a side-channel document write.
Escape drops the preview; release commits one transform.set batch with its base.
View commands produce ara-ops transactions in a separate navigation document.
Persist layout with the canonical writer to `.ara/editor-layout.json`.

Play controls stay disabled until the disposable ara-sim player is connected.
C# integration registers host services/schema types, not mutable UI state.
The temporary glTF loader is isolated in `world/gltf.rs`; replace with ara-asset.

Set CARGO_BUILD_JOBS=2. Use `run.ps1 -Mode Check` for the local gates or targeted
cargo check/test/clippy commands. The GPU gate uses process-local Vulkan loader
isolation and requires validation layers, SDL3 and this machine's GPU.
`run.ps1 -Mode Verify` runs the actual window, protocol interaction scenario,
GPU/CPU comparison, captures both themes and records raw timing samples.
Intentional goldens live in `tests/goldens`; evidence lives in `evidence`.
Never update goldens implicitly during ordinary tests.
