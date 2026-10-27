#![allow(clippy::unwrap_used)]
#[path = "../examples/support/mod.rs"]
mod support;
use ara_gpu::GpuDevice;
use ara_render::capture::{CaptureRequest, ViewMode, compare};
use ara_vfx::{CpuSystem, Effect};
use ara_vfx_gpu::{GpuSystem, View};
use std::path::Path;

#[test]
fn gpu_replay_goldens_and_cpu_count_parity() {
    if std::env::var("ARA_VFX_GPU_TESTS").as_deref() != Ok("1") {
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let captures = root.join("../../target/ara-vfx-gpu/goldens");
    std::fs::create_dir_all(&captures).unwrap();
    let mut gpu = GpuDevice::new(&[], root.join(".cache/tests")).unwrap();
    for (effect, tick) in ara_vfx::samples::all() {
        let mut system = GpuSystem::new(
            &mut gpu,
            &effect,
            &root.join("cooked").join(&effect.name),
            [960, 540],
            ara_vfx_gpu::LINEAR_HDR,
        )
        .unwrap();
        let request = CaptureRequest {
            camera: View::default().camera,
            size: [960, 540],
            mode: ViewMode::Lit,
            deterministic: true,
            exposure: 1.,
            settle_frames: 1,
            tick: u64::from(tick),
            seed: u64::from(effect.seed),
            marks: Some(1),
        };
        let a = system.capture(&mut gpu, &request).unwrap();
        let stats = system.stats(&gpu).unwrap();
        let b = system.capture(&mut gpu, &request).unwrap();
        assert_eq!(a.image.as_raw(), b.image.as_raw(), "{} replay", effect.name);
        assert_eq!(a.legend.len(), 1, "{} has visible particles", effect.name);
        let cpu = CpuSystem::at_tick(&effect, tick, None).unwrap();
        let gpu_particles = system.read_particles(&mut gpu).unwrap();
        for (gpu, cpu) in gpu_particles.iter().zip(cpu.emitters()) {
            for &slot in cpu.alive.iter().take(64) {
                let a = &gpu[slot as usize];
                let b = cpu.particle(slot).unwrap();
                assert_eq!(a.seed, b.seed, "{} seed", effect.name);
                assert_eq!(a.birth, b.birth, "{} birth", effect.name);
                assert_eq!(a.life, b.life, "{} lifetime", effect.name);
                assert!(
                    a.position.abs_diff_eq(b.position, 0.005),
                    "{} position: {:?} vs {:?}",
                    effect.name,
                    a.position,
                    b.position
                );
                assert!(
                    a.velocity.abs_diff_eq(b.velocity, 0.005),
                    "{} velocity",
                    effect.name
                );
                assert!((a.size - b.size).abs() < 0.0001, "{} size", effect.name);
                assert!(a.color.abs_diff_eq(b.color, 0.001), "{} color", effect.name);
            }
        }
        assert_eq!(
            stats.particles_alive,
            cpu.stats().alive,
            "{} CPU/GPU counts",
            effect.name
        );
        for e in &stats.emitters {
            assert!(e.alive + e.dead <= effect.quality.particles);
        }
        if effect.name == "rain" {
            assert!(
                stats.emitters[1].alive > 0,
                "GPU collision events produce splashes"
            );
        }
        system.retire(&mut gpu).unwrap();
        let ticks = support::sequence_ticks(&effect.name);
        let mut times = ticks.to_vec();
        times.push(tick);
        let mut frames = support::capture_frames(&mut gpu, &effect, &times, [960, 540]).unwrap();
        let (replay, _) = frames.pop().unwrap();
        let mid = ticks.iter().position(|t| *t == tick).unwrap();
        let strip = support::sequence_strip(&frames);
        let (a, stats) = frames.swap_remove(mid);
        assert_eq!(
            a.image.as_raw(),
            replay.image.as_raw(),
            "{} fidelity replay",
            effect.name
        );
        assert_eq!(
            a.legend.len(),
            1,
            "{} visible after scene depth",
            effect.name
        );
        assert!(
            a.metadata
                .timings
                .iter()
                .any(|stage| stage.pass == "Feature.Bloom.Down.1")
        );
        let golden = root.join("goldens").join(format!("{}.png", effect.name));
        let strip_golden = root
            .join("goldens")
            .join(format!("{}.sequence.png", effect.name));
        if std::env::var("ARA_UPDATE_GOLDENS").as_deref() == Ok("1") {
            std::fs::create_dir_all(golden.parent().unwrap()).unwrap();
            a.image.save(&golden).unwrap();
            strip.save(&strip_golden).unwrap();
        }
        a.write(&captures.join(format!("{}.png", effect.name)))
            .unwrap();
        strip
            .save(captures.join(format!("{}.sequence.png", effect.name)))
            .unwrap();
        let reference = image::open(&golden).unwrap().to_rgba8();
        let difference = compare(&reference, &a.image, 0.005).unwrap();
        difference
            .diff
            .save(captures.join(format!("{}.diff.png", effect.name)))
            .unwrap();
        assert!(
            difference.passed,
            "{} golden comparison, 1-SSIM: {}",
            effect.name, difference.score
        );
        let reference = image::open(&strip_golden).unwrap().to_rgba8();
        assert!(
            compare(&reference, &strip, 0.005).unwrap().passed,
            "{} sequence golden",
            effect.name
        );
        std::fs::write(
            captures.join(format!("{}.stats.json", effect.name)),
            ara_json::to_canonical_string(&ara_json::to_value(&stats).unwrap()),
        )
        .unwrap();
    }
    gpu.assert_clean().unwrap();
}
#[test]
fn all_generated_sources_use_material_evaluation_and_fixed_seed_inputs() {
    for (effect, _) in ara_vfx::samples::all() {
        let source = ara_vfx_gpu::cook_source(&effect, None).unwrap();
        assert!(source.contains("AraSurface evaluateMaterial"));
        assert!(source.contains("p.seed=hash("));
        assert!(!source.contains("InterlockedAdd"));
    }
    let invalid = Effect::parse("{}");
    assert!(invalid.is_err());
}
