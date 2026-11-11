//! Scene overlays (grid, gizmo lines and quads, icons, hover, outline) as UI instances,
//! with the same encoding as the Vulkan adapter (shader modes 2 and 5 to 8), clipped to
//! the viewport rect.
use super::{Draw, NativeUi};
use crate::Overlay;
use anyhow::{Context, Result};
use ara_ui_render::gpu::Instance;

impl NativeUi {
    pub(super) fn append_overlays(&mut self) -> Result<()> {
        let rect = self.overlay_rect;
        if rect.w <= 0. || rect.h <= 0. {
            return Ok(());
        }
        let clip = [
            (rect.x * self.dpi).max(0.) as u32,
            (rect.y * self.dpi).max(0.) as u32,
            (rect.w * self.dpi) as u32,
            (rect.h * self.dpi) as u32,
        ];
        // Outline and hover flags: bit 0 hover, the rest the group table's slot + 1.
        let groups = self
            .outline_groups
            .and_then(|id| self.slot(id))
            .map_or(0., |slot| (slot + 1) as f32 * 2.);
        // One id texture for outline, hover and the grid: the renderer's, or the debug
        // repack's packed copy of it.
        if let Some(source) = self.repack {
            self.ensure_packed(source)?;
        }
        let (ids, packed) = match self.id_input() {
            Some((slot, encoding)) => (Some(slot), encoding == crate::IdEncoding::Packed),
            None => (None, false),
        };
        let flag = if packed { crate::PACKED_FLAG } else { 0. };
        // The repack draw: one full-target record, drawn before the frame's draws.
        self.pack_instance = None;
        if let Some(packed) = self.repack.and_then(|_| self.packed.as_ref()) {
            let size = packed.size;
            self.pack_instance = Some(self.instances.len() as u32);
            self.instances.push(Instance {
                rect: [0., 0., size[0] as f32, size[1] as f32],
                uv: [0., 0., 1., 1.],
                color: [1.; 4],
                params: [0.; 4],
            });
        }
        let blank = self.slot(0).context("blank UI texture")?;
        let start = self.draws.len();
        for overlay in &self.overlays {
            let draw = |texture, mode, first| Draw {
                first,
                count: 1,
                texture,
                mode,
                clip,
                mask_offset: 0,
                mask_count: 0,
            };
            let first = self.instances.len() as u32;
            let (instance, mode, texture) = match overlay {
                Overlay::Grid {
                    camera,
                    parameters,
                    color,
                    axes,
                } => {
                    let texture = ids.context("grid ID texture missing")?;
                    self.draws.push(draw(texture, 7, first));
                    self.instances.push(Instance {
                        rect: [rect.x, rect.y, rect.w, rect.h],
                        uv: *camera,
                        color: *color,
                        params: *parameters,
                    });
                    // The axes record follows the grid's and is read, not drawn.
                    self.instances.push(Instance {
                        rect: axes[0],
                        uv: axes[1],
                        color: [0.; 4],
                        // The id texture's encoding (shader mode 7 reads axes.parameters.x).
                        params: [flag.min(1.), 0., 0., 0.],
                    });
                    continue;
                }
                Overlay::Line {
                    from,
                    to,
                    width,
                    color,
                } => {
                    let x = from[0].min(to[0]) - width - 1.;
                    let y = from[1].min(to[1]) - width - 1.;
                    let w = (from[0] - to[0]).abs() + width * 2. + 2.;
                    let h = (from[1] - to[1]).abs() + width * 2. + 2.;
                    let line = Instance {
                        rect: [x, y, w, h],
                        uv: [from[0] - x, from[1] - y, to[0] - x, to[1] - y],
                        color: *color,
                        params: [*width, 0., 0., 0.],
                    };
                    (line, 6, blank)
                }
                Overlay::Quad { corners, color } => {
                    let min = corners
                        .iter()
                        .fold([f32::MAX; 2], |m, c| [m[0].min(c[0]), m[1].min(c[1])]);
                    let max = corners
                        .iter()
                        .fold([f32::MIN; 2], |m, c| [m[0].max(c[0]), m[1].max(c[1])]);
                    // One pixel of room on every side for the antialiased edge.
                    let (x, y) = (min[0] - 1., min[1] - 1.);
                    let local = corners.map(|c| [c[0] - x, c[1] - y]);
                    let quad = Instance {
                        rect: [x, y, max[0] - min[0] + 2., max[1] - min[1] + 2.],
                        uv: [local[0][0], local[0][1], local[1][0], local[1][1]],
                        color: *color,
                        params: [local[2][0], local[2][1], local[3][0], local[3][1]],
                    };
                    (quad, 8, blank)
                }
                // Mode 2 multiplies the tint's alpha by the image's.
                Overlay::Icon { rect, image, color } => {
                    let icon = Instance {
                        rect: *rect,
                        uv: [0., 0., 1., 1.],
                        color: *color,
                        params: [0.; 4],
                    };
                    // Icons install with the UI images; none yet, none drawn.
                    let Some(slot) = self.slot(*image) else {
                        continue;
                    };
                    (icon, 2, slot)
                }
                Overlay::Hover { uv, color } => {
                    let hover = Instance {
                        rect: [rect.x, rect.y, rect.w, rect.h],
                        uv: [0., 0., 1., 1.],
                        color: *color,
                        params: [uv[0], uv[1], 0., groups + 1. + flag],
                    };
                    (hover, 5, ids.context("hover texture missing")?)
                }
                Overlay::Outline { index, color } => {
                    let outline = Instance {
                        rect: [rect.x, rect.y, rect.w, rect.h],
                        uv: [0., 0., 1., 1.],
                        color: *color,
                        params: [0., 0., *index as f32, groups + flag],
                    };
                    (outline, 5, ids.context("outline texture missing")?)
                }
            };
            // Consecutive lines, quads and icons of one image share a draw (as on Vulkan).
            let batched = self.draws.len() > start;
            match self.draws.last_mut() {
                Some(last)
                    if batched
                        && matches!(mode, 2 | 6 | 8)
                        && last.mode == mode
                        && last.texture == texture
                        && last.first + last.count == first =>
                {
                    last.count += 1
                }
                _ => self.draws.push(draw(texture, mode, first)),
            }
            self.instances.push(instance);
        }
        Ok(())
    }
}
