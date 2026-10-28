//! Canonical brand artwork, using the checked-in icon family's size-specific frames.
use super::canvas::RectF;
use crate::flow::Kind;
use windows_sys::Win32::{
    Graphics::Gdi::HDC,
    UI::WindowsAndMessaging::{
        CreateIconFromResourceEx, DI_NORMAL, DestroyIcon, DrawIconEx, HICON, LR_DEFAULTCOLOR,
    },
};

const ENGINE: &[u8] = include_bytes!("../../../../assets/brand/icons/engine/ara-engine.ico");
const HUB: &[u8] = include_bytes!("../../../../assets/brand/icons/hub/ara-hub.ico");

/// An owned native icon, suitable for the taskbar or the offscreen GDI painter.
pub(super) struct Icon(HICON);

impl Icon {
    /// Choose the smallest canonical raster frame that covers the requested DPI size.
    pub fn load(kind: Kind, size: i32) -> Option<Self> {
        let bytes = match kind {
            Kind::Engine => ENGINE,
            Kind::Hub => HUB,
        };
        if bytes.get(..4)? != [0, 0, 1, 0] {
            return None;
        }
        let count = u16::from_le_bytes(bytes.get(4..6)?.try_into().ok()?);
        let requested = size.max(1) as u32;
        let frame = (0..usize::from(count))
            .filter_map(|index| {
                let start = 6 + index * 16;
                let entry = bytes.get(start..start + 16)?;
                let width = if entry[0] == 0 {
                    256
                } else {
                    u32::from(entry[0])
                };
                let length = u32::from_le_bytes(entry.get(8..12)?.try_into().ok()?) as usize;
                let offset = u32::from_le_bytes(entry.get(12..16)?.try_into().ok()?) as usize;
                let image = bytes.get(offset..offset.checked_add(length)?)?;
                Some(((width < requested, width.abs_diff(requested)), image))
            })
            .min_by_key(|(score, _)| *score)?
            .1;
        let mut frame = frame.to_vec();
        // SAFETY: the selected checked ICO frame and its exact length remain live
        // for the call. Windows copies the resource into the returned owned HICON.
        let handle = unsafe {
            CreateIconFromResourceEx(
                frame.as_mut_ptr(),
                frame.len().try_into().ok()?,
                1,
                0x0003_0000,
                size,
                size,
                LR_DEFAULTCOLOR,
            )
        };
        (!handle.is_null()).then_some(Self(handle))
    }

    pub fn handle(&self) -> HICON {
        self.0
    }

    pub fn draw(&self, dc: HDC, rect: RectF) {
        // SAFETY: this icon is owned until the call completes; the painter's DC
        // and destination rectangle remain live and no window is created.
        unsafe {
            DrawIconEx(
                dc,
                rect.x.round() as i32,
                rect.y.round() as i32,
                self.0,
                rect.w.round() as i32,
                rect.h.round() as i32,
                0,
                std::ptr::null_mut(),
                DI_NORMAL,
            );
        }
    }
}

impl Drop for Icon {
    fn drop(&mut self) {
        // SAFETY: CreateIconFromResourceEx returned this owned, non-shared icon.
        unsafe { DestroyIcon(self.0) };
    }
}
