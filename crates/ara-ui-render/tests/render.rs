use ara_ui::{
    Result,
    paint::{Box2, DisplayList, Draw},
    style::Color,
};
use ara_ui_render::{
    CpuGpu,
    atlas::TextMode,
    gpu::{Image, Pipeline, Renderer},
};
#[test]
fn image_resources_nested_rounded_clips_batches_and_alpha() -> Result<()> {
    let list = DisplayList {
        commands: vec![
            Draw::PushTransform {
                translation: [4.0, 4.0],
            },
            Draw::PushRoundedClip {
                rect: Box2 {
                    x: 0.0,
                    y: 0.0,
                    w: 24.0,
                    h: 24.0,
                },
                radius: 8.0,
            },
            Draw::Rect {
                node: 1,
                rect: Box2 {
                    x: 0.0,
                    y: 0.0,
                    w: 24.0,
                    h: 24.0,
                },
                color: Color(255, 0, 0, 128),
            },
            Draw::Image {
                node: 2,
                rect: Box2 {
                    x: 8.0,
                    y: 8.0,
                    w: 8.0,
                    h: 8.0,
                },
                image: 7,
            },
            Draw::PopClip,
            Draw::PopTransform,
        ],
        glyphs: vec![],
    };
    let mut renderer = Renderer::default();
    renderer.image(
        7,
        Image {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 255, 255],
        },
    )?;
    let mut cpu = CpuGpu::default();
    renderer.render(&mut cpu, &list, &[], [32, 32], TextMode::Raster)?;
    let data = cpu.pixmap()?.data().to_vec();
    let pixel = |x: usize, y: usize| &data[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4];
    assert_eq!(pixel(4, 4), [0, 0, 0, 0]);
    assert_eq!(pixel(16, 16), [0, 0, 255, 255]);
    assert_eq!(pixel(16, 5), [128, 0, 0, 128]);
    assert_eq!(renderer.batches.len(), 2);
    assert_eq!(renderer.batches[1].pipeline, Pipeline::Image);
    assert_eq!(renderer.batches[1].break_cause, "pipeline change");
    renderer.render(&mut cpu, &list, &[], [32, 32], TextMode::Raster)?;
    assert_eq!(cpu.pixmap()?.data(), data);
    Ok(())
}
#[test]
fn scalable_glyphs_use_real_distance_channels_and_survive_device_reset() -> Result<()> {
    let mut text = ara_ui_text::TextSystem::bundled()?;
    let mut ui = ara_ui::Ui::editor(
        "<ui id='r'><text id='t' text='Ara' style='font-size:40px' /></ui>",
        ara_ui::theme::EDITOR_CSS,
        [160.0, 90.0],
    )?;
    ui.frame(&mut text)?;
    let mut renderer = Renderer::default();
    let mut cpu = CpuGpu::default();
    renderer.render(
        &mut cpu,
        &ui.display,
        &text.assets,
        [160, 90],
        TextMode::Msdf,
    )?;
    assert!(
        renderer
            .batches
            .iter()
            .any(|b| b.pipeline == Pipeline::Msdf)
    );
    assert!(renderer.atlas.bytes() > 0);
    let before = cpu.pixmap()?.data().to_vec();
    renderer.atlas.reset();
    renderer.render(
        &mut cpu,
        &ui.display,
        &text.assets,
        [160, 90],
        TextMode::Msdf,
    )?;
    assert_eq!(cpu.pixmap()?.data(), before);
    assert_eq!(std::mem::size_of::<ara_ui_render::gpu::Instance>(), 64);
    assert_eq!(std::mem::align_of::<ara_ui_render::gpu::Instance>(), 16);
    Ok(())
}
#[test]
fn malformed_display_lists_are_rejected_before_submission() {
    let mut renderer = Renderer::default();
    for commands in [
        vec![Draw::PopClip],
        vec![Draw::PopTransform],
        vec![Draw::PushClip {
            rect: Box2 {
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
            },
        }],
    ] {
        assert!(
            renderer
                .prepare(
                    &DisplayList {
                        commands,
                        glyphs: vec![]
                    },
                    &[],
                    [32, 32],
                    TextMode::Raster
                )
                .is_err()
        );
    }
}
