//! Hidden child-HWND presentation regression; never shows or activates its parent.
use super::*;
use anyhow::Context;
use std::num::NonZeroIsize;

#[link(name = "user32")]
unsafe extern "system" {
    fn CreateWindowExW(
        ex: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: isize,
        menu: isize,
        instance: isize,
        parameter: *const std::ffi::c_void,
    ) -> isize;
    fn DestroyWindow(hwnd: isize) -> i32;
    fn SetWindowPos(hwnd: isize, after: isize, x: i32, y: i32, w: i32, h: i32, flags: u32) -> i32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> isize;
}
struct HiddenWindow(NonZeroIsize);
impl HiddenWindow {
    fn new(parent: Option<&Self>, size: [u32; 2]) -> Result<Self> {
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        // SAFETY: the system STATIC class and module are live. No WS_VISIBLE on
        // the parent; its child cannot become desktop-visible. NOACTIVATE on both.
        let hwnd = unsafe {
            CreateWindowExW(
                0x08000080,
                class.as_ptr(),
                class.as_ptr(),
                if parent.is_some() {
                    0x50000000
                } else {
                    0x80000000
                },
                if parent.is_some() { 0 } else { -32000 },
                if parent.is_some() { 0 } else { -32000 },
                size[0] as i32,
                size[1] as i32,
                parent.map_or(0, |p| p.0.get()),
                0,
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            )
        };
        Ok(Self(
            NonZeroIsize::new(hwnd).context("CreateWindowExW failed")?,
        ))
    }
    fn resize(&self, size: [u32; 2]) -> Result<()> {
        // SAFETY: live owner-thread HWND; NOMOVE | NOZORDER | NOACTIVATE, no show flag.
        ensure!(
            unsafe { SetWindowPos(self.0.get(), 0, 0, 0, size[0] as i32, size[1] as i32, 0x16) }
                != 0,
            "SetWindowPos failed"
        );
        Ok(())
    }
}
impl Drop for HiddenWindow {
    fn drop(&mut self) {
        // SAFETY: owner-thread window; the test retires all Vulkan resources first.
        unsafe {
            DestroyWindow(self.0.get());
        }
    }
}

#[test]
#[ignore = "requires Windows Vulkan and the shared GPU lease"]
fn child_hwnd_presents_and_resizes_without_activation() -> Result<()> {
    let parent = HiddenWindow::new(None, [320, 240])?;
    let child = HiddenWindow::new(Some(&parent), [320, 240])?;
    let extensions = ["VK_KHR_surface", "VK_KHR_win32_surface"]
        .map(|s| CString::new(s).expect("static extension has no NUL"));
    let device = open_presentation(&extensions)?;
    // SAFETY: the device and both hidden windows outlive adapter/presentation.
    let mut ui_gpu = unsafe { NativeUi::new(&device)? };
    // SAFETY: the live child and device are owned on this test thread.
    let mut present = unsafe { ui_gpu.create_hwnd_present(&device, child.0, [320, 240])? };
    let result = (|| -> Result<()> {
        let mut text = ara_ui_text::TextSystem::bundled().map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut ui = ara_ui::Ui::editor(
            "<ui id='hwnd'><text id='label' text='Native HWND swapchain' /></ui>",
            ara_ui::theme::EDITOR_CSS,
            [320., 240.],
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut paint = ara_ui_render::gpu::Renderer::default();
        for (size, count, deferred) in [
            ([320, 240], 3, false),
            ([400, 280], 1, false),
            ([256, 192], 1, true),
        ] {
            child.resize(size)?;
            if deferred {
                present.request_resize(size)?;
            } else if present.extent() != size {
                // SAFETY: same live owners and exclusive queue access.
                ensure!(
                    unsafe { present.resize_to(&device, size)? },
                    "Resize did not retire"
                );
            }
            ui.resize(size.map(|v| v as f32))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            ui.frame(&mut text).map_err(|e| anyhow::anyhow!("{e}"))?;
            ui_gpu.pin_target_size(size);
            paint
                .render(
                    &mut ui_gpu,
                    &ui.display,
                    &text.assets,
                    size,
                    ara_ui_render::atlas::TextMode::Raster,
                )
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            for _ in 0..count {
                // SAFETY: rendered target is CopySource on the same serialized queue.
                unsafe { ui_gpu.present(&device, &mut present)? };
            }
            assert_eq!(present.extent(), size);
        }
        Ok(())
    })();
    ui_gpu.wait_idle(&device)?;
    // SAFETY: all submissions/presents are idle; no future use of the presentation.
    unsafe {
        ui_gpu.retire_present(&device, &mut present);
    }
    result?;
    drop(ui_gpu);
    device.assert_clean()?;
    Ok(())
}
