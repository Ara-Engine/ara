use ara_meshopt_sys::Prepared;
use meshopt::{SimplifyOptions, SimplifyVertex, VertexDataAdapter};
use sha2::{Digest, Sha256};

struct Fixture {
    positions: Vec<[f32; 3]>,
    indices: Vec<u32>,
    values: Vec<f32>,
    locks: Vec<u8>,
}
const WEIGHTS: [f32; 15] = [
    0.05, 0.05, 0.05, 0.01, 0.01, 0.01, 0.01, 0.01, 0.01, 0.01, 0.01, 0.01, 0.01, 0.01, 0.01,
];
fn grid(n: usize, locked: bool) -> Fixture {
    let mut f = Fixture {
        positions: vec![],
        indices: vec![],
        values: vec![],
        locks: vec![],
    };
    for y in 0..=n {
        for x in 0..=n {
            f.positions.push([
                x as f32 * 0.1,
                y as f32 * 0.1,
                ((x * 17 + y * 31) % 19) as f32 * 0.003,
            ]);
            f.values
                .extend((0..15).map(|c| ((x * (c + 1) + y * 7 + c) % 23) as f32 / 23.0));
            f.locks.push(if locked {
                1
            } else if x == n / 2 && y % 3 == 0 {
                2
            } else {
                0
            });
        }
    }
    for y in 0..n {
        for x in 0..n {
            let a = (y * (n + 1) + x) as u32;
            let b = a + (n + 1) as u32;
            f.indices.extend([a, a + 1, b, b, a + 1, b + 1]);
        }
    }
    f
}
fn verify(f: &Fixture) {
    let mut prepared = Prepared::new(&f.indices, &f.positions, &f.values, &WEIGHTS, &f.locks)
        .expect("valid fixture");
    let adapter = VertexDataAdapter::new(bytemuck::cast_slice(&f.positions), 12, 0)
        .expect("packed positions");
    let locks: Vec<_> = f
        .locks
        .iter()
        .map(|&v| SimplifyVertex::from_bits_retain(v))
        .collect();
    let total = f.indices.len();
    for error in [0.1, 0.01, 0.0] {
        for target in [
            total / 6 * 3,
            total / 12 * 3,
            0,
            total,
            total / 24 * 3,
            total / 6 * 3,
        ] {
            let mut expected_error = 0.0;
            let expected = meshopt::simplify_with_attributes_and_locks(
                &f.indices,
                &adapter,
                &f.values,
                &WEIGHTS,
                60,
                &locks,
                target,
                error,
                SimplifyOptions::LockBorder
                    | SimplifyOptions::Permissive
                    | SimplifyOptions::RegularizeLight,
                Some(&mut expected_error),
            );
            let (actual, actual_error) = prepared.simplify(target, error).expect("reset solve");
            assert_eq!(actual, expected, "target {target}, error {error}");
            assert_eq!(
                actual_error.to_bits(),
                expected_error.to_bits(),
                "error bits at target {target}"
            );
        }
    }
}
#[test]
fn resets_equal_upstream_across_targets_errors_locks_and_seams() {
    verify(&grid(24, false));
    verify(&grid(8, true)); // zero eligible collapses
    let mut seams = grid(12, false);
    let count = seams.positions.len();
    seams.positions.extend(seams.positions.clone());
    seams
        .indices
        .extend(seams.indices.clone().iter().map(|v| v + count as u32));
    seams
        .values
        .extend(seams.values.clone().iter().map(|v| 1.0 - v));
    seams.locks.extend(seams.locks.clone());
    verify(&seams);
    let mut unused = grid(12, false);
    unused.positions.extend([[9.0, 0.0, 0.0], [0.0, -7.0, 0.0]]);
    unused.values.extend([0.5; 30]);
    unused.locks.extend([0, 0]);
    verify(&unused);
}
#[test]
fn context_owns_inputs_and_independent_workers_do_not_share_state() {
    std::thread::scope(|scope| {
        for n in [9, 11, 13, 15] {
            scope.spawn(move || verify(&grid(n, false)));
        }
    });
    let mut p = {
        let f = grid(8, false);
        Prepared::new(&f.indices, &f.positions, &f.values, &WEIGHTS, &f.locks)
            .expect("owned context")
    };
    assert!(p.simplify(0, 0.1).is_ok());
    assert!(p.simplify(usize::MAX, 0.1).is_err());
    assert!(p.simplify(0, f32::NAN).is_err());
    assert!(p.simplify(0, 0.1).is_ok());
}
#[test]
fn invalid_inputs_are_rejected_before_native_access() {
    let f = grid(4, false);
    assert!(Prepared::new(&[u32::MAX; 3], &f.positions, &f.values, &WEIGHTS, &f.locks).is_err());
    assert!(Prepared::new(&f.indices, &f.positions, &f.values[..1], &WEIGHTS, &f.locks).is_err());
    assert!(Prepared::new(&f.indices, &f.positions, &f.values, &[0.0; 15], &f.locks).is_err());
    assert!(Prepared::new(&f.indices, &f.positions, &f.values, &WEIGHTS, &[255; 25]).is_err());
}
#[test]
fn vendored_sources_remain_exactly_the_pinned_upstream() {
    for (source, expected) in [
        (
            include_bytes!("../vendor/simplifier.cpp").as_slice(),
            "719303b6970398631e30724e8eebc6cd2e7e08b67fe8ab7cbf7eecafdaa08616".to_owned(),
        ),
        (
            include_bytes!("../vendor/meshoptimizer.h").as_slice(),
            "e3688ef553f603391747895a5c221ef1528574cc7f8173782417a96cb531f61b".into(),
        ),
    ] {
        assert_eq!(format!("{:x}", Sha256::digest(source)), expected);
    }
}
