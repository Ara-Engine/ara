# ECS feature fixture

Standalone benchmark crate, outside the engine workspace. Own only this fixture.
It exercises real Bevy ECS query APIs and exports the independent feature contract.
Gameplay state stays in components; setup and export are outside the timed loop.

- `main.rs`: bounded CLI, pool setup and recorded samples.
- `world.rs`: retained filtered QueryState and integer component updates.
- `protocol.rs`: full-ID/tag/value/update export compatible with reference.py.

Build only in a parent-granted CPU slot, with two Cargo jobs. Use the feature
harness command documented one directory above; outputs/targets stay under the
checkout's target/. Never run Cargo during a timed engine process.

Compare small/partial-count exports against reference.py and Unity before timing.
Do not replace queries with raw arrays, omit change ticks or infer tags during
export. Record actual tag presence and reject duplicate/out-of-range stable IDs.
Do not change the frozen S8 fixtures or production APIs for benchmark advantage.
