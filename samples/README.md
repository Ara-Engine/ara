# Ara samples

Real-art test projects, authored through Ara's own commands by `ara-samples`
(`crates/ara-samples`). No third-party binary is in git: `assets.lock.json` pins every
file (URL, BLAKE3, size, license, authors, source page) and `fetch` restores them.

| Project | Scenes |
|---|---|
| `outpost/` | `levels/outpost.scene`: Outpost at dusk (a derelict compound on 384 m of terrain with foliage, a campfire, floodlights, wind and music). `levels/hangar.scene`: the hangar interior at night, lit by emissive panels, a festoon and a few lamps. |
| `showcase/` | `levels/showcase.scene`: a turntable stage with an animated mannequin, a jogger, 11 hero props on plinths and 18 material balls under a studio HDRI. |

```sh
cargo build --release -p ara -p ara-player -p ara-import-worker -p ara-editor -p ara-samples
target/release/ara-samples fetch                        # ~510 MB, resumable, hash-checked
target/release/ara-samples build outpost                # and: build showcase
target/release/ara-samples capture outpost outpost      # looks: outpost, hangar; showcase showcase
target/release/ara-samples play outpost levels/hangar.scene hangar
target/release/ara-samples editor showcase levels/showcase.scene showcase
```

Builds use the same document, import, animation, music and scene commands as CLI/MCP
agents. Variant sheets become native prefabs with source-node selectors; lamp
emission and suit materials are component overrides. The original Poly Haven fence
uses its texture transforms and companion opacity maps during import. Terrain uses
its authored signed origin, with native material graphs scaled in world metres.
Colliders share the prop entity and automatic physics identities. Rebuilds preserve
the attributed journal and keep stable document, stroke and exclusion IDs.

The projects are ordinary Ara projects: `ara check`, `ara mcp` and the editor work in
them as in any other. GPU commands need the Vulkan SDK (see `crates/ara-samples/AGENTS.md`).

- `<project>/look/*.look.json`: what a scene cannot express yet (physical sky, fog,
  exposure, camera shots) for the fidelity captures.
- `<project>/evidence/`: fidelity captures (1440p native and upscaled, a sequence
  strip), `*-timings.json` (per-pass GPU timings, content stats), Play and editor
  captures.
- [`FINDINGS.md`](FINDINGS.md): engine bugs and friction found building these, with
  repros. [`THIRD_PARTY_ASSETS.md`](THIRD_PARTY_ASSETS.md): attribution.
