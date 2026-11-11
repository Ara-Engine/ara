//! Where the studio's native layers present: the Scene view, the Game view and the graph
//! canvases each sit in a native layer under a hole in the UI (stillkit's
//! `Surface::native_layer`), and their GPU UI adapter (`crate::gpu::NativeUi`) presents
//! its finished image into it. This is the one platform seam of the studio's drawing:
//!
//! - macOS: the layer is a `CAMetalLayer`; `NativeUi::create_layer_present` and
//!   `Present::resize` (ara-platform's Metal presenter).
//! - Windows: the layer is a child `HWND` covering the hole; a Vulkan swapchain for its
//!   client area (`VK_KHR_win32_surface`), sized explicitly, since the child window is
//!   not an SDL window. Resizes are applied at the next presentation pass.
//! - Linux: the layer is a Wayland subsurface below the window; a Vulkan swapchain for it
//!   (`VK_KHR_wayland_surface`), sized explicitly (a Wayland surface takes the swapchain's
//!   size). Resizes are applied at the next presentation pass, as on Windows.
//!
//! Everything above this module (what is drawn, when, at what size) is shared.
use std::num::NonZeroIsize;
use std::ptr::NonNull;

use anyhow::{Context, Result};

#[cfg_attr(
    not(any(target_os = "macos", windows, target_os = "linux")),
    allow(unused_imports)
)]
use crate::gpu::{NativeUi, ui_device};

/// A native layer to present into, as stillkit gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Target {
    /// A live `CAMetalLayer *` (macOS).
    Metal(NonNull<std::ffi::c_void>),
    /// A child `HWND` (Windows) covering the layer's rect.
    Hwnd(NonZeroIsize),
    /// A Wayland subsurface (Linux): its connection's `wl_display *` and its `wl_surface *`.
    Wayland {
        /// The `wl_display *`.
        display: NonNull<std::ffi::c_void>,
        /// The subsurface's `wl_surface *`.
        surface: NonNull<std::ffi::c_void>,
    },
}

impl Target {
    /// A stable identity: another layer needs its own presentation.
    pub(super) fn id(self) -> usize {
        match self {
            Self::Metal(layer) => layer.as_ptr() as usize,
            Self::Hwnd(hwnd) => hwnd.get() as usize,
            Self::Wayland { surface, .. } => surface.as_ptr() as usize,
        }
    }
}

/// The target behind a stillkit native layer.
pub(super) fn target(handle: stillkit::NativeHandle) -> Result<Target> {
    match handle {
        stillkit::NativeHandle::Metal(layer) => NonNull::new(layer)
            .map(Target::Metal)
            .context("the native layer is null"),
        stillkit::NativeHandle::Hwnd(hwnd) => NonZeroIsize::new(hwnd)
            .map(Target::Hwnd)
            .context("the native layer's window is null"),
        stillkit::NativeHandle::Wayland { display, surface } => {
            match (NonNull::new(display), NonNull::new(surface)) {
                (Some(display), Some(surface)) => Ok(Target::Wayland { display, surface }),
                _ => anyhow::bail!("the native layer's Wayland surface is null"),
            }
        }
    }
}

/// A presentation of `ui`'s image into `target` at `extent` physical pixels. The UI thread
/// presents: it never waits on a full swapchain or drawable pool (a covered window shows
/// none); that frame is skipped and the next pass presents again.
/// # Safety
/// `target` is live and outlives the presentation; call on its thread; the renderer
/// outlives the presentation.
pub(super) unsafe fn create(
    ui: &NativeUi,
    renderer: &ara_render::Renderer,
    target: Target,
    extent: [u32; 2],
) -> Result<ara_platform::Present> {
    match target {
        #[cfg(target_os = "macos")]
        Target::Metal(layer) => {
            // SAFETY: forwarded caller contract.
            let mut present =
                unsafe { ui.create_layer_present(ui_device(renderer.gpu()), layer, extent)? };
            present.set_wait(std::time::Duration::from_millis(1))?;
            Ok(present)
        }
        #[cfg(windows)]
        Target::Hwnd(hwnd) => {
            // SAFETY: forwarded caller contract.
            unsafe { ui.create_hwnd_present(ui_device(renderer.gpu()), hwnd, extent) }
        }
        #[cfg(target_os = "linux")]
        Target::Wayland { display, surface } => {
            // SAFETY: forwarded caller contract.
            unsafe {
                ui.create_wayland_present(ui_device(renderer.gpu()), display, surface, extent)
            }
        }
        other => {
            let _ = (ui, renderer, extent);
            anyhow::bail!(
                "the studio cannot present into {other:?} on this platform yet; run with --legacy-ui"
            )
        }
    }
}

/// Matches `present` to its layer's new size, `extent` physical pixels.
pub(super) fn resize(present: &mut ara_platform::Present, extent: [u32; 2]) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        present.resize(extent)
    }
    #[cfg(any(windows, target_os = "linux"))]
    {
        present.request_resize(extent)
    }
    #[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
    {
        let _ = (present, extent);
        anyhow::bail!("the studio cannot resize a layer presentation on this platform yet")
    }
}
