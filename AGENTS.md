# AGENTS.md: Ara

Ara is a Rust game engine. AI agents build it, and agents and humans use it to make games. Pillars:
- AI-native;
- Unreal-level fidelity;
- Unity-level ease;
- a great native editor;
- great authoring tools for both AI and humans.

**Read first:**
- [`docs/decisions.md`](docs/decisions.md): the locked decisions.
- [`docs/plan/milestones.md`](docs/plan/milestones.md): what is being built now.
- [`docs/research/`](docs/research/00-synthesis.md): the evidence behind both.

## Status

M0 is done: every spike decision is in `docs/decisions.md`. M1–M4 tracks are being built in parallel.

**Merged:**
- the kernel (`ara-schema`, `ara-doc`, `ara-ops`, `ara-cmd`, `ara-merge`, `ara-json`, `ara-diag`);
- the Engine Protocol, daemon and MCP (`ara-proto`, `ara-host`, `ara-rpc`, `ara-mcp`, the `ara` CLI);
- `arabench`, the agent-ergonomics benchmark;
- the simulation core (`ara-sim`, `ara-input`, `ara-components`);
- the C# host (`ara-script`, `dotnet/`);
- the renderer (`ara-gpu`, `ara-render`, `ara-platform`, `ara-shader-cook`), fidelity (`ara-render-fidelity`, `ara-pathtrace`), and materials (`ara-graph`, `ara-material`);
- assets (`ara-asset*`, `ara-importers`);
- physics (`ara-physics*`), audio (`ara-audio`), navigation (`ara-nav`), animation (`ara-anim`);
- the UI runtime and tooling (`ara-ui-*`);
- export (`ara-export*`, `ara-pack`, `ara-bridge`);
- localization and saves (`ara-l10n`, `ara-save`).

**In progress:** the editor shell, multiplayer (`ara-net`), runtime integration (`ara-runtime`, `ara-player`), VFX, terrain, 2D, world streaming, profiling, vendored SDL3, and C# host v2. Run `ara init` in a scratch dir to try the CLI and MCP.

GPU tests need `VULKAN_SDK` set (slangc is found through it).

## Layout

```
crates/          Rust workspace members (engine crates; `ara` CLI)
spikes/          M0 throwaway experiments, outside the workspace; each ends with RESULT.md
docs/            decisions.md, plan/, research/
reports/         HTML reports published to Snist
.refs/           reference engine clones (NOT committed; see below)
```

## Commands

- `cargo check -p <crate>`: the fast loop. Check only the crates you touched.
- `cargo clippy -p <crate> --all-targets`: no new warnings.
- `cargo test -p <crate>`: tests for the crates you touched.
- `cargo fmt --all`

There is no CI (D9). These local commands are the only gates, so run them before you call a task done.

## Rules

- **Locked decisions are not re-argued inside a task.** If you think one is wrong, say so in your summary and keep following it until a new entry in `docs/decisions.md` supersedes it.
- **No scope cuts.** Plans and docs describe build order, never dropping features.
- **Every authoring mutation is a typed command that produces ops.** No side-channel writes to documents, from the editor or from plugins.
- **Gameplay state lives in ECS components only.** Never in C# objects, Rust statics or globals.
- **Simulation code is deterministic:**
  - fixed tick;
  - RNG from the context only;
  - no wall-clock time;
  - no iteration order that affects results unless that order is defined.
- **Every public reflected type and field has a doc comment.** These become inspector tooltips, docs and agent tool descriptions.
- **Text formats go through the canonical writer.** Never hand-order keys. References are `@path#id`.
- **Performance contract:**
  - In per-frame code, never silence the borrow checker with `Arc<Mutex<_>>`, `Rc<RefCell<_>>` or `.clone()`. Use owned types, handles and short ECS borrows.
  - Measure (Tracy or a benchmark) before and after any hot-path change.
- **No `unwrap()` outside tests.** Use `expect("…")` only to state a real invariant. Every `unsafe` block gets a `// SAFETY:` comment.
- **Build prerequisites are fixed.** A fresh clone must `cargo check`/`cargo test` with only Rust, the MSVC/Clang C/C++ compiler, the Vulkan SDK (shader cooking) and the .NET 10 SDK (C# host). Vendored C/C++ is built with `cc` from `build.rs`, following the Jolt shim pattern. No CMake, Python or other build-time tools. Optional runtime downloads, such as the Steam Audio runtime, must be clearly diagnosed setup steps, never build requirements.
- **No exact `=` version pins in crate manifests.** Use caret ranges (`"1.8"`); `Cargo.lock` pins the exact versions. Exact pins in two crates conflict as soon as both are in the workspace. Shared dependencies are moving to root `[workspace.dependencies]`; ask the parent before adding a new one there.
- **Keep crates and files small**, split along real dependency boundaries. Each crate has an `AGENTS.md` of 150 lines or fewer: purpose, public API, how to test, gotchas.
- **Comments:** short doc comments on types and functions explaining how they are used. Update them whenever you change the behaviour.
- **Spikes:** `spikes/<id>-<name>/`, own `Cargo.toml` (not a workspace member), throwaway. They finish with `RESULT.md` (numbers, findings, recommendation) and an entry in `docs/decisions.md`.

## Reference code (`.refs/`, not committed)

| Source | How it may be used |
|---|---|
| Godot (MIT), O3DE (Apache-2.0 / MIT) | May be copied. Keep the original headers and add an entry to `THIRD_PARTY_NOTICES.md`. |
| Flax, UnityCsReference, Unreal | Study them for patterns, then write our own implementation. Never transcribe. |
| s&box (MIT) | Prior art for C# hotload, the API whitelist and Razor UI. Clone it on demand. |

Fetch the clones (shallow, with LFS skipped):

```sh
mkdir -p .refs && cd .refs && export GIT_LFS_SKIP_SMUDGE=1
git clone --depth 1 https://github.com/godotengine/godot.git godot
git clone --depth 1 https://github.com/FlaxEngine/FlaxEngine.git flax
git clone --depth 1 https://github.com/o3de/o3de.git o3de
git clone --depth 1 https://github.com/Unity-Technologies/UnityCsReference.git unity-cs-reference
```

## Machines

- **Windows (primary, RTX GPU):** Vulkan development and the main editor host.
- **MacBook Pro M5:** Metal 4, macOS and iOS builds.
- Line endings are LF everywhere (`.gitattributes`).
