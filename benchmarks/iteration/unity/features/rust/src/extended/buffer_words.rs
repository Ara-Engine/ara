//! Exact wrapping-u32 word updates; NEON changes lane width, not the state contract.

/// Updates all live elements and returns their wrapping sum, including scalar tails.
#[inline]
pub fn update_words(words: &mut [u32], first_word_id: u32, tick: u32) -> u32 {
    #[cfg(all(
        feature = "buffer_simd",
        target_arch = "aarch64",
        target_feature = "neon"
    ))]
    {
        // SAFETY: this branch exists only for AArch64 binaries whose compile-time
        // target contract enables NEON; the called kernel requires that feature.
        unsafe { neon(words, first_word_id, tick) }
    }
    #[cfg(not(all(
        feature = "buffer_simd",
        target_arch = "aarch64",
        target_feature = "neon"
    )))]
    {
        scalar(words, first_word_id, tick)
    }
}

#[inline]
fn scalar(words: &mut [u32], first_word_id: u32, tick: u32) -> u32 {
    let mut sum = 0_u32;
    for (index, word) in words.iter_mut().enumerate() {
        *word = super::super::model::update(*word, first_word_id.wrapping_add(index as u32), tick);
        sum = sum.wrapping_add(*word);
    }
    sum
}

#[cfg(all(
    feature = "buffer_simd",
    target_arch = "aarch64",
    target_feature = "neon"
))]
#[inline]
#[target_feature(enable = "neon")]
fn neon(words: &mut [u32], first_word_id: u32, tick: u32) -> u32 {
    use core::arch::aarch64::*;

    #[inline]
    #[target_feature(enable = "neon")]
    fn mix(mut value: uint32x4_t) -> uint32x4_t {
        value = vmulq_u32(
            veorq_u32(value, vshrq_n_u32::<16>(value)),
            vdupq_n_u32(0x7feb_352d),
        );
        value = vmulq_u32(
            veorq_u32(value, vshrq_n_u32::<15>(value)),
            vdupq_n_u32(0x846c_a68b),
        );
        veorq_u32(value, vshrq_n_u32::<16>(value))
    }

    let lanes = vsetq_lane_u32::<3>(
        3,
        vsetq_lane_u32::<2>(2, vsetq_lane_u32::<1>(1, vdupq_n_u32(0))),
    );
    let multiplier = vdupq_n_u32(1_664_525);
    let salt = vdupq_n_u32(1_013_904_223);
    let mut sums = vdupq_n_u32(0);
    let (chunks, tail) = words.as_chunks_mut::<4>();
    let mut offset = 0_u32;
    for chunk in chunks {
        let ids = vaddq_u32(
            vdupq_n_u32(first_word_id.wrapping_add(offset).wrapping_add(tick)),
            lanes,
        );
        // SAFETY: chunk is exactly four initialized u32s. AArch64 vld1q_u32 accepts
        // ordinary u32 alignment; no 16-byte alignment or out-of-slice read is used.
        let before = unsafe { vld1q_u32(chunk.as_ptr()) };
        let after = vaddq_u32(vmulq_u32(before, multiplier), veorq_u32(salt, mix(ids)));
        // SAFETY: this exclusive four-word chunk covers the complete 16-byte store;
        // vst1q_u32 accepts its ordinary u32 alignment and no other chunk overlaps it.
        unsafe { vst1q_u32(chunk.as_mut_ptr(), after) };
        sums = vaddq_u32(sums, after);
        offset = offset.wrapping_add(4);
    }
    // Modulo-2^32 addition is associative, so lane reduction preserves the scalar sum.
    vaddvq_u32(sums).wrapping_add(scalar(tail, first_word_id.wrapping_add(offset), tick))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_lane_width_preserves_wrapping_state_and_unaligned_tails() {
        // Offset slices exercise ordinary u32 alignment, tails, IDs and tick wrap.
        for length in [0, 1, 3, 4, 5, 7, 16, 63, 64] {
            for first in [0, 1, u32::MAX - 2, u32::MAX] {
                let initial = (0..length + 1)
                    .map(|index| (index as u32).wrapping_mul(0xfedc_ba98))
                    .collect::<Vec<_>>();
                let mut reference = initial.clone();
                let mut candidate = initial;
                for tick in [0, 1, u32::MAX - 1, u32::MAX] {
                    let expected_sum = scalar(&mut reference[1..], first, tick);
                    let actual_sum = update_words(&mut candidate[1..], first, tick);
                    assert_eq!(candidate, reference);
                    assert_eq!(actual_sum, expected_sum);
                }
            }
        }
    }
}
