mod common;
use anyhow::Result;
use ara_platform::{Event, Present, Window};
use ara_render::{Renderer, capture::ViewMode};
use std::path::Path;
fn main() -> Result<()> {
    let limit = std::env::args().find_map(|arg| {
        arg.strip_prefix("--frames=")
            .and_then(|s| s.parse::<u32>().ok())
    });
    let verify_resize = std::env::args().any(|s| s == "--resize-test");
    let window = Window::new("Ara M1 | Vulkan PBR baseline | Esc to close", [1280, 720])?;
    let mut renderer = Renderer::new(
        common::scene()?,
        window.size()?,
        &window.extensions()?,
        ".sn/gpu",
    )?;
    // SAFETY: Main retains the window/device and tears down presentation before either drops.
    let mut present = unsafe { Present::create(renderer.gpu(), &window)? };
    let result = (|| -> Result<()> {
        let mut frames = 0;
        let mut running = true;
        let mut angle = 0f32;
        while running {
            for event in window.poll()? {
                match event {
                    Event::Quit
                    | Event::Key {
                        scancode: 41,
                        pressed: true,
                        ..
                    } => running = false,
                    Event::MouseMotion { delta, .. } => {
                        angle += delta[0] * 0.002;
                    }
                    _ => {}
                }
            }
            if !running {
                break;
            }
            let size = window.size()?;
            if size.contains(&0) {
                std::thread::yield_now();
                continue;
            }
            if size != renderer.size() {
                renderer.gpu().wait_idle()?;
                // SAFETY: Device idle was established immediately above; the creating device remains live.
                unsafe {
                    present.retire(renderer.gpu());
                }
                // SAFETY: Window/device are retained by main until explicit idle teardown.
                present = unsafe { Present::create(renderer.gpu(), &window)? };
                renderer.resize(size)?;
            }
            let mut camera = common::camera();
            if let ara_render::scene::CameraSpec::LookAt { position, .. } = &mut camera {
                *position = glam::Mat3::from_rotation_y(angle) * (*position);
            }
            renderer.render(&camera, 0.0015)?;
            // SAFETY: Renderer completed this CopySource image and both native owners remain live.
            if let Err(error) =
                unsafe { present.present_image(renderer.gpu(), renderer.output(), renderer.size()) }
            {
                if error.downcast_ref::<ara_gpu::vk::Result>()
                    == Some(&ara_gpu::vk::Result::ERROR_OUT_OF_DATE_KHR)
                {
                    renderer.gpu().wait_idle()?;
                    // SAFETY: Device idle was established immediately above; the creating device remains live.
                    unsafe {
                        present.retire(renderer.gpu());
                    }
                    // SAFETY: Window/device are retained by main until explicit idle teardown.
                    present = unsafe { Present::create(renderer.gpu(), &window)? };
                } else {
                    return Err(error);
                }
            }
            frames += 1;
            if verify_resize && frames == 3 {
                window.set_size([960, 540])?;
            }
            if limit.is_some_and(|limit| frames >= limit) {
                break;
            }
        }
        let mut request = common::request(renderer.size());
        request.mode = ViewMode::Lit;
        renderer
            .capture(&request)?
            .write(Path::new("crates/ara-render/captures/window.png"))?;
        renderer.gpu().assert_clean()?;
        Ok(())
    })();
    renderer.gpu().wait_idle()?;
    // SAFETY: Device idle was established immediately above; the creating device remains live.
    unsafe {
        present.retire(renderer.gpu());
    }
    result
}
