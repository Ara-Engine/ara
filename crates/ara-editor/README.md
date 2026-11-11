# Ara editor

`ara-editor` is Ara's native editor (the studio, built on stillkit) and the project's
in-process Engine Protocol authority. The Scene, Game and graph views present through
the renderer's own GPU device into native layers under the UI.

From an initialized project:

```powershell
$env:CARGO_BUILD_JOBS='2'
cargo run -p ara-editor -- <project> --sample
```

`--sample` creates the four-entity Outpost graybox only if the scene is empty. Content is
always saved through the host. Docking is saved per project in `.ara/editor-layout.json`.
On Linux the editor needs a Wayland session whose compositor offers subsurfaces (GNOME and
KDE do); elsewhere it opens with no setup. `--headless` writes `editor-scene.png` to
`--output` without opening a window. `--frames N` ends a run after N frames with the window
hidden; `--screenshot` saves the window with its Scene as `editor.png` in `--output` and
exits.

## Two-minute demo

1. Select **Salvage crate** in Hierarchy and enter `1.5m` in position X. Multi-select with
   Ctrl/Shift; fields that differ show "—" and take one value for all.
2. Alt-drag to orbit, middle-drag to pan, wheel to zoom, right-drag plus WASD to fly. F
   frames. Q/W/E/R/T/Y pick select/move/rotate/scale/rect/universal. A drag commits one
   transaction; Escape cancels.
3. Drag a Hierarchy row to reparent or reorder; its context menu creates, renames,
   duplicates, deletes or unpacks a prefab.
4. Open the palette (Ctrl+K / ⌘K) to search every command. Ctrl+Z/Y undo/redo content
   independently of selection and camera history.
5. Connect `ara mcp --root <project>` (or the CLI) to the running editor and apply an
   attributed batch with a `why`; the Agents panel shows the turn with Undo.
6. Request `ara_capture` with `target: "viewport"`, `"editor"` or `"layout"`. Viewport
   captures include marks, legend and the entity map.

Editor navigation is exposed as `editor.*` protocol methods and `ara_session` actions.
Content actions use `ara_apply` and the same command registry as the Inspector.
`.ara/daemon.json` is owned by the window's authority and removed on clean shutdown.

## Integration boundaries

- `world::EditWorldView` isolates the document projection from ara-sim. Provisional
  gizmos overlay that projection; committed transforms use `transform.set`.
- Play uses the registered `ara-play-host` service; the Game view shows each player's
  frames through shared GPU surfaces.
- `world/gltf.rs` is a source geometry loader; ara-asset owns cooked meshes and texture
  streaming. Asset drags use stable `@path#id` references.
- The play host owns C# execution and schema services. The Inspector consumes schema
  docs, units, ranges, defaults, enums and references through the host.
