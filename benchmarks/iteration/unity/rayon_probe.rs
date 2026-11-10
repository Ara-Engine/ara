//! Diagnostic for S8 W4's Rayon recording pool, run separately from timed kernels.
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let workers: usize = std::env::args()
        .nth(1)
        .ok_or("usage: rayon-probe <parallel workers: 2..=63>")?
        .parse()?;
    if !(2..=63).contains(&workers) {
        return Err("parallel workers must be in 2..=63; W4 argument 1 runs directly on the caller".into());
    }
    rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .build_global()?;
    let caller = std::thread::current().id();
    let caller_recorded = AtomicBool::new(false);
    let worker_bits = AtomicU64::new(0);
    let active = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    let mut records = vec![0_u64; 10_000];
    // Match W4's par_iter_mut/enumerate/for_each entry path. Extra arithmetic
    // makes worker participation observable; this is not a throughput result.
    records.par_iter_mut().enumerate().for_each(|(index, row)| {
        if std::thread::current().id() == caller {
            caller_recorded.store(true, Ordering::Relaxed);
        }
        if let Some(worker) = rayon::current_thread_index() {
            worker_bits.fetch_or(1 << worker, Ordering::Relaxed);
        }
        let running = active.fetch_add(1, Ordering::Relaxed) + 1;
        peak.fetch_max(running, Ordering::Relaxed);
        let mut value = index as u64;
        for _ in 0..512 {
            value = std::hint::black_box(value)
                .wrapping_mul(0x9e37_79b9_7f4a_7c15)
                .wrapping_add(1);
        }
        *row = value;
        active.fetch_sub(1, Ordering::Relaxed);
    });
    let observed = worker_bits.load(Ordering::Relaxed).count_ones();
    println!(
        "{{\"configured_workers\":{workers},\"reported_pool_workers\":{},\"observed_worker_threads\":{observed},\"observed_peak_recorders\":{},\"caller_recorded\":{}}}",
        rayon::current_num_threads(),
        peak.load(Ordering::Relaxed),
        caller_recorded.load(Ordering::Relaxed),
    );
    Ok(())
}
