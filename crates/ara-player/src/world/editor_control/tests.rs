#![allow(clippy::unwrap_used)]
use super::*;
use crate::input::Navigation;

fn world() -> World {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            if cfg!(windows) {
                PathBuf::from(r"D:\AraBuild\ux-editor-target")
            } else {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target")
            }
        });
    let cache = target.join("ara-player/navigation-test-cache");
    let scene:PlayScene=serde_json::from_value(json!({
        "format":1,"scene":"sc_navtest","seed":901,"tick_hz":60,"assets":{},"cache":cache,"nav_cache":null,"types":[],
        "documents":[{"path":"navigation.scene","kind":"scene","id":"sc_navtest","records":{
            "e_camera":{"name":"Camera","order":"a0","Transform":{"position":[2.,3.,4.]},"Camera":{}}
        }}]
    })).unwrap();
    World::build(&scene, ara_play::Role::Local, None, false, None)
        .unwrap()
        .0
}
fn view(speed: f32) -> View {
    View {
        camera: Some(ViewCamera {
            position: [2., 3., 4.],
            target: [2., 3., 3.],
            fov_y_degrees: 55.,
        }),
        capture_current: false,
        navigation_speed: Some(speed),
    }
}
#[test]
fn spectator_fixed_clock_speed_and_look_preserve_gameplay_and_source_camera() {
    let mut world = world();
    let before = world.runtime.play().session().state_hash().unwrap();
    for speed in [1., 4., 16.] {
        for hz in [30, 60, 120] {
            world.set_editor_view(view(speed)).unwrap();
            for _ in 0..hz {
                world
                    .navigate(
                        Navigation {
                            movement: [0., 0., 1.],
                            ..Default::default()
                        },
                        Duration::from_secs_f64(1. / f64::from(hz)),
                    )
                    .unwrap();
            }
            let pose = world.editor_view.pose().unwrap();
            assert!(
                (pose.position[2] - (4. - speed)).abs() < 0.001,
                "equal time at {hz}Hz and speed {speed}: {pose:?}"
            );
        }
    }
    world.set_editor_view(view(4.)).unwrap();
    world
        .navigate(
            Navigation {
                movement: [0., 0., 1.],
                fast: true,
                ..Default::default()
            },
            Duration::from_millis(100),
        )
        .unwrap();
    let fast = world.editor_view.pose().unwrap();
    assert!((fast.position[2] - 2.8).abs() < 0.001);
    world
        .navigate(
            Navigation {
                look: [30., 15.],
                ..Default::default()
            },
            Duration::ZERO,
        )
        .unwrap();
    let turned = world.editor_view.pose().unwrap();
    assert_eq!(
        turned.position, fast.position,
        "RMB look turns around the eye"
    );
    let mut frame = RenderFrame::default();
    world.apply_editor_view(&mut frame);
    let camera = frame.camera.unwrap();
    assert!(
        camera.forward[0] > 0. && camera.forward[1] < 0.,
        "right/down mouse mapping: {camera:?}"
    );
    assert_eq!(
        world.runtime.tick(),
        0,
        "spectator camera remains usable while paused"
    );
    assert_eq!(
        world.runtime.play().session().state_hash().unwrap(),
        before,
        "navigation must not mutate gameplay ECS or authored camera values"
    );
    world
        .set_editor_view(View {
            camera: None,
            capture_current: false,
            navigation_speed: None,
        })
        .unwrap();
    assert!(!world.spectating());
}
fn random(world: &mut World) -> u64 {
    world
        .runtime
        .play_mut()
        .session_mut()
        .world_mut()
        .resource_mut::<ara_sim::SimContext>()
        .rng("group.isolation", "e_camera")
        .next_u64()
}
#[test]
fn same_process_runtime_rng_streams_are_independent_and_repeat_the_seed() {
    let mut first = world();
    let mut second = world();
    let first_value = random(&mut first);
    let second_value = random(&mut first);
    for _ in 0..100 {
        let _ = random(&mut first);
    }
    assert_eq!(random(&mut second), first_value);
    assert_eq!(
        random(&mut second),
        second_value,
        "advancing one session's RNG must not consume its sibling's stream"
    );
}
