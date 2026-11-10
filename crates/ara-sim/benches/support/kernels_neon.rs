//! ARM64 lane operations with the same scalar IEEE math contract.
use core::arch::aarch64::*;
use glam::{Mat4, Vec4};

/// A 4x4 transpose is only a permutation of the original float bits.
#[inline]
pub(super) fn transpose(matrix: Mat4) -> Mat4 {
    let columns = matrix.to_cols_array_2d();
    let mut result = [[0.0; 4]; 4];
    // SAFETY: AArch64 NEON is selected by the module cfg. Each load/store covers
    // exactly four initialized floats in a live stack array; alignment is unrestricted.
    unsafe {
        let a = vld1q_f32(columns[0].as_ptr());
        let b = vld1q_f32(columns[1].as_ptr());
        let c = vld1q_f32(columns[2].as_ptr());
        let d = vld1q_f32(columns[3].as_ptr());
        let ab0 = vreinterpretq_f64_f32(vzip1q_f32(a, b));
        let ab1 = vreinterpretq_f64_f32(vzip2q_f32(a, b));
        let cd0 = vreinterpretq_f64_f32(vzip1q_f32(c, d));
        let cd1 = vreinterpretq_f64_f32(vzip2q_f32(c, d));
        vst1q_f32(
            result[0].as_mut_ptr(),
            vreinterpretq_f32_f64(vzip1q_f64(ab0, cd0)),
        );
        vst1q_f32(
            result[1].as_mut_ptr(),
            vreinterpretq_f32_f64(vzip2q_f64(ab0, cd0)),
        );
        vst1q_f32(
            result[2].as_mut_ptr(),
            vreinterpretq_f32_f64(vzip1q_f64(ab1, cd1)),
        );
        vst1q_f32(
            result[3].as_mut_ptr(),
            vreinterpretq_f32_f64(vzip2q_f64(ab1, cd1)),
        );
    }
    Mat4::from_cols_array_2d(&result)
}

/// Uses hardware sqrt in four lanes, preserving each independent scalar result.
#[inline]
pub(super) fn sqrt(values: Vec4) -> Vec4 {
    let values = values.to_array();
    let mut result = [0.0; 4];
    // SAFETY: The module cfg requires NEON. Both arrays contain four live f32s,
    // and vsqrtq_f32 applies IEEE sqrt rather than a reciprocal approximation.
    unsafe { vst1q_f32(result.as_mut_ptr(), vsqrtq_f32(vld1q_f32(values.as_ptr()))) };
    Vec4::from_array(result)
}
