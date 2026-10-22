# Ara

A Rust game engine that AI agents build, and that agents and humans use to make games.

- **AI-native.** The engine's real API is a typed schema plus a command/query system over a world stored as text. The editor, the `ara` CLI, C# scripts, AI agents (through MCP) and the netcode all go through it.
- **Unreal-level fidelity.** Own native GPU backends (Vulkan 1.3, Metal 4); Slang shaders; RT global illumination; virtualized geometry over time.
- **Unity-level ease.** C# gameplay with sub-second hot reload; a native editor; prefabs; an inspector generated from the schema.
- **One native UI system** for the editor, game HUDs and world UI: web-shaped, not a browser.
- **Multiplayer native from day one**, while single-player pays nothing for it.
- **Targets:** Windows, Linux and macOS first; Android and iOS next.

**Status:** pre-M0. The research is done; the foundation spikes are next.

## Docs

- [Decisions](docs/decisions.md): what is locked, and why.
- [Milestones](docs/plan/milestones.md): build order M0–M5, including the dogfood game *Outpost*.
- [Research synthesis](docs/research/00-synthesis.md), plus ten research reports (R01–R10).
- [`AGENTS.md`](AGENTS.md): working rules for agents (and humans).

## Getting started

```sh
rustup show            # installs the pinned toolchain from rust-toolchain.toml
cargo check --workspace
cargo run -p ara -- --version
```

The reference engine clones used for study are not committed. See `AGENTS.md` for how to fetch them.

## License

Not decided yet. All rights reserved until then.
