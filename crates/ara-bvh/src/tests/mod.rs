//! Builder correctness: every triangle reachable through conservative boxes, CPU
//! traversal against brute force, determinism, side-car round trips, the stack bound
//! and the TLAS.
use crate::cwbvh::NODE_WORDS;
use crate::sbvh::Settings;
use crate::traverse::brute_force;
use crate::*;
use glam::{DVec3, Vec3};

mod tlas;

/// Deterministic xorshift generator for fixtures and rays.
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }
    pub(crate) fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// Uniform in [0, 1).
    pub(crate) fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    pub(crate) fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
    pub(crate) fn dir(&mut self) -> Vec3 {
        loop {
            let v = Vec3::new(
                self.range(-1., 1.),
                self.range(-1., 1.),
                self.range(-1., 1.),
            );
            if v.length_squared() > 0.01 && v.length_squared() <= 1.0 {
                return v.normalize();
            }
        }
    }
}

/// An owned mesh fixture.
pub(crate) struct Owned {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub submeshes: Vec<u32>,
}

impl Owned {
    pub(crate) fn mesh(&self) -> Mesh<'_> {
        Mesh {
            positions: &self.positions,
            indices: &self.indices,
            submeshes: &self.submeshes,
        }
    }
    fn quad(&mut self, corners: [Vec3; 4]) {
        let base = self.positions.len() as u32;
        self.positions.extend(corners.map(|c| c.to_array()));
        self.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    fn cuboid(&mut self, lo: Vec3, hi: Vec3) {
        let c = |x: bool, y: bool, z: bool| {
            Vec3::new(
                if x { hi.x } else { lo.x },
                if y { hi.y } else { lo.y },
                if z { hi.z } else { lo.z },
            )
        };
        let (f, t) = (false, true);
        self.quad([c(f, f, f), c(f, t, f), c(t, t, f), c(t, f, f)]);
        self.quad([c(f, f, t), c(t, f, t), c(t, t, t), c(f, t, t)]);
        self.quad([c(f, f, f), c(t, f, f), c(t, f, t), c(f, f, t)]);
        self.quad([c(f, t, f), c(f, t, t), c(t, t, t), c(t, t, f)]);
        self.quad([c(f, f, f), c(f, f, t), c(f, t, t), c(f, t, f)]);
        self.quad([c(t, f, f), c(t, t, f), c(t, t, t), c(t, f, t)]);
    }
}

/// A town: an axis-aligned floor, a few hundred boxes, long thin triangles that
/// straddle everything (spatial splits), a tessellated sphere, duplicates and
/// degenerate triangles; three submeshes.
pub(crate) fn town(seed: u64) -> Owned {
    let mut rng = Rng::new(seed);
    let mut m = Owned {
        positions: Vec::new(),
        indices: Vec::new(),
        submeshes: vec![0],
    };
    m.quad([
        Vec3::new(-50., 0., -50.),
        Vec3::new(-50., 0., 50.),
        Vec3::new(50., 0., 50.),
        Vec3::new(50., 0., -50.),
    ]);
    for _ in 0..300 {
        let c = Vec3::new(rng.range(-45., 45.), 0., rng.range(-45., 45.));
        let s = Vec3::new(rng.range(0.2, 4.), rng.range(0.5, 12.), rng.range(0.2, 4.));
        m.cuboid(c - Vec3::new(s.x, 0., s.z), c + s);
    }
    m.submeshes.push((m.indices.len() / 3) as u32);
    for _ in 0..40 {
        let a = Vec3::new(
            rng.range(-50., 50.),
            rng.range(0., 15.),
            rng.range(-50., 50.),
        );
        let b = Vec3::new(
            rng.range(-50., 50.),
            rng.range(0., 15.),
            rng.range(-50., 50.),
        );
        let base = m.positions.len() as u32;
        m.positions
            .extend([a, b, b + Vec3::Y * 0.05].map(|v| v.to_array()));
        m.indices.extend([base, base + 1, base + 2]);
    }
    m.submeshes.push((m.indices.len() / 3) as u32);
    let (rings, segments) = (16, 32);
    let centre = Vec3::new(10., 6., -10.);
    let base = m.positions.len() as u32;
    for r in 0..=rings {
        let theta = std::f32::consts::PI * r as f32 / rings as f32;
        for s in 0..segments {
            let phi = std::f32::consts::TAU * s as f32 / segments as f32;
            let p = centre
                + 3. * Vec3::new(
                    theta.sin() * phi.cos(),
                    theta.cos(),
                    theta.sin() * phi.sin(),
                );
            m.positions.push(p.to_array());
        }
    }
    for r in 0..rings {
        for s in 0..segments {
            let a = base + r * segments + s;
            let b = base + r * segments + (s + 1) % segments;
            let (c, d) = (a + segments, b + segments);
            m.indices.extend([a, c, b, b, c, d]);
        }
    }
    // A duplicate of the first floor triangle and two degenerate ones.
    m.indices.extend([0, 1, 2, 0, 0, 1, 4, 5, 4]);
    m
}

fn blas(m: &Owned) -> Blas {
    Blas::build(&m.mesh()).expect("fixture builds")
}

/// Exact (f64) dequantized child boxes of a node: Some((lo, hi)) per used slot.
fn child_boxes(blas: &Blas, node: usize) -> [Option<(DVec3, DVec3)>; 8] {
    let n = &blas.nodes[node * NODE_WORDS..(node + 1) * NODE_WORDS];
    let lo = DVec3::new(
        f64::from(f32::from_bits(n[0][0])),
        f64::from(f32::from_bits(n[0][1])),
        f64::from(f32::from_bits(n[0][2])),
    );
    let e = n[0][3];
    let step = DVec3::new(
        f64::from((e & 255) as i32 - 127).exp2(),
        f64::from(((e >> 8) & 255) as i32 - 127).exp2(),
        f64::from(((e >> 16) & 255) as i32 - 127).exp2(),
    );
    let byte = |word: u32, slot: usize| f64::from((word >> ((slot % 4) * 8)) & 255);
    let meta = |slot: usize| (n[1][2 + slot / 4] >> ((slot % 4) * 8)) & 255;
    std::array::from_fn(|slot| {
        if meta(slot) == 0 {
            return None;
        }
        let h = slot / 4;
        let q = |lo_word: u32, hi_word: u32| (byte(lo_word, slot), byte(hi_word, slot));
        let (lx, hx) = q(n[2][h], n[3][2 + h]);
        let (ly, hy) = q(n[2][2 + h], n[4][h]);
        let (lz, hz) = q(n[3][h], n[4][2 + h]);
        Some((
            lo + DVec3::new(lx, ly, lz) * step,
            lo + DVec3::new(hx, hy, hz) * step,
        ))
    })
}

/// Source triangles of every leaf whose box path contains `p`.
fn reachable(blas: &Blas, p: DVec3) -> Vec<u32> {
    let mut out = Vec::new();
    let mut stack = vec![0usize];
    while let Some(node) = stack.pop() {
        let n = &blas.nodes[node * NODE_WORDS..(node + 1) * NODE_WORDS];
        let imask = n[0][3] >> 24;
        for (slot, b) in child_boxes(blas, node).into_iter().enumerate() {
            let Some((lo, hi)) = b else { continue };
            if p.cmplt(lo).any() || p.cmpgt(hi).any() {
                continue;
            }
            let meta = (n[1][2 + slot / 4] >> ((slot % 4) * 8)) & 255;
            if imask & (1 << slot) != 0 {
                let rel = (imask & ((1 << slot) - 1)).count_ones();
                stack.push((n[1][0] + rel) as usize);
            } else {
                let (offset, count) = (meta & 31, (meta >> 5).count_ones());
                for k in 0..count {
                    let addr = (n[1][1] + (offset + k) * 4) as usize;
                    out.push(blas.triangles[addr + 3][3]);
                }
            }
        }
    }
    out
}

#[test]
fn every_triangle_is_reachable_through_conservative_boxes() {
    let m = town(1);
    let blas = blas(&m);
    let mesh = m.mesh();
    let degenerate = (0..mesh.triangle_count() as u32)
        .filter(|&t| {
            let c = mesh.corners(t);
            (c[1] - c[0]).cross(c[2] - c[0]) == Vec3::ZERO
        })
        .count();
    assert_eq!(blas.stats.degenerate as usize, degenerate);
    assert!(degenerate >= 2, "the fixture has degenerate triangles");
    assert!(
        blas.stats.references > blas.stats.triangles - 2,
        "spatial splits add references"
    );
    for t in 0..mesh.triangle_count() as u32 {
        let c = mesh.corners(t).map(|p| p.as_dvec3());
        if (c[1] - c[0]).cross(c[2] - c[0]) == DVec3::ZERO {
            continue;
        }
        // Corners, edge midpoints and interior dyadic points: exact in f64.
        let weights = [
            (0., 0.),
            (1., 0.),
            (0., 1.),
            (0.5, 0.),
            (0., 0.5),
            (0.5, 0.5),
            (0.25, 0.25),
            (0.125, 0.75),
            (0.75, 0.125),
        ];
        for (u, v) in weights {
            let p = c[0] + (c[1] - c[0]) * u + (c[2] - c[0]) * v;
            assert!(
                reachable(&blas, p).contains(&t),
                "triangle {t} not reachable at ({u}, {v})"
            );
        }
    }
}

#[test]
fn traversal_matches_brute_force() {
    let m = town(2);
    let mesh = m.mesh();
    let blas = blas(&m);
    let mut rng = Rng::new(7);
    let (mut hits, mut max_dt) = (0, 0f32);
    for i in 0..3000 {
        let origin = Vec3::new(
            rng.range(-60., 60.),
            rng.range(0.1, 30.),
            rng.range(-60., 60.),
        );
        let direction = rng.dir() * if i % 3 == 0 { 3.0 } else { 1.0 };
        let flags = [
            RayFlags::NONE,
            RayFlags::CULL_BACK_FACES,
            RayFlags::CULL_FRONT_FACES,
        ][i % 3];
        let ray = Ray {
            origin,
            direction,
            t_min: if i % 5 == 0 { 2.0 } else { 0.0 },
            t_max: if i % 7 == 0 { 20.0 } else { f32::INFINITY },
        };
        let got = blas.intersect(&ray, flags);
        let want = brute_force(&mesh, &ray, flags, |_| true);
        match (got, want) {
            (Some(g), Some(w)) => {
                hits += 1;
                let dt = (g.t - w.t).abs();
                max_dt = max_dt.max(dt);
                assert!(
                    dt <= 1e-4_f32.max(w.t * 1e-5),
                    "ray {i}: t {} vs {}",
                    g.t,
                    w.t
                );
                if g.primitive == w.primitive {
                    assert_eq!(g.front, w.front, "ray {i}: orientation");
                    assert!((g.barycentrics[0] - w.barycentrics[0]).abs() < 1e-3);
                    assert!((g.barycentrics[1] - w.barycentrics[1]).abs() < 1e-3);
                    assert_eq!(g.submesh, mesh.submesh_of(w.primitive));
                }
            }
            (None, None) => {}
            (g, w) => panic!("ray {i}: hit {g:?} vs reference {w:?}"),
        }
        // Any-hit agrees on visibility.
        let any = blas.intersect(&ray, flags | RayFlags::ACCEPT_FIRST_HIT);
        assert_eq!(any.is_some(), want.is_some(), "ray {i}: any-hit");
    }
    assert!(hits > 500, "fixture should be hit often ({hits})");
    eprintln!("{hits} hits, max |dt| {max_dt}");
}

#[test]
fn filters_reject_candidates_like_an_alpha_test() {
    let m = town(3);
    let (mesh, blas) = (m.mesh(), blas(&m));
    let mut rng = Rng::new(11);
    // Reject the whole first submesh's even triangles.
    let keep = |h: &Hit| !(h.submesh == 0 && h.primitive.is_multiple_of(2));
    for i in 0..1000 {
        let ray = Ray {
            origin: Vec3::new(
                rng.range(-60., 60.),
                rng.range(0.1, 30.),
                rng.range(-60., 60.),
            ),
            direction: rng.dir(),
            t_min: 0.0,
            t_max: f32::INFINITY,
        };
        let got = blas.intersect_filtered(&ray, RayFlags::NONE, keep);
        let want = brute_force(&mesh, &ray, RayFlags::NONE, keep);
        assert_eq!(got.is_some(), want.is_some(), "ray {i}");
        if let (Some(g), Some(w)) = (got, want) {
            assert!(keep(&g), "ray {i}: a rejected candidate was committed");
            assert!((g.t - w.t).abs() <= 1e-4_f32.max(w.t * 1e-5), "ray {i}");
        }
    }
}

#[test]
fn builds_are_deterministic_and_round_trip_through_sidecars() {
    let m = town(4);
    let a = blas(&m);
    let copy = Owned {
        positions: m.positions.clone(),
        indices: m.indices.clone(),
        submeshes: m.submeshes.clone(),
    };
    let b = blas(&copy);
    assert_eq!(a, b);
    let bytes = sidecar::encode(&a);
    assert_eq!(
        bytes,
        sidecar::encode(&b),
        "same mesh bytes, same BVH bytes"
    );
    assert_eq!(sidecar::decode(&bytes, m.mesh().hash()), Ok(a.clone()));
    let mut other = m.positions.clone();
    other[0][0] += 1.0;
    let moved = Mesh {
        positions: &other,
        ..m.mesh()
    };
    assert_ne!(moved.hash(), m.mesh().hash());
    assert!(
        sidecar::decode(&bytes, moved.hash()).is_err(),
        "another mesh's side-car"
    );
    let mut corrupt = bytes.clone();
    corrupt[HEADER_PROBE] ^= 1;
    assert!(sidecar::decode(&corrupt, m.mesh().hash()).is_err());
    assert!(sidecar::decode(&bytes[..bytes.len() - 1], m.mesh().hash()).is_err());
    assert!(sidecar::file_name(a.hash).ends_with(&format!("-v{BUILDER_VERSION}.cwbvh")));
}
/// A byte inside the node payload.
const HEADER_PROBE: usize = 150;

#[test]
fn deep_unbalanced_meshes_stay_within_the_stack() {
    // Triangles on a geometric series pull SAH splits one triangle at a time.
    let mut m = Owned {
        positions: Vec::new(),
        indices: Vec::new(),
        submeshes: Vec::new(),
    };
    for i in 0..3000u32 {
        let x = 1.0005f32.powi(i as i32 * 8);
        let base = m.positions.len() as u32;
        m.positions
            .extend([[x, 0., 0.], [x * 1.0001, 1., 0.], [x, 0., 1.]]);
        m.indices.extend([base, base + 1, base + 2]);
    }
    let mesh = m.mesh();
    let blas = blas(&m);
    assert!(
        blas.stats.max_stack <= TRACE_STACK,
        "stack {}",
        blas.stats.max_stack
    );
    let mut rng = Rng::new(5);
    for i in 0..300 {
        let target = mesh.corners(rng.next() as u32 % 3000);
        let p = (target[0] + target[1] + target[2]) / 3.0;
        let ray = Ray {
            origin: p - Vec3::new(0.3, 0.2, 2.0),
            direction: Vec3::new(0.3, 0.2, 2.0),
            t_min: 0.0,
            t_max: f32::INFINITY,
        };
        let got = blas.intersect(&ray, RayFlags::NONE).map(|h| h.t);
        let want = brute_force(&mesh, &ray, RayFlags::NONE, |_| true).map(|h| h.t);
        assert_eq!(got.is_some(), want.is_some(), "ray {i}");
    }
}

#[test]
fn settings_without_spatial_splits_keep_one_reference_per_triangle() {
    let m = town(6);
    let settings = Settings {
        spatial_splits: false,
        ..Settings::default()
    };
    let b = Blas::build_with(&m.mesh(), &settings).expect("builds");
    assert_eq!(b.stats.references, b.stats.triangles - b.stats.degenerate);
}

#[test]
fn empty_and_tiny_meshes_build() {
    let positions = [[0f32, 0., 0.], [1., 0., 0.], [0., 1., 0.]];
    let empty = Mesh {
        positions: &positions,
        indices: &[],
        submeshes: &[],
    };
    let b = Blas::build(&empty).expect("empty builds");
    assert!(b.triangles.is_empty() && !b.bounds.is_valid());
    let ray = Ray {
        // The triangle's normal is +z: this ray meets its front.
        origin: Vec3::new(0.2, 0.2, 1.),
        direction: -Vec3::Z,
        t_min: 0.,
        t_max: f32::INFINITY,
    };
    assert_eq!(b.intersect(&ray, RayFlags::NONE), None);
    let one = Mesh {
        indices: &[0, 1, 2],
        ..empty
    };
    let b = Blas::build(&one).expect("one triangle builds");
    let hit = b
        .intersect(&ray, RayFlags::NONE)
        .expect("hits the triangle");
    assert!((hit.t - 1.0).abs() < 1e-6 && hit.front);
    assert!(
        Mesh {
            indices: &[0, 1, 3],
            ..empty
        }
        .validate()
        .is_err()
    );
    assert!(
        Mesh {
            indices: &[0, 1],
            ..empty
        }
        .validate()
        .is_err()
    );
    assert!(
        Mesh {
            indices: &[0, 1, 2],
            submeshes: &[1],
            ..empty
        }
        .validate()
        .is_err()
    );
}
