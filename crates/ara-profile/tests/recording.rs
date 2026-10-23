#![allow(clippy::unwrap_used)]
use ara_profile::*;
use tracing_subscriber::prelude::*;

#[global_allocator]
static ALLOCATOR: CountingAllocator<std::alloc::System> = CountingAllocator(std::alloc::System);

struct Attached;
impl Drop for Attached { fn drop(&mut self) { ThreadRegistration::detach(); } }
fn profiler(history: usize, ring_capacity: usize) -> (Profiler, Attached) {
    let mut p = Profiler::new(Config { history, ring_capacity, spans_per_frame: 32, ..Config::default() }).unwrap();
    p.register_thread("main").attach().unwrap(); p.start(); (p, Attached)
}
fn end(p: &mut Profiler) { p.end_frame(Counters::default(), Memory::default()).unwrap(); }

#[test]
fn nested_self_time_parentage_and_stop() {
    let (mut p, _attached) = profiler(2, 32); p.begin_frame().unwrap();
    { let _parent = scope("parent"); let _child = scope("child"); }
    end(&mut p);
    let frame = &p.profile().frames[0];
    assert_eq!(frame.spans.len(), 2);
    let child = frame.spans[0]; let parent = frame.spans[1];
    assert_eq!(child.parent, parent.id); assert_eq!(parent.parent, 0);
    assert_eq!(parent.self_ns + child.duration_ns, parent.duration_ns);
    assert_eq!(frame.flame_children(0, parent.id).count(), 1);
    p.stop(); p.begin_frame().unwrap(); { let _scope = scope("disabled"); } end(&mut p);
    assert!(p.profile().frames[1].spans.is_empty());
}

#[test]
fn delayed_workers_and_gpu_keep_origin_frame_and_report_eviction() {
    let (mut p, _attached) = profiler(2, 32);
    let worker = p.register_thread("asset-worker");
    let first = p.begin_frame().unwrap(); end(&mut p);
    p.begin_frame().unwrap();
    std::thread::spawn(move || {
        worker.attach().unwrap(); let _attached = Attached;
        first.in_scope(|| { let _scope = scope("jobs.asset.import"); }).unwrap();
    }).join().unwrap();
    assert!(p.gpu_pass(first.id(), "shadow", "graphics", 1000));
    assert!(p.job_activity(first.id(), "compile.server", 123, 5000, true));
    end(&mut p);
    assert_eq!(p.profile().frames[0].spans.len(), 1);
    assert!(p.profile().frames[1].spans.is_empty());
    assert_eq!(p.profile().frames[0].gpu[0].duration_ns, 1000);
    assert_eq!(p.profile().frames[0].jobs[0].worker, 123);
    p.begin_frame().unwrap(); end(&mut p);
    assert!(!p.gpu_pass(first.id(), "late", "compute", 100));
    assert_eq!(p.profile().orphaned_results, 1);
}

#[test]
fn ring_overflow_is_bounded_and_explicit() {
    let (mut p, _attached) = profiler(1, 2); p.begin_frame().unwrap();
    for _ in 0..10 { let _scope = scope("overflow"); }
    end(&mut p);
    assert_eq!(p.profile().frames[0].spans.len(), 2);
    assert_eq!(p.profile().frames[0].dropped, 8);
}

#[test]
fn tracing_uses_dynamic_system_names_and_excludes_suspension() {
    let (mut p, _attached) = profiler(1, 32);
    let subscriber = tracing_subscriber::registry().with(ProfileLayer);
    tracing::subscriber::with_default(subscriber, || {
        p.begin_frame().unwrap();
        let span = tracing::info_span!("system", name = "movement");
        { let _enter = span.enter(); }
        { let _enter = span.enter(); }
        end(&mut p);
    });
    assert_eq!(p.profile().frames[0].spans.len(), 2);
    for span in &p.profile().frames[0].spans { assert_eq!(p.profile().labels[span.label as usize], "movement"); }
}

#[test]
fn tokens_refuse_cross_capture_attachment() {
    let (mut p, attached) = profiler(1, 32); let token = p.begin_frame().unwrap(); end(&mut p);
    drop(attached);
    let (_other, _attached) = profiler(1, 32);
    assert!(token.in_scope(|| panic!("must not run on the wrong capture")).is_err());
}

#[cfg(all(feature = "allocation", not(feature = "tracy")))]
#[test]
fn warmed_recording_and_frame_collection_allocate_nothing() {
    let (mut p, _attached) = profiler(2, 32);
    for _ in 0..2 {
        let f = p.begin_frame().unwrap(); { let _s = scope("warm"); }
        p.gpu_pass(f.id(), "shadow", "graphics", 1); p.job_activity(f.id(), "compile", 1, 1, true); end(&mut p);
    }
    let before = AllocationSample::thread_bytes();
    for _ in 0..100 {
        let f = p.begin_frame().unwrap(); { let _s = scope("warm"); }
        p.gpu_pass(f.id(), "shadow", "graphics", 1); p.job_activity(f.id(), "compile", 1, 1, true); end(&mut p);
    }
    assert_eq!(AllocationSample::thread_bytes(), before);
}

#[cfg(feature = "allocation")]
#[test]
fn allocation_bytes_are_attributed_to_nested_systems() {
    let (mut p, _attached) = profiler(1, 32); p.begin_frame().unwrap();
    { let _parent = scope("parent");
        let outer = std::hint::black_box(vec![0u8; 100]);
        { let _child = scope("child"); std::hint::black_box(vec![0u8; 200]); }
        std::hint::black_box(outer);
    }
    end(&mut p); let spans = &p.profile().frames[0].spans;
    assert_eq!(spans[0].allocated_bytes, 200);
    assert_eq!(spans[1].allocated_bytes, 300);
    assert_eq!(spans[1].self_allocated_bytes, 100);
}

#[test]
fn concurrent_ring_publication_loses_only_counted_records() {
    let (mut p, _attached) = profiler(1, 64);
    let worker = p.register_thread("producer"); let token = p.begin_frame().unwrap();
    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let flag = std::sync::Arc::clone(&running);
    let thread = std::thread::spawn(move || {
        worker.attach().unwrap(); let _attached = Attached;
        token.in_scope(|| { for _ in 0..10000 { let _s = scope("worker"); } }).unwrap();
        flag.store(false, std::sync::atomic::Ordering::Release);
    });
    while running.load(std::sync::atomic::Ordering::Acquire) { p.collect(); std::thread::yield_now(); }
    thread.join().unwrap(); end(&mut p);
    let frame = &p.profile().frames[0]; assert_eq!(frame.spans.len() as u64 + frame.dropped, 10000);
}
