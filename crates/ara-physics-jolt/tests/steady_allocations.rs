//! One test in its own process keeps unrelated test allocations out of the count.
mod support;
use ara_physics::*;
use ara_physics_jolt::{JoltBackend, JoltWorld};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use support::*;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static REALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
struct CountingAllocator;
// SAFETY: forwards the original layout, size and pointer to the system allocator;
// counters neither allocate nor affect allocation ownership.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the caller supplies GlobalAlloc's valid layout.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller supplies its live pointer and original layout.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        REALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: forwards the caller's live pointer, layout and nonzero size.
        unsafe { System.realloc(ptr, layout, size) }
    }
}
fn allocations() -> (usize, usize) {
    (
        ALLOCATIONS.load(Ordering::Relaxed),
        REALLOCATIONS.load(Ordering::Relaxed),
    )
}

#[test]
fn unsubscribed_steady_steps_and_ecs_reuse_rust_storage() {
    let mut w = JoltWorld::new(PhysicsConfig {
        threads: 4,
        max_bodies: 128,
        max_pairs: 2048,
        max_contacts: 2048,
        scratch_mb: 8,
        ..PhysicsConfig::game_default()
    })
    .expect("allocation fixture");
    ground(&mut w, 20.0);
    let ids = grid(&mut w, 64, Vec3::Y * 2.0, 8, 7);
    let mut columns = BodyColumns::default();
    for _ in 0..80 {
        w.step().expect("warm step");
        w.backend_mut()
            .read_body_columns(&ids, &mut columns)
            .expect("warm columns");
    }
    let before = allocations();
    for _ in 0..60 {
        w.step().expect("steady step");
        w.backend_mut()
            .read_body_columns(&ids, &mut columns)
            .expect("steady columns");
    }
    assert_eq!(
        allocations(),
        before,
        "step/columns must reuse Rust storage"
    );

    let mut ecs = bevy_ecs::world::World::new();
    let mut schedule = bevy_ecs::schedule::Schedule::default();
    register::<JoltBackend>(&mut ecs, &mut schedule, PhysicsConfig::game_default())
        .expect("ECS allocation fixture");
    for value in 1..65 {
        ecs.spawn((
            PhysicsId { value },
            PhysicsPose::default(),
            Velocity::default(),
            RigidBody::default(),
            Collider::default(),
        ));
    }
    for _ in 0..80 {
        tick_ecs::<JoltBackend>(&mut ecs).expect("warm ECS");
        ecs.clear_trackers();
    }
    let before = allocations();
    for _ in 0..60 {
        tick_ecs::<JoltBackend>(&mut ecs).expect("steady ECS");
        ecs.clear_trackers();
    }
    assert_eq!(allocations(), before, "ECS must reuse Rust storage");
}
