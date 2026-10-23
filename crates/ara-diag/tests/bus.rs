//! The process-global bus end to end: emit on several threads, sample spam, link
//! causes, drain into a store. The bus is global, so these tests take a lock.

use std::sync::{Mutex, MutexGuard, PoisonError};

use ara_diag::bus::{self, Event, Meta, Origin};
use ara_diag::store::{Filter, Store};
use ara_diag::{Diagnostic, Subject, codes};

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

fn drain(consumer: &bus::Consumer, store: &mut Store) {
    let mut events: Vec<Event> = Vec::new();
    let lost = consumer.drain(&mut events);
    store.ingest(events);
    store.absorb(&lost);
}

#[test]
fn nothing_is_emitted_without_a_consumer() {
    let _g = serial();
    assert!(!bus::admit(codes::IO));
    assert!(Diagnostic::error(codes::IO, "lost").emit().is_none());
    assert!(ara_diag::emit!(error, codes::IO, "never formatted {}", 1).is_none());
}

#[test]
fn threads_emit_into_one_store_with_origins_ticks_and_causes() {
    let _g = serial();
    let consumer = bus::attach();
    let mut store = Store::default();
    bus::set_tick(42);
    let workers: Vec<_> = (0..4)
        .map(|t| {
            std::thread::spawn(move || {
                let _agent = (t == 0).then(|| bus::scope(Origin::Agent("codex".into())));
                let parent =
                    Diagnostic::error(codes::ASSET_IMPORT_FAILED, format!("import {t} failed"))
                        .subject(Subject::asset(format!("@meshes/m{t}.glb")))
                        .emit();
                Diagnostic::warning(codes::RENDER_NO_MESH, format!("entity {t} has no mesh"))
                    .record(format!("e_ent{t}"))
                    .emit_with(Meta::caused_by(parent))
            })
        })
        .collect();
    for w in workers {
        assert!(w.join().expect("worker").is_some());
    }
    drain(&consumer, &mut store);
    assert_eq!(store.len(), 8);
    let mesh = store.query(&Filter {
        subjects: vec![Subject::entity("e_ent2")],
        ..Filter::default()
    });
    let causes = store.causes(mesh[0]);
    assert_eq!(
        causes[0].diagnostic.message, "import 2 failed",
        "cause crosses the ring"
    );
    assert_eq!(mesh[0].tick, Some(42));
    let by_agent = store.query(&Filter {
        origin: ara_diag::store::OriginFilter::parse("agent:codex"),
        ..Filter::default()
    });
    assert_eq!(by_agent.len(), 2, "the scope marks only the agent's thread");
}

#[test]
fn spam_is_sampled_and_counted_not_lost() {
    let _g = serial();
    let consumer = bus::attach();
    let mut store = Store::default();
    let mut formatted = 0;
    for i in 0..10_000 {
        if bus::admit(codes::FRAME_OVER_BUDGET) {
            formatted += 1;
            bus::push(
                Diagnostic::warning(codes::FRAME_OVER_BUDGET, format!("frame {i} over budget")),
                Meta::default().dedup("frame"),
            );
        }
    }
    assert_eq!(formatted, bus::DEFAULT_BURST, "only the burst is formatted");
    drain(&consumer, &mut store);
    let frames = store.query(&Filter {
        codes: vec![codes::FRAME_OVER_BUDGET],
        ..Filter::default()
    });
    assert_eq!(frames.len(), 1, "one record");
    assert_eq!(frames[0].count, 10_000, "every occurrence is counted");

    // A new window admits a new burst; a tuned code admits less.
    bus::set_burst(codes::GPU_VALIDATION, 2);
    let admitted = (0..10)
        .filter(|_| bus::admit(codes::GPU_VALIDATION))
        .count();
    assert_eq!(admitted, 2);
}

#[test]
fn a_full_ring_drops_and_reports_it() {
    let _g = serial();
    let consumer = bus::attach();
    let mut store = Store::default();
    std::thread::spawn(|| {
        for i in 0..bus::RING_CAPACITY + 10 {
            bus::push(
                Diagnostic::info(codes::REF_FILLED, format!("n{i}")),
                Meta::default(),
            );
        }
    })
    .join()
    .expect("producer");
    drain(&consumer, &mut store);
    let dropped = store.query(&Filter {
        codes: vec![codes::DIAG_DROPPED],
        ..Filter::default()
    });
    assert!(dropped[0].diagnostic.message.starts_with("10 diagnostics"));
}
