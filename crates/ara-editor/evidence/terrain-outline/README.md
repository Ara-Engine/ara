# Terrain selection outline

> **Historical evidence.** Captured from the previous editor shell, which was removed in #53: the `ara-editor --verify…` routes and `cargo test --test native` commands named here no longer exist. Kept as the visual record.

Owner report (preview 14): a thin white polyline on Outpost's left summit, the outline
of one terrain chunk (its ridge plus a vertical chunk border).

Offscreen `ara-editor <Outpost> --select <entity> --frames 1500 --screenshot` on a
Hub-created Outpost with its imports cached, 1440×900, Ara Dark. Before: main
`2af190b2` plus only `--select`. After: this fix.

| Selected | Before | After |
|---|---|---|
| Terrain (Hierarchy) | No outline: tiles drew with geometry-hash IDs no entity matched | One silhouette along the skyline: no chunk borders, no loops around what stands on the terrain |
| Summit chunk `e_chunk8z1` | No outline | The chunk's own shape: skyline and its border with the neighbouring tiles |

Hover and clicks use the same map: hovering a terrain tile outlines the whole terrain
(before, hover outlined the one tile under the pointer, most likely the reported
polyline) and clicking one selects the Terrain. Hover is not in these captures:
offscreen runs have no pointer.
