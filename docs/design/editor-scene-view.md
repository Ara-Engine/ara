# Scene view: toolbar, gizmos and overlays

Owner bar (preview 6): "the overall UI and the gizmos must not feel cluttered". The Scene view is the first surface people see in the editor, so this doc is the design its chrome and gizmos are built to. It follows D18 (tokens only) and the gizmo and overlay spec in `D:\AraRefs\editor-ui\ANALYSIS.md`, Appendix C. This doc records the choices that spec leaves open.

**References** (`D:\AraRefs\editor-ui\`):
- Toolbar: `unreal5/viewport-toolbar-1`, `unreal5/viewport-toolbar-2`, `unreal/viewport-1`, `unity/overlays-1`, `godot/viewport-1`.
- Gizmos and selection: `unreal5/transform-gizmos-1`, `unreal/gizmo-1`, `unity/gizmo-1`, `unity/scene-view-1`.

**Benchmarks.** UE 5.6 keeps the image clean with one opaque bar above it. The bar holds icon-only transform tools, World/Local as an icon, and snapping as compact "icon value ▾" items; Perspective, Lit and the Show eye are on the right. Unity's gizmos use solid cone arrowheads, translucent plane squares at the pivot, filled scale cubes and rotation rings. Both colour axes red, green and blue, and highlight hovered handles in yellow.

**Today** (`crates/ara-editor/evidence/scene-view/`):
- **The toolbar overflows at 1440×900.** "Show" is cut off and Frame is hidden. Tools, space and pivot carry text labels, and snapping takes three bordered split buttons.
- **The gizmo reads as lollipops.** Each arrowhead is a stack of round strokes, the plane handles are 4 px dots that don't read as planes, and the scale handles are dots too. The rotation rings draw their far halves at full strength, so the three rings tangle.
- **The axis widget is broken.** Positive caps show "−" and negative caps "+", where they should show the letter. The accessible labels read "XX view".

## Toolbar (28 px, opaque, above the image)

```
[⬚][✥][↻][⤢] [🌐][⊙] | [🧲][⊞ 0.5 m ▾][↻ 15° ▾][⤢ 0.1 ▾]          [🎥 Perspective ▾][50°] [☀ Lit · High ▾] [👁 ▾] [⛶]
```
- **Tools:** Select, Move, Rotate and Scale as an icon-only segment. The active tool fills with the accent. Tooltips name the tool and its key (Q, W, E, R).
- **Space and pivot:** icon toggles whose icon is the state: a globe for World and a box for Local; a crosshair for Pivot and a target for Centre. The tooltip names the state and what a click changes it to.
- **Snapping:** one magnet toggle, in the accent when on. After it come the three increments as value menus (icon, value, chevron), muted while snapping is off. The values stay visible, as in UE, but they are not three bordered splits.
- **Camera:** "Perspective ▾" with the FOV field beside it. **View:** the view mode, its icon and "Lit · High" (mode · quality). **Show:** the eye icon alone. **Frame:** an icon.
- **Narrow bars:** below 820 px the camera and view menus drop their words (camera icon, "Lit"), so nothing is ever cut off.

## Gizmos

Constant screen size; the arm is 88 px at 100 % gizmo size. Every stroke and fill has a 1 px dark halo (black at 60 %), so handles read over any background and over the selection outline. Axis colours are the `destructive-foreground`, `success-foreground` and `info-foreground` tokens. Hover is `warning-foreground`.

- **Move:**
  - shafts are 2 px (3 px hovered) and end in **solid 12 px cones**;
  - **plane handles are squares in their world plane**, between 20 % and 40 % along the two arms, at 20 % fill with a 1 px edge in the colour of their normal, and 45 % fill when hovered;
  - the centre is a 10 px ring for free movement.
- **Rotate:** three rings, 1.5 px wide (2.5 px hovered). The half facing away from the camera draws at 25 %, so the rings read as a sphere, not a tangle.
- **Scale:** 2 px shafts that end in **8 px filled cubes**, and a 10 px centre square for uniform scale.
- **Dragging:** the other handles fade to 15 %. A 1 px constraint line in the axis colour runs through the pivot, along the dragged axis or along both axes of the dragged plane.
- **Hit targets** stay at least 16 px. A plane handle hits anywhere inside its projected square.

Filled shapes (cones, plane squares, cubes) use one new overlay primitive, an antialiased convex quad (`Overlay::Quad`), drawn by the same UI pipeline as the lines.

## Axis widget

60 px, top-right with a 12 px inset, on a transparent background. Positive axes are 16 px filled discs lettered X, Y and Z. Negative axes are 10 px hollow rings with no letter. Stems run from the centre to the positive caps. Clicking a cap looks along that axis, and the centre returns to the default perspective. Tooltips read "Look along +X".

## Selection, grid and overlays

These already follow Appendix C:
- the selection is an accent edge outline from the entity-ID image, with no bounding box;
- agent changes keep their status tints;
- the grid adapts its spacing and fades with distance and grazing angle.

Overlays keep their anchored slots: orientation top-right, the loading pill top-left, agent notices bottom-left.
