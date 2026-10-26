use super::*;
use std::time::{Duration, Instant};

fn ready(renderer: &mut Renderer, candidate: &mut NativeContent) -> Result<()> {
    let start = Instant::now();
    while !candidate.preallocate(renderer)? {
        ensure!(
            start.elapsed() < Duration::from_secs(5),
            "native preparation timeout"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

#[test]
fn worker_content_reserves_quality_shadow_rows_and_keeps_them_on_rejection() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-render/prepared-shadow-capacity");
    let mut renderer = Renderer::new_native(
        &shadows::scene(),
        Box::new(EmptyTextures),
        [64, 48],
        &[],
        root,
    )?;
    let quality = crate::look::Quality {
        local_shadows: Some(crate::look::LocalShadows {
            lights: 8,
            resolution: 128,
            redraws: 6,
            in_gi_and_fog: false,
        }),
        ..*renderer.quality()
    };
    let seed = renderer.native_seed_for_quality(&quality)?;
    let mut candidate = std::thread::spawn(move || {
        NativeContent::new(shadows::scene(), Box::new(EmptyTextures), Some(seed))
    })
    .join()
    .expect("content worker")?;
    ready(&mut renderer, &mut candidate)?;
    renderer.publish_native_content(candidate)?;
    let required = Renderer::shadow_slots_for_quality(&quality);
    let native = renderer.native.as_ref().expect("native content");
    assert!(native.capacity >= culling::BASE_PASSES + required);
    let visible = native.visible;
    renderer.set_quality(quality)?;
    assert_eq!(
        renderer.native.as_ref().expect("native").visible,
        visible,
        "applying the prepared quality must not grow shadow rows"
    );
    renderer.reserve_native_shadow_passes(required)?;
    let usage = renderer.gpu().descriptor_usage();
    for _ in 0..4 {
        renderer.reserve_native_shadow_passes(required)?;
        let mut rejected = NativeContent::new(
            shadows::scene(),
            Box::new(RejectPublication),
            Some(renderer.native_seed()),
        )?;
        ready(&mut renderer, &mut rejected)?;
        assert!(renderer.publish_native_content(rejected).is_err());
        assert_eq!(
            renderer.native.as_ref().expect("last good").visible,
            visible
        );
        assert_eq!(renderer.gpu().descriptor_usage(), usage);
    }
    let lower = crate::look::Quality {
        local_shadows: None,
        ..quality
    };
    renderer.set_quality(lower)?;
    let seed = renderer.native_seed_for_quality(&lower)?;
    let mut replacement =
        NativeContent::new(shadows::scene(), Box::new(EmptyTextures), Some(seed))?;
    ready(&mut renderer, &mut replacement)?;
    renderer.publish_native_content(replacement)?;
    let native = renderer.native.as_ref().expect("replacement");
    assert!(
        native.capacity >= culling::BASE_PASSES + required,
        "replacement retains the larger source capacity"
    );
    let visible = native.visible;
    renderer.reserve_native_shadow_passes(required)?;
    assert_eq!(renderer.native.as_ref().expect("native").visible, visible);
    renderer.gpu().assert_clean()?;
    Ok(())
}
