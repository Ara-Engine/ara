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
fn compare(bench: &mut Bench, settings: Settings) -> Result<(f64, usize, f64, u32)> {
    let timing = bench.run_v2(true, Blockers::On, settings)?;
    assert!(timing.stats.clean(), "{:?}", timing.stats);
    let test = bench.read(Readback::V2)?;
    let counters = bench.read(Readback::Counters)?;
    // Without counters the shade pass splits into its lean and visibility PSOs.
    bench.run_v2(false, Blockers::On, settings)?;
    let split = bench.read(Readback::V2)?;
    let split_worst = (0..(SIZE[0] * SIZE[1]) as usize)
        .map(|i| (lum(&split[i * 4..]) - lum(&test[i * 4..])).abs() / lum(&test[i * 4..]).max(1e-6))
        .fold(0., f64::max);
    assert!(
        split_worst <= 1e-5,
        "split shading differs from the combined pass by {split_worst}"
    );
    bench.run_exact(false, Blockers::On, settings)?;
    let control = bench.read(Readback::Exact)?;
    let n = (SIZE[0] * SIZE[1]) as usize;
    let mean = (0..n).map(|i| lum(&control[i * 4..])).sum::<f64>() / n as f64;
    let floor = mean * 1e-3;
    let errors: Vec<f64> = (0..n)
        .map(|i| {
            (lum(&test[i * 4..]) - lum(&control[i * 4..])).abs() / lum(&control[i * 4..]).max(floor)
        })
        .collect();
    let worst = errors.iter().copied().fold(0., f64::max);
    for (i, e) in errors
        .iter()
        .enumerate()
        .filter(|(_, e)| **e > 1e-4)
        .take(8)
    {
        eprintln!(
            "  pixel {} {}: v2 {:.6} control {:.6} ({e:.4})",
            i % SIZE[0] as usize,
            i / SIZE[0] as usize,
            lum(&test[i * 4..]),
            lum(&control[i * 4..])
        );
    }
    let differing = errors.iter().filter(|&&e| e > 1e-4).count();
    // Rays traced plus 4096 per pair the sample masks resolved (LIGHTS_MASKED).
    let traced = (0..n).filter(|&i| counters[i * 4 + 1] > 0.).count() as f64 / n as f64;
    Ok((worst, differing, traced, timing.stats.partial))
}

#[test]
fn v2_matches_the_exact_control_through_moving_shadows() -> Result<()> {
    for range in [1., 8.] {
        let mut bench = Bench::new(Layout::new(&case(range, 2.4)), SIZE, 1 << 16)?;
        for frame in [0, 7, 31] {
            bench.pose(frame)?;
            let rays = Settings {
                masks: false,
                ..Settings::default()
            };
            let (worst_rays, differing_rays, _, _) = compare(&mut bench, rays)?;
            eprintln!(
                "range {range} frame {frame} rays: {differing_rays} differing, worst {worst_rays}"
            );
            let (worst, differing, traced, partial) = compare(&mut bench, Settings::default())?;
            // The sample masks and trace()'s rays agree except where a segment grazes a
            // triangle edge within trace()'s edge slack (one sample of one light).
            assert!(
                differing * 20_000 <= (SIZE[0] * SIZE[1]) as usize,
                "range {range} frame {frame}: {differing} pixels differ from the control (worst {worst})"
            );
            eprintln!("range {range} frame {frame}: {differing} differing pixels, worst {worst}");
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

/// Möller-Trumbore in f64: the segment o + t d, t in (0, t_max), against a triangle.
fn segment_hits(o: glam::DVec3, d: glam::DVec3, t_max: f64, tri: [glam::DVec3; 3]) -> bool {
    let (e1, e2) = (tri[1] - tri[0], tri[2] - tri[0]);
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-300 {
        return false;
    }
    let s = o - tri[0];
    let u = s.dot(p) / det;
    let q = s.cross(e1);
    let v = d.dot(q) / det;
    let t = e2.dot(q) / det;
    u >= 0. && v >= 0. && u + v <= 1. && t > 0. && t < t_max
}

/// Arbiter for disagreements between the sample masks and per-sample rays: the first
/// disagreeing pixel's light, its 16 samples re-tested in f64 against every cube.
#[test]
#[ignore = "diagnostic"]
fn masks_against_f64_brute_force() -> Result<()> {
    let layout = Layout::new(&case(1., 2.4));
    let frame0 = layout.frame(0);
    let floor = layout.floor();
    let mut bench = Bench::new(layout, SIZE, 1 << 16)?;
    bench.pose(0)?;
    let settings = Settings {
        debug_masks: true,
        ..Settings::default()
    };
    bench.run_v2(true, Blockers::On, settings)?;
    let debug = bench.read(Readback::Counters)?;
    let depth = bench.read(Readback::Depth)?;
    let normal = bench.read(Readback::Normal)?;
    let (w, h) = (SIZE[0] as usize, SIZE[1] as usize);
    let inverse = bench.vp.inverse().as_dmat4();
    let (positions, indices) = scene::cube();
    let cubes: Vec<glam::DAffine3> = std::iter::once(floor)
        .chain(frame0.boxes.iter().copied())
        .map(|b| glam::DAffine3::from_mat4(glam::Mat4::from(b.world).as_dmat4()))
        .collect();
    let mut shown = 0;
    for i in (0..w * h).filter(|&i| debug[i * 4 + 3] > 0.5) {
        let (x, y) = (i % w, i / w);
        let light = frame0.lights[debug[i * 4] as usize];
        let (mask, rays) = (debug[i * 4 + 1] as u32, debug[i * 4 + 2] as u32);
        // The pixel's origin as the shader rebuilds it.
        let ndc = glam::DVec4::new(
            (x as f64 + 0.5) / w as f64 * 2. - 1.,
            1. - (y as f64 + 0.5) / h as f64 * 2.,
            f64::from(depth[i]),
            1.,
        );
        let world = inverse * ndc;
        let world = world.truncate() / world.w;
        let oct =
            glam::DVec2::new(f64::from(normal[i * 4]), f64::from(normal[i * 4 + 1])) * 2. - 1.;
        let mut n = glam::DVec3::new(oct.x, oct.y, 1. - oct.x.abs() - oct.y.abs());
        let t = (-n.z).max(0.);
        n.x += if n.x >= 0. { -t } else { t };
        n.y += if n.y >= 0. { -t } else { t };
        let n = n.normalize();
        let s = Settings::default();
        let footprint = f64::from(scene::footprint(&bench.layout.camera, SIZE[1]));
        let bias = f64::from(s.bias_min).max(
            f64::from(s.bias_footprints) * footprint * (world - bench.camera.as_dvec3()).length(),
        );
        let o = world + n * bias;
        // lightsFrame for a sphere.
        let c = glam::DVec3::new(
            light.position_range[0].into(),
            light.position_range[1].into(),
            light.position_range[2].into(),
        );
        let axis = (o - c).normalize();
        let sg = if axis.z >= 0. { 1. } else { -1. };
        let a = -1. / (sg + axis.z);
        let b = axis.x * axis.y * a;
        let u = glam::DVec3::new(1. + sg * axis.x * axis.x * a, sg * b, -sg * axis.x);
        let v = glam::DVec3::new(b, sg + axis.y * axis.y * a, -axis.y);
        let r = f64::from(light.parameters[2]);
        let d = (o - c).length();
        let radius = r * if d > r * 1.001 {
            d / (d * d - r * r).sqrt()
        } else {
            1.
        };
        let mut brute = 0u32;
        let mut hit_by = Vec::new();
        for k in 0..16u32 {
            let rk = ((k as f64 + 0.5) / 16.).sqrt();
            let phi = k as f64 * 2.399_963_23;
            let q = c + (u * phi.cos() + v * phi.sin()) * (radius * rk);
            for (ci, cube) in cubes.iter().enumerate() {
                for tri in indices.as_chunks::<3>().0 {
                    let corners = [0, 1, 2].map(|j| {
                        cube.transform_point3(
                            glam::Vec3::from(positions[tri[j] as usize]).as_dvec3(),
                        )
                    });
                    if segment_hits(o, q - o, 0.9999, corners) {
                        brute |= 1 << k;
                        hit_by.push((k, ci));
                    }
                }
            }
        }
        eprintln!(
            "pixel {x} {y} light {}: masks {mask:016b} rays {rays:016b} f64 {brute:016b} (hits {hit_by:?})",
            debug[i * 4]
        );
        shown += 1;
        if shown >= 6 {
            break;
        }
    }
    bench.destroy()?;
    Ok(())
}

/// Localizes V2-vs-control disagreements of one dynamic frame (`ARA_LIGHTS_PROBE`:
/// case id, frame, pixel x, pixel y) across classification and mask switches.
#[test]
#[ignore = "diagnostic"]
fn dynamic_mismatch_probe() -> Result<()> {
    let spec = std::env::var("ARA_LIGHTS_PROBE")
        .unwrap_or_else(|_| "moving-200-lights,23,1434,922".into());
    let parts: Vec<&str> = spec.split(',').collect();
    let case = scene::cases()
        .into_iter()
        .find(|c| c.id == parts[0])
        .expect("case");
    let (frame, px, py): (u32, usize, usize) =
        (parts[1].parse()?, parts[2].parse()?, parts[3].parse()?);
    let size = [2560u32, 1440];
    let mut bench = Bench::new(Layout::new(&case), size, 1 << 22)?;
    bench.pose(frame)?;
    let n = (size[0] * size[1]) as usize;
    bench.run_exact(false, Blockers::On, Settings::default())?;
    let control = bench.read(Readback::Exact)?;
    let at = (py * size[0] as usize + px) * 4;
    eprintln!("control at pixel: {}", lum(&control[at..]));
    for (name, s) in [
        ("default", Settings::default()),
        (
            "noclassify",
            Settings {
                classify: false,
                ..Settings::default()
            },
        ),
        (
            "rays",
            Settings {
                masks: false,
                ..Settings::default()
            },
        ),
    ] {
        bench.run_v2(false, Blockers::On, s)?;
        let test = bench.read(Readback::V2)?;
        let differing = (0..n)
            .filter(|&i| {
                (lum(&test[i * 4..]) - lum(&control[i * 4..])).abs()
                    > 1e-4 * lum(&control[i * 4..]).max(1e-3)
            })
            .count();
        eprintln!("{name}: {differing} differing; pixel {}", lum(&test[at..]));
    }
    // The exact control with rays instead of masks.
    bench.run_exact(
        false,
        Blockers::On,
        Settings {
            masks: false,
            ..Settings::default()
        },
    )?;
    let rays = bench.read(Readback::Exact)?;
    let differing = (0..n)
        .filter(|&i| {
            (lum(&rays[i * 4..]) - lum(&control[i * 4..])).abs()
                > 1e-4 * lum(&control[i * 4..]).max(1e-3)
        })
        .count();
    eprintln!(
        "ray control vs mask control: {differing} differing; pixel {}",
        lum(&rays[at..])
    );
    bench.destroy()?;
    Ok(())
}

/// Replays the quality loop's call sequence up to one frame and compares V2 with the
/// exact control there (`ARA_LIGHTS_PROBE`: case, frame, x, y; `ARA_LIGHTS_PROBE_OPEN`
/// 0 drops the no-blocker runs).
#[test]
#[ignore = "diagnostic"]
fn sequence_mismatch_probe() -> Result<()> {
    let spec = std::env::var("ARA_LIGHTS_PROBE")
        .unwrap_or_else(|_| "moving-200-lights,23,1434,922".into());
    let open = std::env::var("ARA_LIGHTS_PROBE_OPEN").map_or(true, |v| v != "0");
    let parts: Vec<&str> = spec.split(',').collect();
    let case = scene::cases()
        .into_iter()
        .find(|c| c.id == parts[0])
        .expect("case");
    let (last, px, py): (u32, usize, usize) =
        (parts[1].parse()?, parts[2].parse()?, parts[3].parse()?);
    let size = [2560u32, 1440];
    let mut bench = Bench::new(Layout::new(&case), size, 1 << 22)?;
    let s = Settings::default();
    let at = (py * size[0] as usize + px) * 4;
    for f in 0..=last {
        bench.pose(f)?;
        bench.run_v2(true, Blockers::On, s)?;
        let test = bench.read(Readback::V2)?;
        if std::env::var("ARA_LIGHTS_PROBE_READS").is_ok() {
            bench.read(Readback::Counters)?;
            bench.read(Readback::Material)?;
        }
        bench.run_exact(false, Blockers::On, s)?;
        let control = bench.read(Readback::Exact)?;
        if open {
            bench.run_v2(false, Blockers::Off, s)?;
            if std::env::var("ARA_LIGHTS_PROBE_READS").is_ok() {
                bench.read(Readback::V2)?;
            }
            bench.run_exact(false, Blockers::Off, s)?;
            if std::env::var("ARA_LIGHTS_PROBE_READS").is_ok() {
                bench.read(Readback::Exact)?;
            }
        }
        let n = (size[0] * size[1]) as usize;
        let differing = (0..n)
            .filter(|&i| {
                (lum(&test[i * 4..]) - lum(&control[i * 4..])).abs()
                    > 1e-4 * lum(&control[i * 4..]).max(1e-3)
            })
            .count();
        if differing > 0 || f == last {
            eprintln!(
                "frame {f}: {differing} differing; pixel v2 {} control {}",
                lum(&test[at..]),
                lum(&control[at..])
            );
        }
    }
    bench.destroy()?;
    Ok(())
}

/// Runs V2 and the exact control twice on every frame of a case and counts pixels where
/// a pass disagrees with itself (`ARA_LIGHTS_PROBE`: case, last frame).
#[test]
#[ignore = "diagnostic"]
fn determinism_probe() -> Result<()> {
    let spec = std::env::var("ARA_LIGHTS_PROBE").unwrap_or_else(|_| "moving-200-lights,23".into());
    let parts: Vec<&str> = spec.split(',').collect();
    let case = scene::cases()
        .into_iter()
        .find(|c| c.id == parts[0])
        .expect("case");
    let last: u32 = parts[1].parse()?;
    let size = [2560u32, 1440];
    let entries = size[0].div_ceil(16) * size[1].div_ceil(16) * 96;
    let mut bench = Bench::new(Layout::new(&case), size, entries)?;
    let s = Settings {
        masks: std::env::var("ARA_LIGHTS_PROBE_RAYS").is_err(),
        ..Settings::default()
    };
    let n = (size[0] * size[1]) as usize;
    let count = |a: &[f32], b: &[f32]| {
        (0..n * 4)
            .filter(|&i| a[i].to_bits() != b[i].to_bits())
            .count()
    };
    let paired = std::env::var("ARA_LIGHTS_PROBE_PAIR").is_ok();
    for f in 0..=last {
        bench.pose(f)?;
        let (a, b, c, again, d);
        if paired {
            bench.run_pair(true, Blockers::On, s)?;
            a = bench.read(Readback::V2)?;
            c = bench.read(Readback::Exact)?;
            again = bench.read(Readback::Exact)?;
            bench.run_pair(true, Blockers::On, s)?;
            b = bench.read(Readback::V2)?;
            d = bench.read(Readback::Exact)?;
        } else {
            bench.run_v2(true, Blockers::On, s)?;
            a = bench.read(Readback::V2)?;
            bench.run_v2(true, Blockers::On, s)?;
            b = bench.read(Readback::V2)?;
            bench.run_exact(false, Blockers::On, s)?;
            c = bench.read(Readback::Exact)?;
            again = bench.read(Readback::Exact)?;
            bench.run_exact(false, Blockers::On, s)?;
            d = bench.read(Readback::Exact)?;
        }
        let (v2, exact, cross, reread) = (
            count(&a, &b),
            count(&c, &d),
            count(&a, &c),
            count(&c, &again),
        );
        if v2 + exact + cross + reread > 0 {
            eprintln!(
                "frame {f}: v2 vs itself {v2}, exact vs itself {exact}, v2 vs exact {cross}, re-read {reread} words"
            );
        }
    }
    bench.destroy()?;
    Ok(())
}
