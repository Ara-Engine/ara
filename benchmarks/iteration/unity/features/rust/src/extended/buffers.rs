//! Explicit buffer kernels; the default preserves the original per-row baseline.
use super::model;

#[cfg(not(feature = "buffer_contiguous"))]
#[path = "buffers_baseline.rs"]
mod implementation;

#[cfg(feature = "buffer_contiguous")]
#[path = "buffers_contiguous.rs"]
mod implementation;

pub use implementation::Buffers;

#[cfg(not(feature = "buffer_contiguous"))]
pub const KERNEL: &str = "baseline-row-mut-scalar";
#[cfg(all(feature = "buffer_contiguous", not(feature = "buffer_simd")))]
pub const KERNEL: &str = "contiguous-scalar";
#[cfg(all(
    feature = "buffer_simd",
    target_arch = "aarch64",
    target_feature = "neon"
))]
pub const KERNEL: &str = "contiguous-neon4";
#[cfg(all(
    feature = "buffer_simd",
    not(all(target_arch = "aarch64", target_feature = "neon"))
))]
pub const KERNEL: &str = "contiguous-scalar-fallback";

#[cfg(all(test, feature = "buffer_contiguous"))]
#[path = "buffers_baseline.rs"]
mod baseline_test;

#[cfg(all(test, feature = "buffer_contiguous"))]
mod tests {
    use super::{
        Buffers, baseline_test,
        model::{self, Case, Frame, Id, Value, Words},
    };
    use bevy_ecs::prelude::*;

    #[derive(Debug, PartialEq, Eq)]
    struct State {
        id: u32,
        value: u32,
        updates: u32,
        words: Vec<u32>,
        capacity: usize,
        spilled: bool,
        value_ticks: (u32, u32),
        words_ticks: (u32, u32),
    }

    fn snapshot(world: &mut World) -> Vec<State> {
        let mut query = world.query::<(Entity, &Id, &Value, &Words)>();
        let mut rows = query
            .iter(world)
            .map(|(entity, id, value, words)| {
                let row = world.entity(entity);
                let value_ticks = row.get_change_ticks::<Value>().expect("Value ticks");
                let words_ticks = row.get_change_ticks::<Words>().expect("Words ticks");
                State {
                    id: id.0,
                    value: value.value,
                    updates: value.updates,
                    words: words.0.to_vec(),
                    capacity: words.0.capacity(),
                    spilled: words.0.spilled(),
                    value_ticks: (value_ticks.added.get(), value_ticks.changed.get()),
                    words_ticks: (words_ticks.added.get(), words_ticks.changed.get()),
                }
            })
            .collect::<Vec<_>>();
        rows.sort_by_key(|row| row.id);
        rows
    }

    #[test]
    fn contiguous_buffer_steps_keep_every_value_tick_and_storage_transition() {
        let mut cases = vec![];
        for length in [0, 4, 16, 64] {
            cases.push((Case::Fixed, length, 10, false));
        }
        for percent in [0, 10, 100] {
            for trim in [false, true] {
                cases.push((Case::Resize, 4, percent, trim));
            }
        }
        bevy_tasks::ComputeTaskPool::get_or_init(|| {
            bevy_tasks::TaskPoolBuilder::new().num_threads(2).build()
        });
        for parallel in [false, true] {
            for &(case, length, percent, trim) in &cases {
                let frame = Frame {
                    tick: 0,
                    count: 1003,
                    percent,
                    clustered: false,
                    length,
                    trim,
                    fork_join: false,
                    parallel,
                };
                let (mut reference, _) = model::setup(case, frame);
                let (mut candidate, _) = model::setup(case, frame);
                let mut before = baseline_test::Buffers::new(&mut reference);
                let mut after = Buffers::new(&mut candidate);
                assert_eq!(snapshot(&mut reference), snapshot(&mut candidate));
                for tick in 0..6 {
                    reference.increment_change_tick();
                    candidate.increment_change_tick();
                    let current = Frame { tick, ..frame };
                    before.step(&mut reference, case, current);
                    after.step(&mut candidate, case, current);
                    assert_eq!(snapshot(&mut reference), snapshot(&mut candidate));
                }
            }
        }
    }
}
