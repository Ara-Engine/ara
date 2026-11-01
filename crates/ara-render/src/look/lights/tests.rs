//! V2 on the GPU against its exact finite-range control: the conservative tile
//! classification never changes a pixel, occluders combine per source sample, and
//! omission stays within its energy bound.
use super::Settings;
use super::bench::scene::{self, Case, Frame, Layout, MASK_BLOCKERS, MASK_FLOOR, Motion, Placed};
use super::bench::{Bench, Blockers, Readback};
use crate::scene::{CameraSpec, Light};
use anyhow::Result;
use glam::{Affine3A, Quat, Vec3};

const SIZE: [u32; 2] = [384, 216];

fn lum(p: &[f32]) -> f64 {
    0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
}

fn case(range: f32, lpp: f32) -> Case {
    Case {
        id: "test".into(),
        motion: Motion::E3,
        lights: 200,
        range,
        target_lpp: lpp,
        event_frame: 8,
    }
}

/// The largest per-pixel |V2 - exact| over the exact luminance (floored at 1e-3 of
/// the exact mean), and the pixels V2 traced rays for.
fn compare(bench: &mut Bench, settings: Settings) -> Result<(f64, f64, u32)> {
    let timing = bench.run_v2(true, Blockers::On, settings)?;
    assert!(timing.stats.clean(), "{:?}", timing.stats);
    let test = bench.read(Readback::V2)?;
    let counters = bench.read(Readback::Counters)?;
    bench.run_exact(false, Blockers::On, settings)?;
    let control = bench.read(Readback::Exact)?;
    let n = (SIZE[0] * SIZE[1]) as usize;
    let mean = (0..n).map(|i| lum(&control[i * 4..])).sum::<f64>() / n as f64;
    let floor = mean * 1e-3;
    let worst = (0..n)
        .map(|i| {
            (lum(&test[i * 4..]) - lum(&control[i * 4..])).abs() / lum(&control[i * 4..]).max(floor)
        })
        .fold(0., f64::max);
    let traced = (0..n).filter(|&i| counters[i * 4 + 1] > 0.).count() as f64 / n as f64;
    Ok((worst, traced, timing.stats.partial))
}

#[test]
fn v2_matches_the_exact_control_through_moving_shadows() -> Result<()> {
    for range in [1., 8.] {
        let mut bench = Bench::new(Layout::new(&case(range, 2.4)), SIZE, 1 << 16)?;
        for frame in [0, 7, 31] {
            bench.pose(frame)?;
            let (worst, traced, partial) = compare(&mut bench, Settings::default())?;
            assert!(
                worst <= 1e-4,
                "range {range} frame {frame}: V2 differs from the control by {worst}"
            );
            assert!(
                partial > 0 && traced > 0. && traced < 0.9,
                "range {range}: {partial} partial pairs, {traced} traced"
            );
        }
        bench.destroy()?;
    }
    Ok(())
}

#[test]
fn occluders_combine_per_source_sample() -> Result<()> {
    // One sphere light 2 m above the origin; two thin blockers at 0.8 and 1.2 m over
    // x < 0 each hide the same half of the source from the floor's centre.
    let light = Light {
        position_range: [0., 2., 0., 6.],
        color_lumens: [1., 1., 1., 1000.],
        direction_outer: [0., -1., 0., -1.],
        parameters: [0., 1., 0.25, 1.],
        ..Light::ZERO
    };
    let blocker = |id: u64, height: f32| Placed {
        id,
        world: Affine3A::from_scale_rotation_translation(
            Vec3::new(2., 0.01, 2.),
            Quat::IDENTITY,
            Vec3::new(-1., height, 0.),
        ),
        material: scene::RED,
        mask: MASK_BLOCKERS,
    };
    let floor = Placed {
        id: 1,
        world: Affine3A::from_scale_rotation_translation(
            Vec3::new(8., 0.2, 8.),
            Quat::IDENTITY,
            Vec3::new(0., -0.1, 0.),
        ),
        material: scene::FLOOR,
        mask: MASK_FLOOR,
    };
    // Look up at the floor's centre from below the blockers' edge (x > 0 side).
    let camera = CameraSpec::LookAt {
        position: Vec3::new(0.6, 0.5, 0.0001),
        target: Vec3::new(0., 0., 0.),
        vertical_fov: 0.3,
    };
    let visibility = |boxes: Vec<Placed>| -> Result<f64> {
        let layout = Layout::custom(
            Frame {
                lights: vec![light],
                boxes,
            },
            floor,
            camera,
        );
        let mut bench = Bench::new(layout, SIZE, 1 << 16)?;
        bench.pose(0)?;
        let settings = Settings {
            samples: 64,
            ..Settings::default()
        };
        bench.run_v2(false, Blockers::On, settings)?;
        let shadowed = bench.read(Readback::V2)?;
        bench.run_v2(false, Blockers::Off, settings)?;
        let open = bench.read(Readback::V2)?;
        bench.run_exact(false, Blockers::On, settings)?;
        let exact = bench.read(Readback::Exact)?;
        bench.destroy()?;
        let centre = ((SIZE[1] / 2) * SIZE[0] + SIZE[0] / 2) as usize * 4;
        let v = lum(&shadowed[centre..]) / lum(&open[centre..]);
        assert!(
            (lum(&exact[centre..]) - lum(&shadowed[centre..])).abs() <= 1e-6 * lum(&open[centre..])
        );
        Ok(v)
    };
    let one = visibility(vec![blocker(10, 0.8)])?;
    let two = visibility(vec![blocker(10, 0.8), blocker(11, 1.2)])?;
    assert!(
        (0.4..=0.6).contains(&one),
        "one blocker over half the source: {one}"
    );
    assert!(
        (two - one).abs() < 1e-6,
        "a second blocker over the same half changed visibility {one} -> {two}"
    );
    Ok(())
}

#[test]
fn omission_stays_within_its_energy_bound() -> Result<()> {
    let mut bench = Bench::new(Layout::new(&case(4., 27.)), SIZE, 1 << 20)?;
    let settings = Settings {
        omission: Some(0.01),
        ..Settings::default()
    };
    let (mut worst, mut omitted) = (0f64, 0f64);
    for frame in [0, 13] {
        bench.pose(frame)?;
        bench.run_v2(true, Blockers::On, settings)?;
        let test = bench.read(Readback::V2)?;
        let counters = bench.read(Readback::Counters)?;
        bench.run_exact(false, Blockers::On, settings)?;
        let control = bench.read(Readback::Exact)?;
        for i in 0..(SIZE[0] * SIZE[1]) as usize {
            let (lt, lc) = (lum(&test[i * 4..]), lum(&control[i * 4..]));
            assert!(
                lt <= lc * (1. + 1e-5) + 1e-12,
                "omission added light at pixel {i}: {lt} > {lc}"
            );
            if lc > 0. {
                worst = worst.max((lc - lt) / lc);
            }
            omitted += f64::from(counters[i * 4 + 3]);
        }
    }
    bench.destroy()?;
    assert!(
        worst <= 0.01 + 1e-5,
        "omitted {worst} of a pixel's luminance"
    );
    eprintln!("omission: worst {worst}, {omitted} pixel-lights omitted");
    Ok(())
}
