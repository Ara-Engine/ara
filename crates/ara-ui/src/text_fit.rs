//! Feed final paragraph breaking back into intrinsic leaf height, without changing authored fixed sizes.
use crate::{Result, Ui};

impl Ui {
    pub(super) fn fit_auto_text(&mut self) -> Result<bool> {
        let mut changed = false;
        for index in 0..self.nodes.len() {
            let node = &self.nodes[index];
            if !self.shown(index)
                || !node.element.children.is_empty()
                || matches!(node.element.tag.as_str(), "field" | "input")
                || !node.computed.layout.size.height.is_auto()
            {
                continue;
            }
            let Some(text) = &node.text else {
                continue;
            };
            let layout = self.tree.layout(node.layout_id)?;
            let measured = text.layout.height()
                + layout.padding.top
                + layout.padding.bottom
                + layout.border.top
                + layout.border.bottom;
            let current = self.tree.style(node.layout_id)?;
            let minimum = current
                .min_size
                .height
                .resolve_to_option(self.size[1], |_, _| 0.0)
                .unwrap_or(0.0);
            let maximum = current
                .max_size
                .height
                .resolve_to_option(self.size[1], |_, _| 0.0)
                .unwrap_or(f32::MAX)
                .max(minimum);
            let height = measured.clamp(minimum, maximum);
            if (layout.size.height - height).abs() <= 0.5 {
                continue;
            }
            let mut style = current.clone();
            style.size.height = taffy::Dimension::length(height);
            // A flex parent may legitimately shrink a leaf. Do not retry the same
            // requested height forever when its authored constraints still win.
            if style == *current {
                continue;
            }
            self.tree.set_style(node.layout_id, style)?;
            self.nodes[index].dirty.layout = true;
            changed = true;
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_ui_text::TextSystem;

    #[test]
    fn fitted_auto_text_keeps_geometry_on_color_only_themes_and_idle() -> Result<()> {
        let mut text = TextSystem::bundled()?;
        let mut ui = Ui::editor(
            "<ui id='root'><panel id='constrained' style='height:8px;flex-shrink:0;overflow:hidden'><text id='copy' text='Retained auto text' style='height:auto;min-height:0px;flex-shrink:1' /></panel></ui>",
            crate::theme::EDITOR_CSS,
            [240., 100.],
        )?;
        ui.frame(&mut text)?;
        let copy = ui.find("copy").expect("auto text leaf");
        let height = ui.tree.style(ui.nodes[copy].layout_id)?.size.height;
        assert!(
            !height.is_auto(),
            "fixture must exercise an effective height fitted from authored auto"
        );
        assert!(ui.nodes[copy].computed.layout.size.height.is_auto());
        let geometry = |ui: &Ui| {
            ui.nodes
                .iter()
                .map(|node| {
                    [
                        node.rect.x,
                        node.rect.y,
                        node.rect.w,
                        node.rect.h,
                        node.clip.x,
                        node.clip.y,
                        node.clip.w,
                        node.clip.h,
                    ]
                })
                .collect::<Vec<_>>()
        };
        let before = geometry(&ui);
        for theme in [
            crate::theme::EditorTheme::Light,
            crate::theme::EditorTheme::Dark,
        ] {
            ui.switch_theme(theme)?;
            let work = ui.frame(&mut text)?;
            assert_eq!(work.restyled, ui.nodes.len(), "{work:?}");
            assert_eq!(
                work.shaped, 0,
                "color-only restyles retain shaping: {work:?}"
            );
            assert_eq!(
                work.layout_passes, 0,
                "color-only restyles retain fitted layout: {work:?}"
            );
            assert!(work.paint_nodes > 0);
            assert_eq!(geometry(&ui), before);
            assert_eq!(ui.tree.style(ui.nodes[copy].layout_id)?.size.height, height);
            assert_eq!(ui.frame(&mut text)?, crate::Work::default());
        }
        let paragraph_height = ui.nodes[copy]
            .text
            .as_ref()
            .expect("paragraph")
            .layout
            .height();
        ui.set_style(
            copy,
            "height:auto;min-height:0px;flex-shrink:1;font-size:26px",
        )?;
        let metrics = ui.frame(&mut text)?;
        assert_eq!(
            metrics.shaped, 1,
            "a genuine font change reshapes its paragraph"
        );
        assert!(metrics.layout_passes > 0);
        assert!(
            ui.nodes[copy]
                .text
                .as_ref()
                .expect("paragraph")
                .layout
                .height()
                > paragraph_height
        );
        assert_ne!(ui.tree.style(ui.nodes[copy].layout_id)?.size.height, height);
        assert_eq!(ui.frame(&mut text)?, crate::Work::default());
        Ok(())
    }

    #[test]
    fn final_wrapping_fits_auto_controls_preserves_fixed_height_and_settles_after_resize()
    -> Result<()> {
        let mut text = TextSystem::bundled()?;
        let copy = "A long label with localized éclairage and إعدادات العرض that must wrap at its final available content width.";
        let markup = format!(
            "<ui id='root'><panel id='form' style='position:absolute;left:8px;top:8px;max-width:300px'><dropdown id='auto' text='{copy}' style='height:auto;padding:6px;white-space:normal' /><button id='fixed' text='{copy}' style='width:180px;height:24px;padding:0px;white-space:normal' /></panel></ui>"
        );
        let mut ui = Ui::editor(&markup, crate::theme::EDITOR_CSS, [200., 600.])?;
        ui.frame(&mut text)?;
        let auto = ui.find("auto").expect("auto control");
        let fixed = ui.find("fixed").expect("fixed control");
        let fits = |ui: &Ui| -> Result<()> {
            let node = &ui.nodes[auto];
            let layout = ui.tree.layout(node.layout_id)?;
            let available = layout.size.height
                - layout.padding.top
                - layout.padding.bottom
                - layout.border.top
                - layout.border.bottom;
            assert!(node.text.as_ref().expect("paragraph").layout.height() <= available + 0.5);
            assert_eq!(ui.nodes[fixed].rect.h, 24., "authored height remains fixed");
            Ok(())
        };
        fits(&ui)?;
        let narrow = ui.nodes[auto].rect.h;
        assert_eq!(ui.frame(&mut text)?.layout_passes, 0);
        ui.resize([500., 600.])?;
        ui.frame(&mut text)?;
        fits(&ui)?;
        assert!(
            ui.nodes[auto].rect.h < narrow,
            "auto height follows reduced wrapping"
        );
        assert_eq!(ui.frame(&mut text)?.layout_passes, 0);
        Ok(())
    }
}
