//! The threaded TLAS: every instance a ray's boxes reach is visited, skips are
//! consistent, refits keep boxes conservative and report structural changes.
use super::Rng;
use crate::Aabb;
use crate::tlas::{END, LEAF, Tlas, TlasNode};
use glam::Vec3;

fn boxes(seed: u64, n: usize) -> Vec<Aabb> {
    let mut rng = Rng::new(seed);
    (0..n)
        .map(|i| {
            if i % 17 == 5 {
                return Aabb::EMPTY;
            }
            let c = Vec3::new(
                rng.range(-100., 100.),
                rng.range(-5., 20.),
                rng.range(-100., 100.),
            );
            let s = Vec3::new(rng.range(0.1, 8.), rng.range(0.1, 8.), rng.range(0.1, 8.));
            Aabb {
                min: c - s,
                max: c + s,
            }
        })
        .chain([Aabb {
            // The world: one big box.
            min: Vec3::splat(-120.),
            max: Vec3::splat(120.),
        }])
        .collect()
}

fn hit(n: &TlasNode, o: Vec3, rd: Vec3) -> bool {
    let lo = (Vec3::from(n.min) - o) * rd;
    let hi = (Vec3::from(n.max) - o) * rd;
    let near = lo.min(hi).max_element().max(0.0);
    let far = lo.max(hi).min_element();
    near <= far
}

/// Instances whose leaf boxes a threaded walk reaches.
fn walk(tlas: &Tlas, o: Vec3, d: Vec3) -> Vec<u32> {
    let rd = d.recip();
    let mut out = Vec::new();
    let mut node = if tlas.nodes.is_empty() { END } else { 0 };
    while node != END {
        let n = tlas.nodes[node as usize];
        if hit(&n, o, rd) {
            if n.first & LEAF != 0 {
                out.push(n.first & !LEAF);
            } else {
                node = n.first;
                continue;
            }
        }
        node = n.skip;
    }
    out.sort_unstable();
    out
}

fn check_boxes(tlas: &Tlas, boxes: &[Aabb]) {
    for (i, n) in tlas.nodes.iter().enumerate() {
        if n.first & LEAF != 0 {
            let b = boxes[(n.first & !LEAF) as usize];
            assert_eq!((Vec3::from(n.min), Vec3::from(n.max)), (b.min, b.max));
        } else {
            let (l, r) = (
                tlas.nodes[i + 1],
                tlas.nodes[tlas.nodes[i + 1].skip as usize],
            );
            for a in 0..3 {
                assert_eq!(n.min[a], l.min[a].min(r.min[a]));
                assert_eq!(n.max[a], l.max[a].max(r.max[a]));
            }
            assert_eq!(r.skip, n.skip, "second child skips where its parent does");
        }
    }
}

#[test]
fn threaded_walk_finds_every_instance_a_ray_reaches() {
    let boxes = boxes(1, 500);
    let tlas = Tlas::build(&boxes);
    assert_eq!(
        tlas.nodes.len(),
        2 * (boxes.iter().filter(|b| b.is_valid()).count()) - 1
    );
    assert_eq!(tlas.nodes[0].skip, END);
    check_boxes(&tlas, &boxes);
    let mut rng = Rng::new(2);
    for _ in 0..500 {
        let o = Vec3::new(
            rng.range(-150., 150.),
            rng.range(-10., 30.),
            rng.range(-150., 150.),
        );
        let d = rng.dir();
        let rd = d.recip();
        let mut want: Vec<u32> = (0..boxes.len() as u32)
            .filter(|&i| {
                let b = boxes[i as usize];
                b.is_valid()
                    && hit(
                        &TlasNode {
                            min: b.min.into(),
                            first: 0,
                            max: b.max.into(),
                            skip: 0,
                        },
                        o,
                        rd,
                    )
            })
            .collect();
        want.sort_unstable();
        assert_eq!(walk(&tlas, o, d), want);
    }
    assert_eq!(Tlas::build(&boxes).nodes, tlas.nodes, "deterministic");
}

#[test]
fn refits_track_moves_and_reject_structural_changes() {
    let mut boxes = boxes(3, 200);
    let mut tlas = Tlas::build(&boxes);
    let built = tlas.cost();
    let mut rng = Rng::new(4);
    for b in boxes.iter_mut().filter(|b| b.is_valid()).step_by(3) {
        let shift = Vec3::new(rng.range(-3., 3.), 0., rng.range(-3., 3.));
        b.min += shift;
        b.max += shift;
    }
    assert!(tlas.refit(&boxes));
    check_boxes(&tlas, &boxes);
    assert!(
        !tlas.needs_rebuild(),
        "small moves keep the tree ({} vs {built})",
        tlas.cost()
    );
    // Scatter everything far apart: the refit tree degrades.
    for b in boxes.iter_mut().filter(|b| b.is_valid()) {
        let shift = Vec3::new(
            rng.range(-900., 900.),
            rng.range(-900., 900.),
            rng.range(-900., 900.),
        );
        b.min += shift;
        b.max += shift;
    }
    assert!(tlas.refit(&boxes));
    check_boxes(&tlas, &boxes);
    assert!(tlas.needs_rebuild());
    let before = tlas.nodes.clone();
    boxes[5] = Aabb {
        min: Vec3::ZERO,
        max: Vec3::ONE,
    };
    assert!(
        !tlas.refit(&boxes),
        "an instance gaining a box is structural"
    );
    assert_eq!(tlas.nodes, before);
    assert!(Tlas::build(&[Aabb::EMPTY]).nodes.is_empty());
    let one = Tlas::build(&[Aabb {
        min: Vec3::ZERO,
        max: Vec3::ONE,
    }]);
    assert_eq!(one.nodes.len(), 1);
    assert_eq!((one.nodes[0].first, one.nodes[0].skip), (LEAF, END));
    assert_eq!(one.leaf_node(0), Some(0));
}
