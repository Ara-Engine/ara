//! Golden comparison shared by the geometry tests.

/// Whether RGBA8 `actual` matches a golden rendered on Windows (Vulkan). Vulkan must
/// match it byte for byte. Metal's rasterizer and texture filtering differ in the last
/// bits, so there at most 0.05% of pixels (silhouette tie-breaks) may differ by more
/// than 16 levels and the mean channel difference must stay under one level.
pub fn matches_golden(actual: &[u8], expected: &[u8]) -> bool {
    if cfg!(not(target_os = "macos")) {
        return actual == expected;
    }
    if actual.len() != expected.len() || !actual.len().is_multiple_of(4) {
        return false;
    }
    let pixels = actual.len() / 4;
    let mut far = 0;
    let mut total = 0u64;
    let (actual_pixels, _) = actual.as_chunks::<4>();
    let (expected_pixels, _) = expected.as_chunks::<4>();
    for (a, b) in actual_pixels.iter().zip(expected_pixels) {
        let diff = std::array::from_fn::<u8, 4, _>(|c| a[c].abs_diff(b[c]));
        far += usize::from(diff.iter().any(|d| *d > 16));
        total += diff.iter().map(|d| u64::from(*d)).sum::<u64>();
    }
    let mean = total as f64 / actual.len() as f64;
    let matched = far * 2000 <= pixels && mean < 1.0;
    if !matched {
        eprintln!("golden: {far} of {pixels} pixels differ by more than 16, mean {mean:.3}");
    }
    matched
}
