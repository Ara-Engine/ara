pub mod ecs;
pub mod model;

use ecs::{BenchWorld, ChurnCommand};
use glam::Vec4;

/// Rust owns every buffer; managed code receives a synchronous borrowed view.
pub struct Columns {
    p: Vec<Vec4>,
    v: Vec<Vec4>,
    q: Vec<Vec4>,
    old_p: Vec<Vec4>,
    old_v: Vec<Vec4>,
    grid: model::Grid,
    churn: Option<BenchWorld>,
}

/// C layout mirrored by Managed.HostView; pointers remain valid until s8_free.
#[repr(C)]
pub struct HostView {
    pub p: *mut Vec4,
    pub v: *mut Vec4,
    pub q: *mut Vec4,
    pub old_p: *mut Vec4,
    pub old_v: *mut Vec4,
    pub offsets: *mut i32,
    pub ids: *mut i32,
    pub cursor: *mut i32,
    pub commands: *mut ChurnCommand,
    pub count: i32,
}

/// Allocate native columns. This trusted spike ABI requires count > 0.
#[unsafe(no_mangle)]
pub extern "C" fn s8_alloc(count: i32, churn: bool) -> *mut Columns {
    let n = count as usize;
    let mut data = Columns {
        p: Vec::with_capacity(n),
        v: Vec::with_capacity(n),
        q: Vec::with_capacity(n),
        old_p: vec![Vec4::ZERO; n],
        old_v: vec![Vec4::ZERO; n],
        grid: model::Grid::new(n),
        churn: churn.then(|| BenchWorld::new(n)),
    };
    for id in 0..n {
        let (p, v, q) = model::initial(id as u32);
        data.p.push(p);
        data.v.push(v);
        data.q.push(q);
    }
    Box::into_raw(Box::new(data))
}

/// # Safety
/// `data` is a live exclusive s8_alloc handle. The caller does not free it mid-call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn s8_view(data: *mut Columns) -> HostView {
    // SAFETY: the caller holds the unique live allocation by the ABI contract.
    let data = unsafe { &mut *data };
    HostView {
        p: data.p.as_mut_ptr(),
        v: data.v.as_mut_ptr(),
        q: data.q.as_mut_ptr(),
        old_p: data.old_p.as_mut_ptr(),
        old_v: data.old_v.as_mut_ptr(),
        offsets: data.grid.offsets.as_mut_ptr(),
        ids: data.grid.ids.as_mut_ptr(),
        cursor: data.grid.cursor.as_mut_ptr(),
        commands: data
            .churn
            .as_mut()
            .map_or(std::ptr::null_mut(), |w| w.commands.as_mut_ptr()),
        count: data.p.len() as i32,
    }
}

/// Apply C#'s batched typed command buffer to the Rust ECS.
/// # Safety
/// All workers must be joined before the live exclusive handle is used here.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn s8_playback(data: *mut Columns) {
    // SAFETY: unique live handle, with no outstanding managed worker accesses.
    if let Some(world) = unsafe { &mut *data }.churn.as_mut() {
        world.playback();
    }
}

/// Copy churn output to the borrowed columns for untimed validation.
/// # Safety
/// The live handle is exclusively borrowed and all workers have completed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn s8_collect(data: *mut Columns) {
    // SAFETY: caller promises exclusive access to the still-live allocation.
    let data = unsafe { &mut *data };
    if let Some(world) = data.churn.as_mut() {
        for (i, r) in world.records().iter().enumerate() {
            data.p[i] = Vec4::from_slice(&r[..4]);
            data.v[i] = Vec4::from_slice(&r[4..8]);
            data.q[i] = Vec4::from_slice(&r[8..]);
        }
    }
}

/// # Safety
/// Handle came from s8_alloc, is freed exactly once, and has no live borrowed views.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn s8_free(data: *mut Columns) {
    // SAFETY: ownership is returned exactly once, after managed workers join.
    drop(unsafe { Box::from_raw(data) });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_covers_all_ids_and_ecs_matches_reference() {
        let mut world = BenchWorld::new(257);
        let mut p: Vec<_> = (0..257).map(|i| model::initial(i).0).collect();
        let mut v: Vec<_> = (0..257).map(|i| model::initial(i).1).collect();
        let mut grid = model::Grid::new(257);
        grid.build(&p);
        let mut ids = grid.ids.to_vec();
        ids.sort_unstable();
        assert_eq!(ids, (0..257).collect::<Vec<_>>());
        for _ in 0..10 {
            grid.build(&p);
            let next: Vec<_> = (0..257).map(|i| grid.boid(i, &p, &v)).collect();
            for (i, (np, nv)) in next.into_iter().enumerate() {
                p[i] = np;
                v[i] = nv;
            }
            world.step("w2", false, 0);
        }
        for (i, r) in world.records().iter().enumerate() {
            assert_eq!(&r[..4], p[i].as_ref());
            assert_eq!(&r[4..8], v[i].as_ref());
        }
    }

    #[test]
    fn churn_keeps_count_and_replaces_each_ring_slot() {
        let mut world = BenchWorld::new(100_000);
        for frame in 0..12 {
            world.step("w4", false, frame);
        }
        assert_eq!(
            world
                .world
                .query::<&ecs::StableId>()
                .iter(&world.world)
                .count(),
            100_000
        );
        let records = world.records();
        assert_eq!(&records[0][..4], model::initial(200_000).0.as_ref());
        assert_eq!(&records[10_000][..4], model::initial(210_000).0.as_ref());
        assert_eq!(&records[20_000][..4], model::initial(120_000).0.as_ref());
    }
}
