# ara-vfx

Typed ordered VFX stacks, ID-free JSON authoring, graph projection, reversible
commands, Slang generation and the deterministic CPU reference simulator.

## API

- `Effect::parse`, `text`, `validate`, `resolve`: strict canonical text and bindings.
- `Effect::to_graph`, `from_graph`, `VfxAdapter`: shared graph documents.
- `commands::register`: text apply, insert/remove/move module and set parameter.
- `CpuSystem`: fixed ticks, deterministic SoA storage and authoritative events.
- `generate`: specialized Slang module functions for offline cooking.
- `samples`: six authored effects, also used by GPU captures; `library()`/`find(name)`
  are the names `VisualEffect.effect` resolves (+ `depth_sparks`, `dust`).

## Contracts

Names are persistent graph identities; reordering a named module preserves its
node ID. Text renames create new identity. All live writes go through commands.
Particle parameters are immutable per-particle defaults; mutable particle state
lives only in simulation SoA channels. Runtime overrides are checked by type.
CPU instances belong inside ECS components, never in globals or C# objects.
GPU events are cosmetic; gameplay consumes only CpuSystem's authoritative events.
Fixed dt is 1 / effect.hz. Events spawn at the next tick, before update.
Shader compilation belongs in a worker; publish at frame boundaries (D20).

## Checks

Set CARGO_BUILD_JOBS=2. Run cargo check/test/clippy -p ara-vfx, with
--all-targets -- -D warnings for clippy; cargo fmt -p ara-vfx -- --check.
No compiler, SDK or external tools are required by this crate's tests.
