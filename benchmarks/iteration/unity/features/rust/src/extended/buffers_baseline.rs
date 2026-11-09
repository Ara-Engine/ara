//! Component-owned SmallVec payloads, including real spill/resize/trim work.
use super::model::{Case, Frame, Id, Value, Words, initial_word, update};
use bevy_ecs::prelude::*;

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
        let run = |(id, mut value, mut words): (&Id, Mut<Value>, Mut<Words>)| {
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
            let mut sum = 0_u32;
            for (index, word) in words.0.iter_mut().enumerate() {
                *word = update(
                    *word,
                    id.0.wrapping_mul(64).wrapping_add(index as u32),
                    frame.tick,
                );
                sum = sum.wrapping_add(*word);
            }
            value.value = value.value.wrapping_add(sum);
            value.updates += 1;
        };
        if frame.parallel {
            self.query.par_iter_mut(world).for_each(run);
        } else {
            self.query.iter_mut(world).for_each(run);
        }
    }
}
