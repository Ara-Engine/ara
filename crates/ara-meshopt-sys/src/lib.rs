//! Owned, exclusive contexts for the importer's fixed attribute-aware LOD mode.
use std::{ffi::c_void, marker::PhantomData, ptr::NonNull, rc::Rc};

unsafe extern "C" {
    fn ara_lod_begin(
        indices: *const u32,
        index_count: usize,
        positions: *const f32,
        vertex_count: usize,
        attributes: *const f32,
        weights: *const f32,
        attribute_count: usize,
        locks: *const u8,
    ) -> *mut c_void;
    fn ara_lod_reset(
        context: *mut c_void,
        destination: *mut u32,
        target_count: usize,
        error: f32,
        result_error: *mut f32,
    ) -> usize;
    fn ara_lod_end(context: *mut c_void);
}

/// Invalid input or a native allocation/setup failure. No partial result is served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error(&'static str);
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Error {}

/// Owns pristine inputs and working state for LockBorder|Permissive|RegularizeLight.
/// Each solve restores LOD0 state. Contexts are confined to their creating worker;
/// independent contexts may run concurrently. No input borrow survives construction.
pub struct Prepared {
    native: NonNull<c_void>,
    index_count: usize,
    _worker: PhantomData<Rc<()>>,
}
impl Prepared {
    /// Builds initial quadrics and ranking once. Attributes are tightly packed,
    /// with one positive finite weight per channel (at most 32); locks use bits 0–1.
    pub fn new(
        indices: &[u32],
        positions: &[[f32; 3]],
        attributes: &[f32],
        weights: &[f32],
        locks: &[u8],
    ) -> Result<Self, Error> {
        let vertices = positions.len();
        if vertices == 0
            || vertices > u32::MAX as usize
            || indices.is_empty()
            || !indices.len().is_multiple_of(3)
            || indices.len() > u32::MAX as usize
        {
            return Err(Error("invalid mesh sizes"));
        }
        if weights.is_empty()
            || weights.len() > 32
            || vertices.checked_mul(weights.len()) != Some(attributes.len())
            || locks.len() != vertices
        {
            return Err(Error("invalid attribute or lock layout"));
        }
        if positions.iter().flatten().any(|v| !v.is_finite())
            || attributes.iter().any(|v| !v.is_finite())
            || weights.iter().any(|v| !v.is_finite() || *v <= 0.0)
            || locks.iter().any(|v| v & !3 != 0)
            || indices.iter().any(|&v| v as usize >= vertices)
        {
            return Err(Error("invalid vertex, weight, index or lock value"));
        }
        // SAFETY: All typed, aligned slices have validated lengths and finite data.
        // Native construction copies/rescales every input it retains and catches exceptions.
        let native = unsafe {
            ara_lod_begin(
                indices.as_ptr(),
                indices.len(),
                positions.as_ptr().cast(),
                vertices,
                attributes.as_ptr(),
                weights.as_ptr(),
                weights.len(),
                locks.as_ptr(),
            )
        };
        Ok(Self {
            native: NonNull::new(native).ok_or(Error("native prepared LOD allocation failed"))?,
            index_count: indices.len(),
            _worker: PhantomData,
        })
    }
    /// Restores pristine state and solves one target, returning upstream indices
    /// and relative geometric error. Target order does not affect subsequent calls.
    pub fn simplify(&mut self, target_count: usize, error: f32) -> Result<(Vec<u32>, f32), Error> {
        if target_count > self.index_count || !error.is_finite() || error < 0.0 {
            return Err(Error("invalid LOD target or error"));
        }
        let mut output = vec![0; self.index_count];
        let mut result_error = 0.0;
        // SAFETY: This exclusive context is live; output has its original index capacity.
        // Native code catches exceptions and reports failure without exposing partial output.
        let count = unsafe {
            ara_lod_reset(
                self.native.as_ptr(),
                output.as_mut_ptr(),
                target_count,
                error,
                &mut result_error,
            )
        };
        if count > self.index_count || !count.is_multiple_of(3) || !result_error.is_finite() {
            return Err(Error("native prepared LOD solve failed"));
        }
        output.truncate(count);
        Ok((output, result_error))
    }
}
impl Drop for Prepared {
    fn drop(&mut self) {
        // SAFETY: This unique owner releases its native context exactly once.
        unsafe {
            ara_lod_end(self.native.as_ptr());
        }
    }
}
