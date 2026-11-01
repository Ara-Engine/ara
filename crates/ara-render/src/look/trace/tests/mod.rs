//! `look::trace` tests: identities and tombstones, TLAS commits, the side-car cache,
//! and (in `gpu`) `trace()` on the GPU against an f64 brute-force world reference.
use super::*;
use ara_bvh::{Blas, Mesh};
use glam::{Affine3A, Quat, Vec3};
use std::sync::Arc;

mod bench;
mod gpu;

/// Deterministic xorshift generator.
pub(super) struct Rng(u64);

impl Rng {
    pub(super) fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }
    pub(super) fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub(super) fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * ((self.next() >> 40) as f32 / (1u64 << 24) as f32)
    }
    pub(super) fn dir(&mut self) -> Vec3 {
        loop {
            let v = Vec3::new(
                self.range(-1., 1.),
                self.range(-1., 1.),
                self.range(-1., 1.),
            );
            if (0.01..=1.0).contains(&v.length_squared()) {
                return v.normalize();
            }
        }
    }
}

/// An owned test mesh: positions, indices, submesh starts and per-vertex UVs.
#[derive(Clone)]
pub(super) struct TestMesh {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub submeshes: Vec<u32>,
}

impl TestMesh {
    pub(super) fn mesh(&self) -> Mesh<'_> {
        Mesh {
            positions: &self.positions,
            indices: &self.indices,
            submeshes: &self.submeshes,
        }
    }
    fn quad(&mut self, corners: [Vec3; 4], uv: [[f32; 2]; 4]) {
        let base = self.positions.len() as u32;
        self.positions.extend(corners.map(|c| c.to_array()));
        self.uvs.extend(uv);
        self.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    /// The trace mesh, with corner UVs (set 1 mirrors set 0) when `uvs`.
    pub(super) fn trace(&self, uvs: bool) -> TraceMesh {
        let rows = uvs.then(|| {
            Arc::new(
                self.indices
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|t| {
                        let [a, b, c] = t.map(|i| self.uvs[i as usize]);
                        [
                            a[0], a[1], b[0], b[1], c[0], c[1], a[0], a[1], b[0], b[1], c[0], c[1],
                        ]
                    })
                    .collect(),
            )
        });
        TraceMesh {
            blas: Arc::new(Blas::build(&self.mesh()).expect("test mesh builds")),
            uvs: rows,
        }
    }
}

fn empty() -> TestMesh {
    TestMesh {
        positions: Vec::new(),
        uvs: Vec::new(),
        indices: Vec::new(),
        submeshes: Vec::new(),
    }
}

/// A 40 m floor facing +y.
pub(super) fn floor() -> TestMesh {
    let mut m = empty();
    let c = |x: f32, z: f32| Vec3::new(x, 0., z);
    m.quad(
        [c(-20., -20.), c(-20., 20.), c(20., 20.), c(20., -20.)],
        [[0., 0.], [0., 1.], [1., 1.], [1., 0.]],
    );
    m
}

/// A unit cube centred on the origin, faces outward.
pub(super) fn cube() -> TestMesh {
    let mut m = empty();
    let p = |x: bool, y: bool, z: bool| {
        Vec3::new(
            if x { 0.5 } else { -0.5 },
            if y { 0.5 } else { -0.5 },
            if z { 0.5 } else { -0.5 },
        )
    };
    let uv = [[0., 0.], [0., 1.], [1., 1.], [1., 0.]];
    let (f, t) = (false, true);
    m.quad([p(f, f, f), p(f, t, f), p(t, t, f), p(t, f, f)], uv);
    m.quad([p(f, f, t), p(t, f, t), p(t, t, t), p(f, t, t)], uv);
    m.quad([p(f, f, f), p(t, f, f), p(t, f, t), p(f, f, t)], uv);
    m.quad([p(f, t, f), p(f, t, t), p(t, t, t), p(t, t, f)], uv);
    m.quad([p(f, f, f), p(f, f, t), p(f, t, t), p(f, t, f)], uv);
    m.quad([p(t, f, f), p(t, t, f), p(t, t, t), p(t, f, t)], uv);
    m
}

/// A unit UV sphere (its pole triangles are degenerate).
pub(super) fn sphere() -> TestMesh {
    let mut m = empty();
    let (rings, segments) = (12u32, 24u32);
    for r in 0..=rings {
        let theta = std::f32::consts::PI * r as f32 / rings as f32;
        for s in 0..segments {
            let phi = std::f32::consts::TAU * s as f32 / segments as f32;
            m.positions.push([
                theta.sin() * phi.cos(),
                theta.cos(),
                theta.sin() * phi.sin(),
            ]);
            m.uvs
                .push([s as f32 / segments as f32, r as f32 / rings as f32]);
        }
    }
    for r in 0..rings {
        for s in 0..segments {
            let a = r * segments + s;
            let b = r * segments + (s + 1) % segments;
            m.indices
                .extend([a, b, a + segments, b, b + segments, a + segments]);
        }
    }
    m
}

/// A double card: submesh 0 a 2 m quad in the xy plane with UVs 0..2 (an
/// alpha-tested grating), submesh 1 an opaque quad 0.5 m behind it.
pub(super) fn card() -> TestMesh {
    let mut m = empty();
    let c = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
    m.quad(
        [c(-1., 0., 0.), c(1., 0., 0.), c(1., 2., 0.), c(-1., 2., 0.)],
        [[0., 0.], [2., 0.], [2., 2.], [0., 2.]],
    );
    m.submeshes = vec![0, 2];
    m.quad(
        [
            c(-0.5, 0.2, -0.5),
            c(0.5, 0.2, -0.5),
            c(0.5, 1.2, -0.5),
            c(-0.5, 1.2, -0.5),
        ],
        [[0., 0.], [1., 0.], [1., 1.], [0., 1.]],
    );
    m
}

fn instance(mesh: &TraceMesh, world: Affine3A, user: u32) -> TraceInstance {
    TraceInstance {
        mesh: mesh.blas.hash,
        world,
        mask: 1,
        materials: vec![NO_MATERIAL; mesh.blas.stats.submeshes as usize],
        user,
    }
}

#[test]
fn identities_survive_moves_and_die_with_tombstones() {
    let mut scene = TraceScene::default();
    let (cube, sphere) = (cube().trace(false), sphere().trace(false));
    let cube_key = scene.add_mesh(cube.clone());
    assert_eq!(
        scene.add_mesh(cube.clone()),
        cube_key,
        "one BLAS per mesh hash"
    );
    scene.add_mesh(sphere.clone());
    let a = scene
        .set_instance(7, instance(&cube, Affine3A::IDENTITY, 0))
        .expect("insert");
    let b = scene
        .set_instance(
            9,
            instance(&sphere, Affine3A::from_translation(Vec3::X * 3.), 1),
        )
        .expect("insert");
    assert_eq!(scene.commit(), Commit::Rebuilt);
    assert!(scene.set_transform(7, Affine3A::from_translation(Vec3::Y)));
    assert_eq!(scene.commit(), Commit::Refit);
    assert_eq!(scene.instance_key(7), Some(a), "moves keep the key");
    let p = scene.primitive(a, 3).expect("cube triangle 3");
    assert_eq!((p.mesh, p.submesh, p.triangle), (cube.blas.hash, 0, 3));
    assert_eq!(scene.primitive(a, 12), None, "past the mesh");
    // Mesh replacement: the old key dies, the id lives on under a new generation.
    let epoch = scene.epoch();
    let a2 = scene
        .set_instance(7, instance(&sphere, Affine3A::IDENTITY, 0))
        .expect("replace");
    assert_eq!(a2.slot, a.slot);
    assert_ne!(a2.generation, a.generation);
    assert!(!scene.is_live(a) && scene.is_live(a2));
    assert_eq!(scene.primitive(a, 3), None);
    let dead = scene.tombstones_since(epoch);
    assert_eq!(dead.len(), 1);
    assert_eq!(
        (dead[0].id, dead[0].key, dead[0].mesh, dead[0].death),
        (
            7,
            a,
            cube.blas.hash,
            Death::MeshReplaced {
                mesh: sphere.blas.hash
            }
        )
    );
    // Removal, then slot reuse with a fresh generation.
    assert!(scene.remove_instance(9));
    assert!(!scene.remove_instance(9));
    assert_eq!(scene.tombstones_since(epoch).len(), 2);
    assert_eq!(scene.tombstones_since(epoch)[1].death, Death::Removed);
    let c = scene
        .set_instance(11, instance(&cube, Affine3A::IDENTITY, 2))
        .expect("reuse");
    assert_eq!(c.slot, b.slot, "the free slot is reused");
    assert!(c.generation > b.generation);
    assert!(!scene.is_live(b));
    scene.forget_tombstones(scene.epoch());
    assert!(scene.tombstones_since(0).is_empty());
    assert_eq!(scene.commit(), Commit::Rebuilt);
    // Unused meshes free their BLAS slot; the next mesh gets a new generation.
    scene.remove_instance(11);
    scene.remove_instance(7);
    scene.remove_unused_meshes();
    assert_eq!(scene.blas_key(cube.blas.hash), None);
    let again = scene.add_mesh(cube.clone());
    assert_eq!(again.slot, cube_key.slot);
    assert!(again.generation > cube_key.generation);
    assert!(
        scene
            .set_instance(1, instance(&floor().trace(false), Affine3A::IDENTITY, 0))
            .is_err()
    );
}

#[test]
fn refits_degrade_into_rebuilds() {
    let mut scene = TraceScene::default();
    let cube = cube().trace(false);
    scene.add_mesh(cube.clone());
    let mut rng = Rng::new(3);
    for id in 0..64 {
        let t = Vec3::new(rng.range(-10., 10.), 0., rng.range(-10., 10.));
        scene
            .set_instance(id, instance(&cube, Affine3A::from_translation(t), 0))
            .expect("insert");
    }
    assert_eq!(scene.commit(), Commit::Rebuilt);
    assert_eq!(scene.commit(), Commit::Unchanged);
    scene.set_transform(3, Affine3A::from_translation(Vec3::new(1., 1., 1.)));
    assert_eq!(scene.commit(), Commit::Refit);
    for id in 0..64 {
        let t = Vec3::new(
            rng.range(-900., 900.),
            rng.range(-900., 900.),
            rng.range(-900., 900.),
        );
        scene.set_transform(id, Affine3A::from_translation(t));
    }
    assert_eq!(scene.commit(), Commit::Rebuilt, "scattered refits rebuild");
}

#[test]
fn blas_side_cars_build_once_and_survive_corruption() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/trace-cache")
        .join(format!("{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cache = BlasCache::new(&dir);
    let m = sphere();
    let (built, source) = cache.load_or_build(&m.mesh()).expect("build");
    assert_eq!(source, Source::Built);
    let (cached, source) = cache.load_or_build(&m.mesh()).expect("load");
    assert_eq!(source, Source::Cache);
    assert_eq!(cached, built);
    let path = cache.path(&m.mesh());
    let mut bytes = std::fs::read(&path).expect("side-car written");
    let last = bytes.len() - 40;
    bytes[last] ^= 0xff;
    std::fs::write(&path, &bytes).expect("corrupt");
    let (rebuilt, source) = cache.load_or_build(&m.mesh()).expect("rebuild");
    assert_eq!((source, &rebuilt), (Source::Built, &built));
    assert_eq!(
        cache.load_or_build(&m.mesh()).expect("reload").1,
        Source::Cache
    );
    let _ = std::fs::remove_dir_all(&dir);
}
