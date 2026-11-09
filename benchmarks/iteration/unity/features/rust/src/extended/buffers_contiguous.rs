//! Safe table slices batch change-tick stamping while retaining real SmallVec work.
use super::model::{Case, Frame, Id, Value, Words, initial_word};
use bevy_ecs::{change_detection::ContiguousMut, prelude::*};

#[path = "buffer_words.rs"]
mod word_kernel;

pub struct Buffers {
    query: QueryState<(&'static Id, &'static mut Value, &'static mut Words)>,
}

impl Buffers {
    pub fn new(world: &mut World) -> Self {
        Self {
            query: world.query(),
        }
    }

    pub fn step(&mut self, world: &mut World, case: Case, frame: Frame) {
        let run = |(ids, values, words): (&[Id], ContiguousMut<Value>, ContiguousMut<Words>)| {
            // split stamps every selected writable row; no change detection is bypassed.
            let (values, _) = ContiguousMut::split(values);
            let (words, _) = ContiguousMut::split(words);
            for ((id, value), words) in ids.iter().zip(values).zip(words) {
                if case == Case::Resize && id.0 % 100 < frame.percent {
                    let length = if frame.tick.is_multiple_of(2) { 64 } else { 4 };
                    let previous = words.0.len();
                    words.0.resize(length, 0);
                    for index in previous..length {
                        words.0[index] = initial_word(id.0, index);
                    }
                    if frame.trim {
                        words.0.shrink_to_fit();
                    }
                }
                let sum = word_kernel::update_words(
                    words.0.as_mut_slice(),
                    id.0.wrapping_mul(64),
                    frame.tick,
                );
                value.value = value.value.wrapping_add(sum);
                value.updates += 1;
            }
        };
        if frame.parallel {
            self.query
                .contiguous_par_iter_mut(world)
                .expect("table-only buffer columns support contiguous iteration")
                .for_each(run);
        } else {
            self.query
                .contiguous_iter_mut(world)
                .expect("table-only buffer columns support contiguous iteration")
                .for_each(run);
        }
    }
}
