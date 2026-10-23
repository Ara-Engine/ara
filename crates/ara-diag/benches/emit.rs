//! What emitting costs: `cargo bench -p ara-diag --bench emit`.
//!
//! Prints nanoseconds per call for the paths a hot loop can hit: the bus off (no
//! consumer), a sampled-out `emit!` (spam past its burst), an admitted emit of a
//! prebuilt diagnostic, an admitted `emit!` that formats, four threads at once, and
//! the host-side drain + ingest per event.

use std::hint::black_box;
use std::time::Instant;

use ara_diag::bus::{self, Event, Meta};
use ara_diag::store::Store;
use ara_diag::{Diagnostic, Subject, codes};

/// Runs `f` `n` times; returns ns per call (best of 5 rounds).
fn per_call(n: u32, mut f: impl FnMut(u32)) -> f64 {
    (0..5)
        .map(|_| {
            let t = Instant::now();
            for i in 0..n {
                f(i);
            }
            t.elapsed().as_nanos() as f64 / f64::from(n)
        })
        .fold(f64::INFINITY, f64::min)
}

fn report(what: &str, ns: f64) {
    println!("{what:<52} {ns:>9.1} ns");
}

fn main() {
    let n = 200_000;
    report(
        "admit, bus off (no consumer)",
        per_call(n, |i| {
            black_box(ara_diag::emit!(warning, codes::FRAME_OVER_BUDGET, "frame {i}"));
        }),
    );

    let consumer = bus::attach();
    let mut sink: Vec<Event> = Vec::with_capacity(bus::RING_CAPACITY);
    let drain = |sink: &mut Vec<Event>| {
        consumer.drain(sink);
        sink.clear();
    };

    // Past its burst the code is only counted: no formatting, no ring traffic.
    for _ in 0..=bus::DEFAULT_BURST {
        bus::admit(codes::FRAME_OVER_BUDGET);
    }
    report(
        "emit! sampled out (spam past its burst)",
        per_call(n, |i| {
            black_box(ara_diag::emit!(warning, codes::FRAME_OVER_BUDGET, "frame {i}"));
        }),
    );
    drain(&mut sink);

    // Admitted emits: drain every 256 calls so the ring never fills; only the emits
    // between drains are timed.
    let prebuilt = Diagnostic::warning(codes::RENDER_NO_MESH, "entity has no mesh")
        .subject(Subject::entity("e_crate001"));
    let admitted = |label: &str, f: &mut dyn FnMut(u32), sink: &mut Vec<Event>| {
        let (rounds, batch) = (400u32, 256u32);
        let mut total = 0u128;
        for r in 0..rounds {
            let t = Instant::now();
            for i in 0..batch {
                f(r * batch + i);
            }
            total += t.elapsed().as_nanos();
            consumer.drain(sink);
            sink.clear();
        }
        report(label, total as f64 / f64::from(rounds * batch));
    };
    bus::set_burst(codes::RENDER_NO_MESH, u32::MAX);
    bus::set_burst(codes::RENDER_DISABLED, u32::MAX);
    admitted(
        "emit, admitted, prebuilt diagnostic (clone + push)",
        &mut |_| {
            black_box(prebuilt.clone().emit());
        },
        &mut sink,
    );
    admitted(
        "emit!, admitted, formats its message",
        &mut |i| {
            black_box(ara_diag::emit!(warning, codes::RENDER_DISABLED, "entity e_{i} is hidden"));
        },
        &mut sink,
    );
    let clone_only = per_call(n, |_| {
        black_box(prebuilt.clone());
    });
    report("  (of which: cloning the prebuilt diagnostic)", clone_only);

    // Four producers at once, one consumer draining.
    let threads = 4;
    let per_thread = 100_000u32;
    let t = Instant::now();
    let workers: Vec<_> = (0..threads)
        .map(|_| {
            std::thread::spawn(move || {
                bus::set_burst(codes::RENDER_DISABLED, u32::MAX);
                let mut sent = 0u32;
                for i in 0..per_thread {
                    if ara_diag::emit!(warning, codes::RENDER_DISABLED, "e_{i}").is_some() {
                        sent += 1;
                    }
                }
                sent
            })
        })
        .collect();
    let mut store = Store::default();
    let mut drained = 0usize;
    let mut ingest_ns = 0u128;
    while workers.iter().any(|w| !w.is_finished()) {
        let lost = consumer.drain(&mut sink);
        drained += sink.len();
        let ti = Instant::now();
        store.ingest(std::mem::take(&mut sink));
        store.absorb(&lost);
        ingest_ns += ti.elapsed().as_nanos();
    }
    let sent: u32 = workers.into_iter().map(|w| w.join().unwrap_or(0)).sum();
    let lost = consumer.drain(&mut sink);
    drained += sink.len();
    store.ingest(std::mem::take(&mut sink));
    store.absorb(&lost);
    let elapsed = t.elapsed().as_nanos() as f64;
    report(
        &format!("{threads} threads × {per_thread} emits, wall time per emit"),
        elapsed / f64::from(threads * per_thread),
    );
    println!(
        "  admitted {sent}, drained {drained}, store records {} (deduped by message)",
        store.len()
    );
    report(
        "host drain + ingest, per drained event",
        ingest_ns as f64 / drained.max(1) as f64,
    );

    // Direct host logging (no ring), as `Cx::log` does.
    let mut store = Store::default();
    report(
        "store.push (host thread, dedup hit)",
        per_call(n, |_| {
            black_box(store.push(prebuilt.clone(), Meta::default()).count);
        }),
    );
}
