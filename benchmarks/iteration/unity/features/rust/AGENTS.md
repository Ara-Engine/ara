# ECS feature fixture

Standalone benchmark crate, outside the engine workspace. Own only this fixture.
It exercises real Bevy ECS query APIs and exports the independent feature contract.
Gameplay state stays in components; setup and export are outside the timed loop.

- `main.rs`: bounded CLI, pool setup and recorded samples.
- `world.rs`: retained filtered QueryState and integer component updates.
- `protocol.rs`: full-ID/tag/value/update export compatible with reference.py.
- `extended/`: separate binary for dirty epochs, table-tag churn, real dependency
  schedules and component-owned SmallVec buffers. Keep the query binary's
  numerical source unchanged. The standalone lock must match production Bevy.
- `buffers_baseline.rs` retains the original per-row loop. Opt-in
  `buffer_contiguous` batches change stamping; `buffer_simd` adds exact NEON word
  updates where supported, with scalar tails/fallback. Default features stay empty.

The extended scheduled cases configure a separate pool budget from direct
par_iter cases. Record configured workers and separate diagnostic observations;
do not treat either as measured simultaneous activity. Dirty candidate counts
come from a separately exported/hash-compared replay, outside throughput samples.

Build only in a parent-granted CPU slot, with two Cargo jobs. Use the feature
harness command documented one directory above; outputs/targets stay under the
checkout's target/. Never run Cargo during a timed engine process.

Compare small/partial-count exports against reference.py and Unity before timing.
Do not replace queries with raw arrays, omit change ticks or infer tags during
export. Record actual tag presence and reject duplicate/out-of-range stable IDs.
Do not change the frozen S8 fixtures or production APIs for benchmark advantage.
The harness selects variants with `--buffer-kernel baseline|contiguous|simd` and
isolates their targets/results. Bind Cargo features and actual target capabilities
to build provenance; never infer the kernel from matching source hashes alone.
