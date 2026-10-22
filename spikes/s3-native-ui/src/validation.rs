//! Behavioral checks shared by the executable and the focused integration test.
use crate::{runtime::Ui, source::Result, text::TextSystem};
pub fn diagnostics() -> Result<serde_json::Value> {
    let mut evidence = Vec::new();
    for source in [
        "\nbutton > text { gap: 4px; }",
        "\nbutton:visited { color:#112233; }",
        "\n.unused { position:fixed; }",
        "text { color:var(--missing); }",
    ] {
        let error = crate::style::Sheet::parse(source)
            .err()
            .ok_or("unsupported CSS was silently accepted")?;
        evidence.push(serde_json::json!({"source":source,"diagnostic":error.to_string()}));
    }
    Ok(serde_json::json!(evidence))
}
pub fn text_coverage(ui: &Ui) -> Result<serde_json::Value> {
    let mut evidence = serde_json::Map::new();
    for id in ["arabic", "mixed", "cjk", "combining"] {
        let text = ui.nodes[ui.index(id)]
            .text
            .as_ref()
            .ok_or("missing text fixture")?;
        let item = crate::text::inspect(text);
        if item.missing_glyphs != 0 {
            return Err(format!("TEXT001: {id} has {} missing glyphs", item.missing_glyphs).into());
        }
        match id {
            "arabic" if item.rtl_runs == 0 || item.lines < 2 => {
                return Err("TEXT002: Arabic did not resolve RTL and wrap".into());
            }
            "mixed" if item.rtl_runs == 0 || item.ltr_runs == 0 => {
                return Err("TEXT003: mixed bidi did not produce both run directions".into());
            }
            "cjk" if item.lines < 2 => return Err("TEXT004: CJK did not wrap".into()),
            "combining" => {
                for sequence in ["e\u{301}", "a\u{308}", "n\u{303}", "👩🏽‍🚀", "👨‍👩‍👧‍👦", "🏳️‍🌈"]
                {
                    let start = text
                        .value
                        .find(sequence)
                        .ok_or("coverage fixture lost a sequence")?;
                    if !item
                        .clusters
                        .iter()
                        .any(|c| c.bytes.start == start && c.bytes.end == start + sequence.len())
                    {
                        return Err(
                            format!("TEXT005: grapheme sequence {sequence} was split").into()
                        );
                    }
                }
            }
            _ => {}
        }
        evidence.insert(id.into(), serde_json::to_value(item)?);
    }
    Ok(serde_json::Value::Object(evidence))
}
pub fn clipping_probe(text: &mut TextSystem, out: &std::path::Path) -> Result<serde_json::Value> {
    let markup = "<panel id='probe' style='width: 360px; height: 180px; padding: 20px; background: #17212f; overflow: hidden'><text id='clip-title' text='Clip + transform probe' /><panel id='clip' style='height: 48px; width: 180px; overflow: hidden; background: #243448; translate-x: 30px'><button id='clipped-button' text='A deliberately overflowing control' style='width: 300px; height: 100px' /></panel><text id='small-text' text='Deliberately clipped text' style='height: 8px; overflow: hidden' /></panel>";
    let mut ui = Ui::parse(markup, "", [360.0, 180.0])?;
    ui.frame(text)?;
    let child = ui.index("clipped-button");
    let r = ui.nodes[child].rect;
    if ui.hit_test(r.x + 20.0, r.y + 20.0) != Some(child)
        || ui.hit_test(r.x + 210.0, r.y + 20.0).is_some()
    {
        return Err("INPUT002: hit testing ignored a translated clip".into());
    }
    let inspection = ui.inspect();
    if !inspection["nodes"]
        .as_array()
        .ok_or("inspection is not a node array")?
        .iter()
        .any(|n| {
            n["warnings"]
                .as_array()
                .is_some_and(|a| a.iter().any(|w| w == "clipped-text-height"))
        })
    {
        return Err("INSPECT001: clipped text was not reported".into());
    }
    ui.display
        .render(&text.assets, 360, 180, &out.join("clip-transform.png"))?;
    Ok(inspection)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_layout_shapes_unicode_and_separates_paint_from_layout() -> Result<()> {
        let mut text = TextSystem::new()?;
        let mut lab = Ui::parse(
            include_str!("../assets/text.ui"),
            crate::screens::CSS,
            [1000.0, 1100.0],
        )?;
        lab.frame(&mut text)?;
        text_coverage(&lab)?;
        let mut ui = Ui::parse(
            include_str!("../assets/pause.ui"),
            crate::screens::CSS,
            [600.0, 760.0],
        )?;
        ui.frame(&mut text)?;
        let index = ui.index("resume");
        ui.set_state(
            index,
            crate::style::State {
                hover: true,
                ..Default::default()
            },
        );
        let work = ui.frame(&mut text)?;
        assert_eq!(work.restyled, 1);
        assert_eq!(work.layout_passes, 0);
        assert_eq!(work.shaped, 0);
        assert_eq!(
            crate::input::next_tab(&ui, index, false),
            Some(ui.index("settings"))
        );
        let a11y = crate::screens::accessibility(&ui, "resume");
        assert_eq!(a11y.nodes.len(), ui.nodes.len());
        let mut inventory = crate::screens::Inventory::new()?;
        inventory.scroll_to(0)?;
        inventory.ui.frame(&mut text)?;
        let retained = inventory.ui.index("item-1");
        inventory.scroll_to(1)?;
        let work = inventory.ui.frame(&mut text)?;
        assert_eq!(inventory.ui.index("item-1"), retained);
        assert_eq!(work.shaped, 1);
        assert_eq!(
            crate::input::next_tab(&inventory.ui, retained, false),
            Some(inventory.row_indices[1])
        );
        Ok(())
    }
}
