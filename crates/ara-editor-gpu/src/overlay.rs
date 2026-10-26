use super::*;
impl NativeUi {
    pub(super) fn append_overlays(&mut self) -> Result<()> {
        let rect = self.overlay_rect;
        if rect.w <= 0. || rect.h <= 0. {
            return Ok(());
        }
        let scissor = vk::Rect2D {
            offset: vk::Offset2D {
                x: (rect.x * self.dpi).max(0.) as i32,
                y: (rect.y * self.dpi).max(0.) as i32,
            },
            extent: vk::Extent2D {
                width: (rect.w * self.dpi) as u32,
                height: (rect.h * self.dpi) as u32,
            },
        };
        // Outline and hover flags: bit 0 hover, the rest the group table's slot + 1.
        let groups = self
            .outline_groups
            .and_then(|id| {
                self.textures
                    .get(&id)
                    .map(|t| t.slot)
                    .or_else(|| self.external.get(&id).copied())
            })
            .map_or(0., |slot| (slot + 1) as f32 * 2.);
        for overlay in &self.overlays {
            if let Overlay::Grid {
                camera,
                parameters,
                color,
                axes,
            } = overlay
            {
                let texture = self
                    .outline_texture
                    .and_then(|id| self.external.get(&id))
                    .copied()
                    .ok_or_else(|| anyhow::anyhow!("grid ID texture missing"))?;
                self.draws.push(Draw {
                    first: self.instances.len() as u32,
                    count: 1,
                    texture,
                    mode: 7,
                    scissor,
                    mask_offset: 0,
                    mask_count: 0,
                });
                self.instances.push(Instance {
                    rect: [rect.x, rect.y, rect.w, rect.h],
                    uv: *camera,
                    color: *color,
                    params: *parameters,
                });
                self.instances.push(Instance {
                    rect: axes[0],
                    uv: axes[1],
                    color: [0.; 4],
                    params: [0.; 4],
                });
                continue;
            }
            let (instance, mode, texture) = match overlay {
                Overlay::Grid { .. } => unreachable!("grid handled above"),
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
                    (
                        Instance {
                            rect: [x, y, w, h],
                            uv: [from[0] - x, from[1] - y, to[0] - x, to[1] - y],
                            color: *color,
                            params: [*width, 0., 0., 0.],
                        },
                        6,
                        0,
                    )
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
                    (
                        Instance {
                            rect: [x, y, max[0] - min[0] + 2., max[1] - min[1] + 2.],
                            uv: [local[0][0], local[0][1], local[1][0], local[1][1]],
                            color: *color,
                            params: [local[2][0], local[2][1], local[3][0], local[3][1]],
                        },
                        8,
                        0,
                    )
                }
                Overlay::Hover { uv, color } => (
                    Instance {
                        rect: [rect.x, rect.y, rect.w, rect.h],
                        uv: [0., 0., 1., 1.],
                        color: *color,
                        params: [uv[0], uv[1], 0., groups + 1.],
                    },
                    5,
                    self.outline_texture
                        .ok_or_else(|| anyhow::anyhow!("hover texture missing"))?,
                ),
                Overlay::Outline { index, color } => (
                    Instance {
                        rect: [rect.x, rect.y, rect.w, rect.h],
                        uv: [0., 0., 1., 1.],
                        color: *color,
                        params: [0., 0., *index as f32, groups],
                    },
                    5,
                    self.outline_texture
                        .ok_or_else(|| anyhow::anyhow!("outline texture missing"))?,
                ),
            };
            let texture = self
                .external
                .get(&texture)
                .copied()
                .or_else(|| self.textures.get(&texture).map(|t| t.slot))
                .ok_or_else(|| anyhow::anyhow!("overlay texture not uploaded"))?;
            self.draws.push(Draw {
                first: self.instances.len() as u32,
                count: 1,
                texture,
                mode,
                scissor,
                mask_offset: 0,
                mask_count: 0,
            });
            self.instances.push(instance);
        }
        Ok(())
    }
}
