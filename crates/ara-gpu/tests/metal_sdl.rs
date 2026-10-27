//! Presents a frame through an SDL3 Metal view: the window path `ara-platform` uses on
//! macOS (`SDL_Metal_CreateView` -> `SDL_Metal_GetLayer` -> `Presenter::from_layer`).
//! A custom harness keeps SDL video on the main thread (Cocoa requires it); the window
//! stays hidden and the process uses SDL's background-app policy, so nothing appears
//! on screen or in the Dock.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    use ara_gpu::commands::{Attachment, Command};
    use ara_gpu::metal::{MetalDevice, present::Presenter};
    use ara_gpu::{Queue, vk};
    use sdl3_sys::{hints::*, init::*, metal::*, video::*};
    use std::ptr::NonNull;

    let size = [64, 32];
    // SAFETY: SDL is used from the main thread only, before any window exists; the
    // hint and title are NUL-terminated literals.
    let (window, view) = unsafe {
        assert!(SDL_SetHint(SDL_HINT_MAC_BACKGROUND_APP, c"1".as_ptr()));
        assert!(SDL_Init(SDL_INIT_VIDEO), "SDL video init");
        let window = SDL_CreateWindow(
            c"ara-metal-sdl-test".as_ptr(),
            size[0] as i32,
            size[1] as i32,
            SDL_WINDOW_HIDDEN | SDL_WINDOW_METAL,
        );
        assert!(!window.is_null(), "SDL window");
        let view = SDL_Metal_CreateView(window);
        assert!(!view.is_null(), "SDL Metal view");
        (window, view)
    };
    let mut gpu = MetalDevice::new_raster(&[], std::env::temp_dir()).expect("Metal 4 device");
    // SAFETY: the view's CAMetalLayer lives until SDL_Metal_DestroyView below, after the
    // presenter is dropped, and only this presenter presents into it.
    let layer = NonNull::new(unsafe { SDL_Metal_GetLayer(view) }).expect("CAMetalLayer");
    let mut presenter =
        unsafe { Presenter::from_layer(&gpu, layer, size) }.expect("presenter on the SDL layer");
    let frame = gpu
        .create_image(
            size[0],
            size[1],
            vk::Format::B8G8R8A8_UNORM,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
        )
        .expect("frame");
    for shade in [0.25, 0.75] {
        let mut rec = gpu.begin(Queue::Graphics);
        rec.push(Command::BeginRendering {
            colors: vec![Attachment {
                image: frame,
                clear: Some([shade, 0.5, 1.0, 1.0]),
            }],
            depth: None,
            size,
        });
        rec.push(Command::EndRendering);
        gpu.submit(rec.finish(), &[]).expect("render");
        presenter.display(&mut gpu, frame).expect("present");
    }
    drop(presenter);
    // SAFETY: the presenter released its layer reference; destroy in creation order.
    unsafe {
        SDL_Metal_DestroyView(view);
        SDL_DestroyWindow(window);
        SDL_Quit();
    }
    println!("metal_sdl: presented 2 frames through an SDL3 Metal view");
}
