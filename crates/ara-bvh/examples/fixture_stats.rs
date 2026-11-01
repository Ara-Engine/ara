//! Builds the BLAS of an R4-1 lab fixture and checks CPU traversal against the lab's
//! independent references: `cargo run --release -p ara-bvh --example fixture_stats --
//! <r4-1 dir> [set]` (set `a` by default: `vertices_<set>.bin` is three f32x4 per
//! triangle, `rays_<set>.bin` two f32x4 per ray, `refs_<set>.bin` (t, prim) for
//! 4,096 sampled rays). Prints build time, sizes and mismatches, then a tree quality
//! proxy: single-threaded CPU traversal time over every ray for this build and for
//! tinybvh's own tree of the set (`nodes_<set>.bin`, `tris_<set>.bin`).
use ara_bvh::{Blas, Mesh, Ray, RayFlags};
use glam::Vec3;
use std::time::Instant;

fn floats(path: &std::path::Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(args.next().expect("r4-1 directory"));
    let set = args.next().unwrap_or_else(|| "a".into());
    let v = floats(&dir.join(format!("vertices_{set}.bin")));
    let positions: Vec<[f32; 3]> = v
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| [c[0], c[1], c[2]])
        .collect();
    let indices: Vec<u32> = (0..positions.len() as u32).collect();
    let mesh = Mesh {
        positions: &positions,
        indices: &indices,
        submeshes: &[],
    };
    let start = Instant::now();
    let blas = Blas::build(&mesh).expect("fixture builds");
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    println!(
        "set {set}: {} triangles, build {ms:.1} ms, {} nodes ({} B), {} refs ({} B), max stack {}, degenerate {}",
        blas.stats.triangles,
        blas.stats.nodes,
        blas.nodes.len() * 16,
        blas.stats.references,
        blas.triangles.len() * 16,
        blas.stats.max_stack,
        blas.stats.degenerate
    );
    let rays = floats(&dir.join(format!("rays_{set}.bin")));
    let refs = std::fs::read(dir.join(format!("refs_{set}.bin"))).expect("refs");
    let count = rays.len() / 8;
    let (mut mismatch, mut dist, mut max_dt) = (0, 0, 0f32);
    for k in 0..4096u64 {
        let i = (k * count as u64 / 4096
            + u64::from((k as u32).wrapping_mul(2654435761)) % (count as u64 / 4096))
            as usize;
        let r = &rays[i * 8..i * 8 + 8];
        let ray = Ray {
            origin: Vec3::new(r[0], r[1], r[2]),
            direction: Vec3::new(r[4], r[5], r[6]),
            t_min: 0.0,
            t_max: 1e30,
        };
        let want_t = f32::from_le_bytes(
            refs[k as usize * 8..k as usize * 8 + 4]
                .try_into()
                .expect("t"),
        );
        let got = blas.intersect(&ray, RayFlags::NONE);
        let want_hit = want_t < 1e29;
        if got.is_some() != want_hit {
            mismatch += 1;
            continue;
        }
        if let Some(h) = got {
            let dt = (h.t - want_t).abs();
            max_dt = max_dt.max(dt);
            if dt > 1e-4f32.max(want_t * 1e-5) {
                dist += 1;
            }
        }
    }
    println!(
        "4096 reference rays: {mismatch} hit/miss mismatches, {dist} distance failures, max |dt| {max_dt}"
    );
    // Tree quality proxy: CPU traversal time over every ray, ours vs tinybvh's tree.
    let all: Vec<Ray> = rays
        .as_chunks::<8>()
        .0
        .iter()
        .map(|r| Ray {
            origin: Vec3::new(r[0], r[1], r[2]),
            direction: Vec3::new(r[4], r[5], r[6]),
            t_min: 0.0,
            t_max: 1e30,
        })
        .collect();
    let time = |b: &Blas| {
        let start = Instant::now();
        let hits = all
            .iter()
            .filter(|r| b.intersect(r, RayFlags::NONE).is_some())
            .count();
        (start.elapsed().as_secs_f64() * 1000.0, hits)
    };
    let (ours, hits) = time(&blas);
    let words = |f: &str| -> Option<Vec<[u32; 4]>> {
        let b = std::fs::read(dir.join(f)).ok()?;
        Some(
            b.as_chunks::<16>()
                .0
                .iter()
                .map(|c| std::array::from_fn(|k| u32::from_le_bytes([c[k * 4], c[k * 4 + 1], c[k * 4 + 2], c[k * 4 + 3]])))
                .collect(),
        )
    };
    match (words(&format!("nodes_{set}.bin")), words(&format!("tris_{set}.bin"))) {
        (Some(nodes), Some(triangles)) => {
            let lab = Blas {
                nodes,
                triangles,
                ..blas.clone()
            };
            let (theirs, lab_hits) = time(&lab);
            println!(
                "CPU traversal of {} rays: ours {ours:.1} ms ({hits} hits), tinybvh tree {theirs:.1} ms ({lab_hits} hits), ratio {:.3}",
                all.len(),
                ours / theirs
            );
        }
        _ => println!("CPU traversal of {} rays: {ours:.1} ms ({hits} hits)", all.len()),
    }
}
