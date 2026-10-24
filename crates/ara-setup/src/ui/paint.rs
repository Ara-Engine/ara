//! Draws a [`View`] into a 32-bit DIB section: shapes through [`Canvas`], then
//! Segoe UI text through GDI on the same pixels. The window blits the result;
//! [`render`] returns it as RGBA for PNG previews. Layout is in logical pixels
//! (a 480 × 200 window) times the monitor scale. Colours are the Hub's Ara Dark
//! theme tokens (`assets/themes/ara-dark.json`).
use super::{
    Action, Busy, View,
    canvas::{Canvas, RectF, Rgb, Strokes},
};
use std::sync::OnceLock;
use windows_sys::Win32::{
    Foundation::{RECT, SIZE},
    Graphics::Gdi::{
        ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLIP_DEFAULT_PRECIS,
        CreateCompatibleDC, CreateDIBSection, CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH,
        DIB_RGB_COLORS, DRAW_TEXT_FORMAT, DT_CENTER, DT_EDITCONTROL, DT_END_ELLIPSIS, DT_NOPREFIX,
        DT_PATH_ELLIPSIS, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, DeleteDC,
        DeleteObject, DrawTextW, FF_DONTCARE, FW_NORMAL, FW_SEMIBOLD, GdiFlush,
        GetTextExtentPoint32W, HBITMAP, HDC, HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS, SelectObject,
        SetBkMode, SetTextColor, TRANSPARENT,
    },
};

/// Logical window width.
pub const WIDTH: f32 = 480.0;
/// Logical window height.
pub const HEIGHT: f32 = 200.0;
/// Windows 11's window corner radius.
pub const CORNER: f32 = 8.0;
const PAD: f32 = 24.0;
const TILE: f32 = 40.0;
const TEXT_X: f32 = PAD + TILE + 14.0;
const BUTTON_H: f32 = 32.0;
const FOOTER_Y: f32 = HEIGHT - PAD - BUTTON_H;
const CONTENT_W: f32 = WIDTH - 2.0 * PAD;

/// How the window outline is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frame {
    /// DWM rounds the corners and draws the border (Windows 11).
    System,
    /// A painted square 1 px border (Windows 10).
    Square,
    /// A painted rounded border with transparent corners: the Windows 11 look,
    /// for offscreen previews.
    Preview,
}

/// Pointer and animation state the painter needs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pointer {
    /// Button under the pointer.
    pub hover: Option<Action>,
    /// Button held down.
    pub pressed: Option<Action>,
    /// Progress sheen frame, advanced by the window's timer.
    pub tick: u32,
}

/// The window size in device pixels at `scale`.
pub fn size(scale: f32) -> (i32, i32) {
    (
        (WIDTH * scale).round() as i32,
        (HEIGHT * scale).round() as i32,
    )
}

/// A memory DC with a selected top-down 32-bit DIB section.
pub struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u32,
    width: i32,
    height: i32,
}

impl Surface {
    /// `None` when GDI is out of resources.
    pub fn new(width: i32, height: i32) -> Option<Surface> {
        // SAFETY: the header describes a top-down 32 bpp bitmap; every created
        // object is checked and owned by the returned surface (released in Drop).
        unsafe {
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            if dc.is_null() {
                return None;
            }
            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader = BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..std::mem::zeroed()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(
                dc,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            );
            if bitmap.is_null() || bits.is_null() {
                DeleteDC(dc);
                return None;
            }
            let previous = SelectObject(dc, bitmap);
            Some(Surface {
                dc,
                bitmap,
                previous,
                bits: bits.cast(),
                width,
                height,
            })
        }
    }
    /// The memory DC, for blitting.
    pub fn dc(&self) -> HDC {
        self.dc
    }
    /// Size in device pixels.
    pub fn size(&self) -> (i32, i32) {
        (self.width, self.height)
    }
    fn pixels(&mut self) -> &mut [u32] {
        // SAFETY: the DIB section holds width * height pixels and lives as long as
        // the surface; GDI is flushed before the pixels are touched directly.
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.width * self.height) as usize) }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        // SAFETY: restores the DC's original bitmap, then frees what `new` created.
        unsafe {
            SelectObject(self.dc, self.previous);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

/// An offscreen rendering, straight-alpha RGBA rows.
pub struct Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

/// Renders `view` offscreen (tests and previews); no window is created.
pub fn render(view: &View, pointer: Pointer, scale: f32, frame: Frame) -> Option<Image> {
    let (width, height) = size(scale);
    let mut surface = Surface::new(width, height)?;
    paint(&mut surface, view, pointer, scale, frame);
    let rgba = surface
        .pixels()
        .iter()
        .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, (p >> 24) as u8])
        .collect();
    Some(Image {
        width: width as u32,
        height: height as u32,
        rgba,
    })
}

struct Palette {
    background: Rgb,
    border: Rgb,
    foreground: Rgb,
    muted: Rgb,
    raised: Rgb,
    hover: Rgb,
    well: Rgb,
    primary: Rgb,
    primary_hover: Rgb,
    on_primary: Rgb,
    warning: Rgb,
    warning_surface: Rgb,
    destructive: Rgb,
    destructive_surface: Rgb,
    success: Rgb,
}

fn palette() -> &'static Palette {
    static PALETTE: OnceLock<Palette> = OnceLock::new();
    PALETTE.get_or_init(|| {
        let theme: serde_json::Value =
            serde_json::from_str(include_str!("../../../../assets/themes/ara-dark.json"))
                .unwrap_or_default();
        let token = |name: &str, fallback: u32| {
            theme["colors"][name]
                .as_str()
                .map(|hex| hex.trim_start_matches('#'))
                .filter(|hex| hex.len() == 6)
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .map_or(Rgb(fallback), Rgb)
        };
        Palette {
            background: token("card", 0x111111),
            border: token("separator-strong", 0x2e2e2e),
            foreground: token("foreground", 0xf5f5f5),
            muted: token("muted-foreground", 0x818181),
            raised: token("raised", 0x1c1c1c),
            hover: token("hover-surface", 0x262626),
            well: token("background", 0x0a0a0a),
            primary: token("primary", 0x346bf1),
            primary_hover: token("selection-surface", 0x2f62dc),
            on_primary: token("primary-foreground", 0xffffff),
            warning: token("warning-foreground", 0xffb900),
            warning_surface: token("warning-surface", 0x312108),
            destructive: token("destructive-foreground", 0xff6467),
            destructive_surface: token("destructive-surface", 0x301214),
            success: token("success-foreground", 0x34d399),
        }
    })
}

#[derive(Clone, Copy)]
enum Font {
    Title,
    Body,
    Small,
    Mono,
    Button,
}

/// The fonts of one paint at one scale, deleted on drop.
struct Fonts([HFONT; 5]);

impl Fonts {
    fn new(scale: f32) -> Fonts {
        let create = |px: f32, weight: u32, face: &str| {
            let face: Vec<u16> = face.encode_utf16().chain([0]).collect();
            // SAFETY: `face` is a NUL-terminated UTF-16 string that outlives the call.
            unsafe {
                CreateFontW(
                    -(px * scale).round() as i32,
                    0,
                    0,
                    0,
                    weight as i32,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET as u32,
                    OUT_DEFAULT_PRECIS as u32,
                    CLIP_DEFAULT_PRECIS as u32,
                    ANTIALIASED_QUALITY as u32,
                    (DEFAULT_PITCH | FF_DONTCARE) as u32,
                    face.as_ptr(),
                )
            }
        };
        Fonts([
            create(15.0, FW_SEMIBOLD, "Segoe UI"),
            create(13.0, FW_NORMAL, "Segoe UI"),
            create(12.0, FW_NORMAL, "Segoe UI"),
            create(12.0, FW_NORMAL, "Consolas"),
            create(13.0, FW_SEMIBOLD, "Segoe UI"),
        ])
    }
    fn get(&self, font: Font) -> HFONT {
        self.0[font as usize]
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        for font in self.0 {
            // SAFETY: each font was created by `Fonts::new` and is no longer selected.
            unsafe { DeleteObject(font) };
        }
    }
}

#[derive(Clone, Copy)]
enum Tone {
    Brand,
    Warning,
    Error,
    Success,
}

const TRIANGLE: Strokes<'static> = Strokes {
    lines: &[
        ((12.0, 3.4), (21.7, 20.3)),
        ((21.7, 20.3), (2.3, 20.3)),
        ((2.3, 20.3), (12.0, 3.4)),
        ((12.0, 9.0), (12.0, 13.0)),
        ((12.0, 17.0), (12.0, 17.0)),
    ],
    circles: &[],
    width: 2.0,
};
const ALERT: Strokes<'static> = Strokes {
    lines: &[((12.0, 8.0), (12.0, 12.0)), ((12.0, 16.0), (12.0, 16.0))],
    circles: &[((12.0, 12.0), 10.0)],
    width: 2.0,
};
const CHECK: Strokes<'static> = Strokes {
    lines: &[((9.0, 12.0), (11.0, 14.0)), ((11.0, 14.0), (15.0, 10.0))],
    circles: &[((12.0, 12.0), 10.0)],
    width: 2.0,
};
/// `assets/brand/ara-mark.svg` (64 units): the A and its diamond.
const MARK: [&[(f32, f32)]; 2] = [
    &[
        (32.0, 4.0),
        (60.0, 60.0),
        (44.0, 60.0),
        (28.0, 28.0),
        (12.0, 60.0),
        (4.0, 60.0),
    ],
    &[(28.0, 44.0), (34.0, 50.0), (28.0, 56.0), (22.0, 50.0)],
];

enum Op {
    Fill {
        rect: RectF,
        radius: f32,
        color: Rgb,
    },
    Mark(RectF),
    Icon {
        rect: RectF,
        icon: Strokes<'static>,
        color: Rgb,
    },
    Sheen {
        rect: RectF,
        tick: u32,
    },
    Text {
        text: String,
        rect: RectF,
        font: Font,
        color: Rgb,
        format: DRAW_TEXT_FORMAT,
    },
}

const LINE: DRAW_TEXT_FORMAT = DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS;
const PATH: DRAW_TEXT_FORMAT = DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_PATH_ELLIPSIS;
const WRAP: DRAW_TEXT_FORMAT = DT_WORDBREAK | DT_EDITCONTROL | DT_NOPREFIX | DT_END_ELLIPSIS;

/// Collects shapes and text for one view, then draws shapes before text.
struct Painter<'a> {
    dc: HDC,
    fonts: &'a Fonts,
    scale: f32,
    ops: Vec<Op>,
    hits: Vec<(Action, RectF)>,
}

impl Painter<'_> {
    fn rect(&self, x: f32, y: f32, w: f32, h: f32) -> RectF {
        let s = self.scale;
        RectF {
            x: x * s,
            y: y * s,
            w: w * s,
            h: h * s,
        }
    }
    fn fill(&mut self, (x, y, w, h): (f32, f32, f32, f32), radius: f32, color: Rgb) {
        let rect = self.rect(x, y, w, h);
        self.ops.push(Op::Fill {
            rect,
            radius: radius * self.scale,
            color,
        });
    }
    /// A filled shape with a 1 px border.
    fn bordered(&mut self, bounds: (f32, f32, f32, f32), radius: f32, fill: Rgb, border: Rgb) {
        self.fill(bounds, radius, border);
        let rect = self.rect(bounds.0, bounds.1, bounds.2, bounds.3).inset(1.0);
        self.ops.push(Op::Fill {
            rect,
            radius: radius * self.scale - 1.0,
            color: fill,
        });
    }
    fn text(
        &mut self,
        text: &str,
        (x, y, w, h): (f32, f32, f32, f32),
        font: Font,
        color: Rgb,
        format: DRAW_TEXT_FORMAT,
    ) {
        // DrawTextW's ellipsis handling reads the string even when it is empty.
        if text.is_empty() || w < 1.0 || h < 1.0 {
            return;
        }
        let rect = self.rect(x, y, w, h);
        self.ops.push(Op::Text {
            text: text.to_owned(),
            rect,
            font,
            color,
            format,
        });
    }
    /// Logical width of `text` in `font`.
    fn measure(&self, font: Font, text: &str) -> f32 {
        let wide: Vec<u16> = text.encode_utf16().collect();
        let mut size = SIZE { cx: 0, cy: 0 };
        // SAFETY: the DC and font are live; `wide` holds `len` UTF-16 units.
        unsafe {
            SelectObject(self.dc, self.fonts.get(font));
            GetTextExtentPoint32W(self.dc, wide.as_ptr(), wide.len() as i32, &mut size);
        }
        size.cx as f32 / self.scale
    }

    /// Icon tile, title and subtitle.
    fn header(
        &mut self,
        tone: Tone,
        title: &str,
        subtitle: &str,
        subtitle_font: Font,
        subtitle_color: Rgb,
    ) {
        let colors = palette();
        let (surface, color) = match tone {
            Tone::Brand => (colors.background.mix(colors.primary, 0.16), colors.primary),
            Tone::Warning => (colors.warning_surface, colors.warning),
            Tone::Error => (colors.destructive_surface, colors.destructive),
            Tone::Success => (colors.background.mix(colors.success, 0.14), colors.success),
        };
        self.fill((PAD, PAD, TILE, TILE), 10.0, surface);
        let icon = match tone {
            Tone::Brand => {
                let rect = self.rect(PAD + 8.0, PAD + 8.0, 24.0, 24.0);
                self.ops.push(Op::Mark(rect));
                None
            }
            Tone::Warning => Some(TRIANGLE),
            Tone::Error => Some(ALERT),
            Tone::Success => Some(CHECK),
        };
        if let Some(icon) = icon {
            let rect = self.rect(PAD + 10.0, PAD + 10.0, 20.0, 20.0);
            self.ops.push(Op::Icon { rect, icon, color });
        }
        let width = WIDTH - PAD - TEXT_X;
        self.text(
            title,
            (TEXT_X, PAD - 1.0, width, 24.0),
            Font::Title,
            colors.foreground,
            LINE,
        );
        let format = if matches!(subtitle_font, Font::Mono) || subtitle.contains('\\') {
            PATH
        } else {
            LINE
        };
        self.text(
            subtitle,
            (TEXT_X, PAD + 22.0, width, 18.0),
            subtitle_font,
            subtitle_color,
            format,
        );
    }

    /// Right-aligned buttons; returns the free width left of them.
    fn footer(&mut self, view: &View, pointer: Pointer) -> f32 {
        let colors = palette();
        let mut right = WIDTH - PAD;
        for button in view.buttons().into_iter().rev() {
            let width = (self.measure(Font::Button, button.label) + 32.0)
                .max(84.0)
                .round();
            let x = right - width;
            let bounds = (x, FOOTER_Y, width, BUTTON_H);
            let hover = pointer.hover == Some(button.action);
            let pressed = hover && pointer.pressed == Some(button.action);
            if button.primary {
                let fill = match (hover, pressed) {
                    (_, true) => colors.primary.mix(Rgb(0), 0.12),
                    (true, _) => colors.primary_hover,
                    _ => colors.primary,
                };
                self.fill(bounds, 6.0, fill);
                self.text(
                    button.label,
                    bounds,
                    Font::Button,
                    colors.on_primary,
                    LINE | DT_CENTER,
                );
            } else {
                let fill = if hover { colors.hover } else { colors.raised };
                self.bordered(bounds, 6.0, fill, colors.border);
                self.text(
                    button.label,
                    bounds,
                    Font::Button,
                    colors.foreground,
                    LINE | DT_CENTER,
                );
            }
            let hit = self.rect(x, FOOTER_Y, width, BUTTON_H);
            self.hits.push((button.action, hit));
            right = x - 8.0;
        }
        right - PAD
    }

    fn busy(&mut self, busy: &Busy, tick: u32) {
        let colors = palette();
        self.header(
            Tone::Brand,
            &busy.title,
            &busy.subtitle,
            Font::Small,
            colors.muted,
        );
        let percent = format!("{}%", (busy.fraction.clamp(0.0, 1.0) * 100.0).floor());
        self.text(
            &busy.label,
            (PAD, 114.0, CONTENT_W - 60.0, 20.0),
            Font::Body,
            colors.foreground,
            LINE,
        );
        self.text(
            &percent,
            (WIDTH - PAD - 60.0, 114.0, 60.0, 20.0),
            Font::Body,
            colors.muted,
            LINE | DT_RIGHT,
        );
        let track = (PAD, 140.0, CONTENT_W, 6.0);
        self.fill(track, 3.0, colors.raised);
        let filled = (CONTENT_W * busy.fraction.clamp(0.0, 1.0)).max(6.0);
        self.fill((PAD, 140.0, filled, 6.0), 3.0, colors.primary);
        let rect = self.rect(PAD, 140.0, filled, 6.0);
        self.ops.push(Op::Sheen { rect, tick });
        let amount_w = if busy.amount.is_empty() {
            0.0
        } else {
            self.measure(Font::Small, &busy.amount) + 16.0
        };
        self.text(
            &busy.detail,
            (PAD, 154.0, CONTENT_W - amount_w, 18.0),
            Font::Small,
            colors.muted,
            PATH,
        );
        self.text(
            &busy.amount,
            (WIDTH - PAD - amount_w, 154.0, amount_w, 18.0),
            Font::Small,
            colors.muted,
            LINE | DT_RIGHT,
        );
    }
}

/// Draws `view` and returns each button's device-pixel rectangle.
pub fn paint(
    surface: &mut Surface,
    view: &View,
    pointer: Pointer,
    scale: f32,
    frame: Frame,
) -> Vec<(Action, RectF)> {
    let colors = palette();
    let fonts = Fonts::new(scale);
    let mut p = Painter {
        dc: surface.dc(),
        fonts: &fonts,
        scale,
        ops: Vec::new(),
        hits: Vec::new(),
    };
    match view {
        View::Busy(busy) => p.busy(busy, pointer.tick),
        View::Confirm {
            title,
            subtitle,
            lead,
            destination,
        } => {
            p.header(Tone::Warning, title, subtitle, Font::Small, colors.muted);
            p.text(
                lead,
                (PAD, 78.0, CONTENT_W, 20.0),
                Font::Body,
                colors.foreground,
                LINE,
            );
            let well = (PAD, 102.0, CONTENT_W, 30.0);
            p.bordered(well, 6.0, colors.well, colors.border);
            p.text(
                destination,
                (PAD + 12.0, 102.0, CONTENT_W - 24.0, 30.0),
                Font::Mono,
                colors.foreground,
                PATH,
            );
            let free = p.footer(view, pointer);
            p.text(
                "Projects and settings are kept.",
                (PAD, FOOTER_Y, free, BUTTON_H),
                Font::Small,
                colors.muted,
                LINE,
            );
        }
        View::Blocked {
            title,
            subtitle,
            programs,
            ..
        } => {
            p.header(Tone::Warning, title, subtitle, Font::Small, colors.muted);
            if programs.is_empty() {
                p.text(
                    "Close Ara windows, terminals and agents that use this folder, then select Retry.",
                    (PAD, 80.0, CONTENT_W, 40.0),
                    Font::Body,
                    colors.foreground,
                    WRAP,
                );
            }
            let shown = if programs.len() > 3 {
                2
            } else {
                programs.len()
            };
            for (row, (name, detail)) in programs.iter().take(shown).enumerate() {
                let y = 78.0 + 20.0 * row as f32;
                p.fill((PAD + 2.0, y + 7.0, 6.0, 6.0), 3.0, colors.warning);
                p.text(
                    name,
                    (PAD + 16.0, y, CONTENT_W - 16.0, 20.0),
                    Font::Body,
                    colors.foreground,
                    LINE,
                );
                let x = PAD + 16.0 + p.measure(Font::Body, name) + 10.0;
                p.text(
                    detail,
                    (x, y, WIDTH - PAD - x, 20.0),
                    Font::Small,
                    colors.muted,
                    LINE,
                );
            }
            if programs.len() > shown {
                let more = format!("and {} more", programs.len() - shown);
                p.text(
                    &more,
                    (PAD + 16.0, 118.0, CONTENT_W - 16.0, 20.0),
                    Font::Small,
                    colors.muted,
                    LINE,
                );
            }
            p.footer(view, pointer);
        }
        View::Error {
            title,
            code,
            message,
            ..
        } => {
            match code {
                Some(code) => p.header(Tone::Error, title, code, Font::Mono, colors.destructive),
                None => p.header(
                    Tone::Error,
                    title,
                    "Unexpected error",
                    Font::Small,
                    colors.muted,
                ),
            }
            p.text(
                message,
                (PAD, 78.0, CONTENT_W, 58.0),
                Font::Body,
                colors.foreground,
                WRAP,
            );
            p.footer(view, pointer);
        }
        View::Done {
            title,
            destination,
            note,
            warning,
            ..
        } => {
            p.header(Tone::Success, title, destination, Font::Small, colors.muted);
            p.text(
                note,
                (PAD, 78.0, CONTENT_W, 40.0),
                Font::Body,
                colors.foreground,
                WRAP,
            );
            if let Some(warning) = warning {
                p.text(
                    warning,
                    (PAD, 118.0, CONTENT_W, 18.0),
                    Font::Small,
                    colors.warning,
                    PATH,
                );
            }
            p.footer(view, pointer);
        }
    }
    let Painter { ops, hits, .. } = p;
    let (width, height) = surface.size();
    let full = RectF {
        x: 0.0,
        y: 0.0,
        w: width as f32,
        h: height as f32,
    };
    let mut texts = Vec::new();
    {
        let mut canvas = Canvas::new(width as usize, height as usize, surface.pixels());
        canvas.clear(colors.background);
        for op in ops {
            match op {
                Op::Fill {
                    rect,
                    radius,
                    color,
                } => canvas.round_rect(rect, radius, color),
                Op::Mark(rect) => {
                    for shape in MARK {
                        let points: Vec<_> = shape
                            .iter()
                            .map(|(x, y)| (rect.x + x * rect.w / 64.0, rect.y + y * rect.h / 64.0))
                            .collect();
                        canvas.polygon(&points, colors.primary);
                    }
                }
                Op::Icon { rect, icon, color } => canvas.strokes(rect, 24.0, icon, color),
                Op::Sheen { rect, tick } => {
                    // A soft highlight sweeping along the filled bar.
                    let band = 90.0 * scale;
                    let travel = rect.w + 2.0 * band;
                    let centre = rect.x - band + (tick as f32 * 5.0 * scale) % travel;
                    let radius = rect.h / 2.0;
                    canvas.shade(rect, colors.on_primary, |x, y| {
                        let inside =
                            (0.5 - super::canvas::rounded(x, y, rect, radius)).clamp(0.0, 1.0);
                        let fade = (1.0 - (x - centre).abs() / band).max(0.0);
                        inside * 0.28 * fade * fade
                    });
                }
                text @ Op::Text { .. } => texts.push(text),
            }
        }
        match frame {
            Frame::System => {}
            Frame::Square => ring(&mut canvas, full, 0.0, colors.border),
            Frame::Preview => ring(&mut canvas, full, CORNER * scale, colors.border),
        }
    }
    // SAFETY: the DC owns the DIB written above; text is drawn after the direct
    // pixel writes and GDI is flushed before the pixels are touched again.
    unsafe {
        GdiFlush();
        SetBkMode(surface.dc(), TRANSPARENT as i32);
        for op in texts {
            if let Op::Text {
                text,
                rect,
                font,
                color,
                format,
            } = op
            {
                // NUL-terminated with spare room, as ellipsis handling may look past the end.
                let length = text.encode_utf16().count() as i32;
                let mut wide: Vec<u16> = text.encode_utf16().chain([0; 5]).collect();
                let mut bounds = RECT {
                    left: rect.x.round() as i32,
                    top: rect.y.round() as i32,
                    right: (rect.x + rect.w).round() as i32,
                    bottom: (rect.y + rect.h).round() as i32,
                };
                SelectObject(surface.dc(), fonts.get(font));
                SetTextColor(surface.dc(), color.colorref());
                DrawTextW(surface.dc(), wide.as_mut_ptr(), length, &mut bounds, format);
            }
        }
        GdiFlush();
    }
    let mut canvas = Canvas::new(width as usize, height as usize, surface.pixels());
    match frame {
        Frame::Preview => canvas.mask_corners(CORNER * scale),
        Frame::System | Frame::Square => canvas.opaque(),
    }
    hits
}

/// A 1 px outline along the window edge.
fn ring(canvas: &mut Canvas<'_>, full: RectF, radius: f32, color: Rgb) {
    canvas.shade(full, color, |x, y| {
        let outer = (0.5 - super::canvas::rounded(x, y, full, radius)).clamp(0.0, 1.0);
        let inner = (0.5 - super::canvas::rounded(x, y, full.inset(1.0), (radius - 1.0).max(0.0)))
            .clamp(0.0, 1.0);
        outer - inner
    });
}

/// The window border colour (`--separator-strong`), for the DWM frame.
pub fn border() -> Rgb {
    palette().border
}
