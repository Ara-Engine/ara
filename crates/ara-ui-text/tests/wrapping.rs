//! Emergency wrapping preserves original text and uses the existing shaping cache.
use ara_ui_text::{Result, Text, TextSystem, inspect};
use std::collections::BTreeSet;
use unicode_segmentation::UnicodeSegmentation;

#[test]
fn overlong_profile_word_wraps_inside_seventy_four_pixels_without_changing_its_value() -> Result<()>
{
    let mut system = TextSystem::bundled()?;
    let mut text = Text::new("configuration".into());
    let natural = system.measure(&mut text, None, 12.);
    assert!(
        natural.width > 74.,
        "fixture must exceed the actual Build Select content width"
    );
    let shapes = system.shapes;
    let wrapped = system.measure(&mut text, Some(74.), 12.);
    assert!(
        wrapped.width <= 74.5,
        "long profile word overflows: {}",
        wrapped.width
    );
    assert!(text.layout.len() > 1);
    assert_eq!(text.value, "configuration");
    assert_eq!(system.shapes, shapes);
    let unwrapped = system.measure(&mut text, None, 12.);
    assert_eq!(
        text.layout.len(),
        1,
        "nowrap remains an unconstrained single line"
    );
    assert_eq!(unwrapped.width, natural.width);
    assert_eq!(system.shapes, shapes);
    Ok(())
}

#[test]
fn normal_word_boundaries_are_preferred_to_emergency_breaks() -> Result<()> {
    let mut system = TextSystem::bundled()?;
    let mut alpha = Text::new("alpha".into());
    let mut beta = Text::new("beta".into());
    let width = system
        .measure(&mut alpha, None, 12.)
        .width
        .max(system.measure(&mut beta, None, 12.).width)
        + 1.;
    let mut text = Text::new("alpha beta".into());
    system.measure(&mut text, Some(width), 12.);
    let words: Vec<_> = text
        .layout
        .lines()
        .map(|line| text.value[line.text_range()].trim())
        .collect();
    assert_eq!(words, ["alpha", "beta"]);
    Ok(())
}

#[test]
fn filenames_combining_marks_and_arabic_reflow_only_at_grapheme_boundaries_and_reuse_shapes()
-> Result<()> {
    let mut system = TextSystem::bundled()?;
    for source in [
        "WindowsShippingConfigurationWithSymbols.export.json".to_owned(),
        "e\u{301}".repeat(30),
        "إعداداتالبناءوالتوزيعوالمعاينة".to_owned(),
    ] {
        let mut text = Text::new(source.clone());
        let mut boundaries: BTreeSet<_> = source
            .grapheme_indices(true)
            .map(|(byte, _)| byte)
            .collect();
        boundaries.insert(source.len());
        system.measure(&mut text, None, 12.);
        let shapes = system.shapes;
        for width in [Some(74.), Some(42.), Some(120.), None, Some(74.)] {
            let measured = system.measure(&mut text, width, 12.);
            if let Some(width) = width {
                assert!(
                    measured.width <= width + 0.5,
                    "{source}: {} > {width}",
                    measured.width
                );
            }
            for line in text.layout.lines() {
                let range = line.text_range();
                assert!(
                    boundaries.contains(&range.start) && boundaries.contains(&range.end),
                    "line splits a grapheme: {source}, {range:?}"
                );
            }
            assert_eq!(text.value, source);
            assert_eq!(
                system.shapes, shapes,
                "width changes must only rebreak lines"
            );
            assert_eq!(
                inspect(&text).missing_glyphs,
                0,
                "explicit bundled font coverage"
            );
        }
        // A recycled row with the same caption uses the same cached unbroken paragraph.
        let mut remounted = Text::new(source);
        system.measure(&mut remounted, Some(74.), 12.);
        assert_eq!(system.shapes, shapes);
    }
    Ok(())
}
