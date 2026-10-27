//! Dispatch/check overhead against the original direct call; no GPU or live patch needed.
use std::{hint::black_box, time::Instant};

#[inline(never)]
fn body(value: u64) -> u64 {
    value.wrapping_mul(3).wrapping_add(7)
}

fn sample(mut call: impl FnMut(u64) -> u64) -> f64 {
    const CALLS: u64 = 2_000_000;
    let start = Instant::now();
    for value in 0..CALLS {
        black_box(call(black_box(value)));
    }
    start.elapsed().as_secs_f64() * 1e9 / CALLS as f64
}

fn main() {
    let mut patches = ara_hotpatch::Hotpatch::connect();
    let mut rows = Vec::new();
    for _ in 0..5 {
        let direct = sample(body);
        let hot = sample(|value| ara_hotpatch::call!(body, value));
        let idle_check = sample(|_| u64::from(patches.frame_boundary()));
        rows.push(
            serde_json::json!({"direct_ns":direct,"hot_call_ns":hot,"idle_check_ns":idle_check}),
        );
    }
    println!(
        "{}",
        serde_json::json!({"calls_per_sample":2_000_000,"samples":rows,"live_patch_table":false})
    );
}
