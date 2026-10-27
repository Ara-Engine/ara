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
fn missing_image_does_not_abort_the_remaining_controls() -> Result<()> {
    let list = DisplayList {
        commands: vec![
            Draw::Image {
                node: 1,
                rect: Box2 {
                    x: 0.,
                    y: 0.,
                    w: 8.,
                    h: 8.,
                },
                image: 7,
                tint: Color(255, 255, 255, 255),
            },
            Draw::Rect {
                node: 2,
                rect: Box2 {
                    x: 8.,
                    y: 0.,
                    w: 8.,
                    h: 8.,
                },
                color: Color(0, 255, 0, 255),
            },
        ],
        glyphs: vec![],
    };
    let mut renderer = Renderer::default();
    let mut cpu = CpuGpu::default();
    renderer.render(&mut cpu, &list, &[], [16, 8], TextMode::Raster)?;
    assert_eq!(renderer.missing_images, [7]);
    assert_eq!(&cpu.pixmap()?.data()[..4], &[0, 0, 0, 0]);
    assert_eq!(&cpu.pixmap()?.data()[8 * 4..9 * 4], &[0, 255, 0, 255]);
    Ok(())
}

#[test]
fn borrowed_image_keeps_adapter_pixels_without_an_owned_upload() -> Result<()> {
    use ara_ui_render::gpu::{TextureUpload, UiGpu};
    let list = DisplayList {
        commands: vec![Draw::Image {
            node: 1,
            rect: Box2 {
                x: 0.,
                y: 0.,
                w: 8.,
                h: 8.,
            },
            image: 7,
            tint: Color(255, 255, 255, 255),
        }],
        glyphs: vec![],
    };
    let mut cpu = CpuGpu::default();
    cpu.upload(TextureUpload {
        id: 7,
        width: 1,
        height: 1,
        rgba: &[0, 0, 255, 255],
    })?;
    let mut renderer = Renderer::default();
    renderer.external_image(7)?;
    renderer.render(&mut cpu, &list, &[], [8, 8], TextMode::Raster)?;
    assert!(renderer.missing_images.is_empty());
    assert_eq!(&cpu.pixmap()?.data()[..4], &[0, 0, 255, 255]);
    renderer.device_lost();
    renderer.prepare(&list, &[], [8, 8], TextMode::Raster)?;
    assert_eq!(renderer.missing_images, [7]);
    renderer.external_image(7)?;
    renderer.render(&mut cpu, &list, &[], [8, 8], TextMode::Raster)?;
    assert_eq!(&cpu.pixmap()?.data()[..4], &[0, 0, 255, 255]);
    renderer.forget_image(7);
    renderer.prepare(&list, &[], [8, 8], TextMode::Raster)?;
    assert_eq!(renderer.missing_images, [7]);
    Ok(())
}
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
                tint: Color(255, 255, 255, 255),
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

#[test]
fn scalable_glyphs_reuse_one_outline_at_every_zoom() -> Result<()> {
    use ara_ui::{paint::Draw, paint::Glyph};
    let font = ara_ui_text::FontAsset {
        source_id: 99,
        index: 0,
        bytes: include_bytes!("../../ara-ui-text/fonts/NotoSans.ttf").to_vec(),
    };
    let fonts = [font];
    let mut renderer = Renderer::default();
    let mut list = DisplayList {
        commands: vec![Draw::ScalableGlyphRun {
            node: 1,
            font: 0,
            size: 12.,
            origin: [10., 10.],
            glyphs: 0..1,
            color: Color(255, 255, 255, 255),
        }],
        glyphs: vec![Glyph {
            id: 36,
            x: 0.,
            y: 0.,
        }],
    };
    renderer.prepare(&list, &fonts, [200, 100], TextMode::Raster)?;
    let uv = renderer.instances[0].uv;
    let height = renderer.instances[0].rect[3];
    for size in [18., 24., 8., 12.5] {
        if let Draw::ScalableGlyphRun { size: current, .. } = &mut list.commands[0] {
            *current = size;
        }
        renderer.prepare(&list, &fonts, [200, 100], TextMode::Raster)?;
        assert_eq!(renderer.instances[0].uv, uv);
        assert!((renderer.instances[0].rect[3] - height * size / 12.).abs() < 0.001);
    }
    Ok(())
}
#[test]
fn analytic_lines_preserve_endpoints_through_translation_and_clipping() -> Result<()> {
    let list = DisplayList {
        commands: vec![
            Draw::PushTransform {
                translation: [8., 4.],
            },
            Draw::PushClip {
                rect: Box2 {
                    x: 0.,
                    y: 0.,
                    w: 12.,
                    h: 12.,
                },
            },
            Draw::Line {
                node: 1,
                from: [0., 6.],
                to: [16., 6.],
                width: 2.,
                color: Color(255, 0, 0, 255),
            },
            Draw::PopClip,
            Draw::PopTransform,
        ],
        glyphs: vec![],
    };
    let mut renderer = Renderer::default();
    let mut cpu = CpuGpu::default();
    renderer.render(&mut cpu, &list, &[], [32, 24], TextMode::Raster)?;
    let image = cpu.pixmap()?;
    let alpha = |x, y| image.data()[((y * 32 + x) * 4 + 3) as usize];
    assert_eq!(alpha(12, 10), 255);
    assert_eq!(alpha(21, 10), 0);
    assert_eq!(alpha(12, 15), 0);
    Ok(())
}

#[test]
fn clipped_text_does_not_submit_offscreen_glyphs() -> Result<()> {
    use ara_ui::paint::Glyph;
    let fonts = [ara_ui_text::FontAsset {
        source_id: 99,
        index: 0,
        bytes: include_bytes!("../../ara-ui-text/fonts/NotoSans.ttf").to_vec(),
    }];
    let list = DisplayList {
        commands: vec![
            Draw::PushClip {
                rect: Box2 {
                    x: 12.,
                    y: 10.,
                    w: 24.,
                    h: 24.,
                },
            },
            Draw::PushTransform {
                translation: [4., 3.],
            },
            Draw::PushClip {
                rect: Box2 {
                    x: 0.,
                    y: 0.,
                    w: 100.,
                    h: 100.,
                },
            },
            Draw::GlyphRun {
                node: 1,
                font: 0,
                size: 12.,
                origin: [10., 22.],
                glyphs: 0..4,
                color: Color(255, 255, 255, 255),
            },
            Draw::PopClip,
            Draw::PopTransform,
            Draw::PopClip,
        ],
        glyphs: [(0., 0.), (1000., 0.), (0., 1000.), (-1000., 0.)]
            .into_iter()
            .map(|(x, y)| Glyph { id: 36, x, y })
            .collect(),
    };
    let mut renderer = Renderer::default();
    renderer.prepare(&list, &fonts, [128, 128], TextMode::Raster)?;
    assert_eq!(
        renderer.instances.len(),
        1,
        "the nested ancestor clip excludes the other three glyphs"
    );
    Ok(())
}
