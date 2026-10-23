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
                Overlay::Hover { uv, color } => (
                    Instance {
                        rect: [rect.x, rect.y, rect.w, rect.h],
                        uv: [0., 0., 1., 1.],
                        color: *color,
                        params: [uv[0], uv[1], 0., 1.],
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
                        params: [0., 0., *index as f32, 0.],
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
