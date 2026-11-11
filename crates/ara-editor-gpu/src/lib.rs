//! The editor's native UiGpu adapter, sharing ara-render's device and offscreen scene
//! images: Vulkan everywhere except macOS, where it draws through the Metal backend's
//! portable API (`metal`). Both expose the same `NativeUi` and `PlaySurface` API,
//! plus `UiDevice`, `ui_device`, `open_presentation` and the `NativeUi` window
//! presentation calls the editor, splash and Hub use.
#![warn(missing_docs)]
#[cfg(target_os = "macos")]
mod metal;
#[cfg(not(target_os = "macos"))]
mod vulkan;

#[cfg(target_os = "macos")]
pub use metal::{NativeUi, PlaySurface, UiDevice, open_presentation, ui_device};
#[cfg(not(target_os = "macos"))]
pub use vulkan::{NativeUi, PlaySurface, UiDevice, open_presentation, ui_device};

/// How the id texture the overlays read encodes entity ids. Outlines, hover and the grid's
/// occlusion read ids through one fetch (`idAt` in `ui_body.slang`) for either encoding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IdEncoding {
    /// ara-render's own id channel: a float texel (R32F), the compact channel index + 1,
    /// 0 for nothing.
    #[default]
    Float,
    /// A packed id image: RGBA8 UNORM (not sRGB), one texel per output pixel, the 32-bit id
    /// little-endian across R, G, B, A (R = bits 0 to 7). 0 is nothing; [`NO_ID_HIT`] is a
    /// hit without an id (it hides the grid, it never outlines). The id is the same compact
    /// channel index + 1 as the float encoding's.
    Packed,
}

/// A packed id texel meaning "something is here, but nothing selectable".
pub const NO_ID_HIT: u32 = u32::MAX;

/// The overlay instance flag bit that marks a [`IdEncoding::Packed`] id texture (an outline
/// or hover draw's `parameters.w`; the grid's axes record `parameters.x` is 1 instead).
pub(crate) const PACKED_FLAG: f32 = (1u32 << 20) as f32;

/// Editor-only overlays rendered through the same instance ABI and clip target.
pub enum Overlay {
    /// Antialiased line segment in logical pixels.
    Line {
        /// First endpoint.
        from: [f32; 2],
        /// Second endpoint.
        to: [f32; 2],
        /// Half width in logical pixels.
        width: f32,
        /// Semantic token colour, in sRGB numeric space.
        color: [f32; 4],
    },
    /// Antialiased filled convex quad in logical pixels, corners in order (either
    /// winding); a triangle repeats a corner. Gizmo cones, plane squares and cubes.
    Quad {
        /// Corners in order around the quad.
        corners: [[f32; 2]; 4],
        /// Semantic token colour, in sRGB numeric space.
        color: [f32; 4],
    },
    /// Analytic antialiased ground grid, faded by distance and occluded by scene IDs.
    Grid {
        /// Camera position and yaw, in world metres/radians.
        camera: [f32; 4],
        /// Pitch, vertical-FOV tangent, grid spacing and fade distance.
        parameters: [f32; 4],
        /// Semantic minor-grid tint.
        color: [f32; 4],
        /// Semantic X/Z axis tints.
        axes: [[f32; 4]; 2],
    },
    /// GPU-only hover lookup; no CPU ID-buffer readback per pointer event. With an
    /// outline group table ([`NativeUi::outline_groups`]) it outlines every channel in
    /// the hovered channel's hover group.
    Hover {
        /// Normalized pointer position in the viewport.
        uv: [f32; 2],
        /// Subtle semantic hover tint.
        color: [f32; 4],
    },
    /// An uploaded UI image (a white icon) tinted by its alpha, in logical pixels:
    /// the Scene's light, camera and audio icons.
    Icon {
        /// Logical x, y, width and height.
        rect: [f32; 4],
        /// Resource handle of the uploaded image (`icon-<name>`).
        image: u32,
        /// Semantic tint, in sRGB numeric space.
        color: [f32; 4],
    },
    /// Silhouette from the real entity-ID texture.
    Outline {
        /// Compact render index, one-based; with an outline group table
        /// ([`NativeUi::outline_groups`]), the outline colour group: every channel in it
        /// outlines as one shape, with no edges between its channels.
        index: u32,
        /// Semantic selection or review tint.
        color: [f32; 4],
    },
}
