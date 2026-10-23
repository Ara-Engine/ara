//! Typed native property values, intentionally separate from cascade evaluation.
use super::{Color, Computed};
use crate::Result;
use taffy::prelude::*;
fn scalar(value: &str) -> Result<f32> {
    let n: f32 = value.strip_suffix("px").unwrap_or(value).parse()?;
    if !n.is_finite() || n < 0.0 {
        return Err("length must be finite and nonnegative".into());
    }
    Ok(n)
}
fn dimension(v: &str) -> Result<Dimension> {
    Ok(if v == "auto" {
        Dimension::auto()
    } else if let Some(p) = v.strip_suffix('%') {
        Dimension::percent(scalar(p)? / 100.0)
    } else {
        Dimension::length(scalar(v)?)
    })
}
fn limit(v: &str) -> Result<LengthPercentageAuto> {
    Ok(if v == "auto" {
        LengthPercentageAuto::auto()
    } else if let Some(p) = v.strip_suffix('%') {
        LengthPercentageAuto::percent(scalar(p)? / 100.0)
    } else {
        LengthPercentageAuto::length(scalar(v)?)
    })
}
pub(super) fn declarations(body: &str) -> Result<Vec<(String, String)>> {
    Ok(ara_ui_document::stylesheet::declarations(body, 0)?
        .into_iter()
        .map(|d| (d.name, d.value))
        .collect())
}
pub(super) fn apply(c: &mut Computed, k: &str, v: &str) -> Result<()> {
    match k {
        "background" => c.background = Color::parse(v)?,
        "color" => c.color = Color::parse(v)?,
        "border-color" => c.border_color = Color::parse(v)?,
        "font-size" => c.font_size = scalar(v)?,
        "border-radius" => c.radius = scalar(v)?,
        "border-width" => {
            c.border = scalar(v)?;
            c.layout.border = Rect::length(c.border);
        }
        "min-width" => c.layout.min_size.width = limit(v)?,
        "min-height" => c.layout.min_size.height = limit(v)?,
        "max-width" => c.layout.max_size.width = limit(v)?,
        "max-height" => c.layout.max_size.height = limit(v)?,
        "flex-shrink" => c.layout.flex_shrink = scalar(v)?,
        "position" => {
            c.layout.position = match v {
                "relative" => Position::Relative,
                "absolute" => Position::Absolute,
                _ => return Err("ARA7011: position must be relative or absolute".into()),
            }
        }
        "left" => c.layout.inset.left = LengthPercentageAuto::length(scalar(v)?),
        "right" => c.layout.inset.right = LengthPercentageAuto::length(scalar(v)?),
        "top" => c.layout.inset.top = LengthPercentageAuto::length(scalar(v)?),
        "bottom" => c.layout.inset.bottom = LengthPercentageAuto::length(scalar(v)?),
        "align-items" => {
            c.layout.align_items = match v {
                "start" => AlignItems::START,
                "end" => AlignItems::END,
                "center" => AlignItems::CENTER,
                "stretch" => AlignItems::STRETCH,
                _ => return Err("ARA7011: unsupported alignment".into()),
            }
        }
        "justify-content" => {
            c.layout.justify_content = match v {
                "start" => JustifyContent::START,
                "end" => JustifyContent::END,
                "center" => JustifyContent::CENTER,
                "space-between" => JustifyContent::SPACE_BETWEEN,
                _ => return Err("ARA7011: unsupported justification".into()),
            }
        }
        "width" => c.layout.size.width = dimension(v)?,
        "height" => c.layout.size.height = dimension(v)?,
        "padding-left" => c.layout.padding.left = LengthPercentage::length(scalar(v)?),
        "padding" => c.layout.padding = Rect::length(scalar(v)?),
        "gap" => c.layout.gap = Size::length(scalar(v)?),
        "flex-grow" => c.layout.flex_grow = scalar(v)?,
        "display" => {
            c.layout.display = match v {
                "flex" => Display::Flex,
                "grid" => Display::Grid,
                _ => return Err("display must be flex or grid".into()),
            }
        }
        "flex-direction" => {
            c.layout.flex_direction = match v {
                "row" => FlexDirection::Row,
                "column" => FlexDirection::Column,
                _ => return Err("flex-direction must be row or column".into()),
            }
        }
        "grid-template-columns" => {
            c.layout.grid_template_columns = v
                .split_whitespace()
                .map(|s| {
                    s.strip_suffix("fr")
                        .ok_or_else(|| "grid tracks must use fr".into())
                        .and_then(|n| Ok(GridTemplateComponent::Single(fr(scalar(n)?))))
                })
                .collect::<Result<_>>()?;
            if c.layout.grid_template_columns.is_empty() {
                return Err("grid needs tracks".into());
            }
        }
        "overflow" => {
            c.clip = match v {
                "hidden" => true,
                "visible" => false,
                _ => return Err("overflow must be hidden or visible".into()),
            };
        }
        "translate-x" | "translate-y" => {
            let value: f32 = v.strip_suffix("px").unwrap_or(v).parse()?;
            if !value.is_finite() {
                return Err("translation must be finite".into());
            }
            c.translate[usize::from(k == "translate-y")] = value;
        }
        _ => return Err(format!("unknown property {k}").into()),
    }
    Ok(())
}
