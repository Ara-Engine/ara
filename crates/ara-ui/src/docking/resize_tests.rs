//! Window-relative resizing keeps persistence and failure atomic across all dock trees.
use super::*;

fn area(id: &str, panel: &str) -> DockNode {
    DockNode::Area {
        id: id.into(),
        tabs: vec![panel.into()],
        active: 0,
    }
}
fn split(ratio: f32, first: DockNode, second: DockNode) -> DockNode {
    DockNode::Split {
        axis: Axis::Vertical,
        ratio,
        first: Box::new(first),
        second: Box::new(second),
    }
}
fn fixture() -> Result<DockLayout> {
    DockLayout::with_floating(
        split(
            0.3,
            area("main-left", "hierarchy"),
            area("main-view", "scene"),
        ),
        vec![
            Floating {
                id: "w1".into(),
                rect: [100, 100, 600, 500],
                root: split(
                    0.6,
                    area("f-inspector", "inspector"),
                    split(
                        0.4,
                        area("f-console", "console"),
                        area("f-assets", "assets"),
                    ),
                ),
            },
            Floating {
                id: "w2".into(),
                rect: [200, 200, 500, 400],
                root: split(0.2, area("f-material", "material"), area("f-vfx", "vfx")),
            },
        ],
    )
}

#[test]
fn floating_nested_ratio_round_trips_without_changing_main_or_another_window() -> Result<()> {
    let mut dock = fixture()?;
    let main = dock.root().clone();
    let other = dock.floating()[1].clone();
    dock.resize_split_in(Some("w1"), &[true], 0.75)?;
    assert_eq!(dock.root(), &main);
    assert_eq!(dock.floating()[1], other);
    assert!(
        matches!(&dock.floating()[0].root,DockNode::Split {ratio,second,..}
        if *ratio==0.6 && matches!(second.as_ref(),DockNode::Split {ratio,..} if *ratio==0.75))
    );
    let saved = dock.serialize()?;
    let mut restored = DockLayout::new(area("reset", "reset-panel"))?;
    restored.restore(&saved)?;
    assert_eq!(restored.root(), dock.root());
    assert_eq!(restored.floating(), dock.floating());
    assert_eq!(restored.serialize()?, saved);
    Ok(())
}

#[test]
fn invalid_window_paths_and_nonfinite_values_never_change_any_serialized_tree() -> Result<()> {
    let mut dock = fixture()?;
    let before = dock.serialize()?;
    for (window, path, ratio) in [
        (Some("missing"), vec![], 0.5),
        (Some(""), vec![], 0.5),
        (Some("w1"), vec![false], 0.5),
        (Some("w1"), vec![true, false, true], 0.5),
        (None, vec![true], 0.5),
        (Some("w1"), vec![], f32::NAN),
        (Some("w1"), vec![true], f32::INFINITY),
        (None, vec![], f32::NEG_INFINITY),
    ] {
        assert!(
            dock.resize_split_in(window, &path, ratio).is_err(),
            "{window:?}/{path:?}/{ratio}"
        );
        assert_eq!(
            dock.serialize()?,
            before,
            "Rejected resize must preserve every window"
        );
    }
    Ok(())
}

#[test]
fn existing_main_resize_api_and_none_window_keep_the_same_clamping_contract() -> Result<()> {
    let mut legacy = fixture()?;
    let mut explicit = fixture()?;
    let floating = legacy.floating().to_vec();
    for (input, expected) in [(-5., 0.1), (0.45, 0.45), (5., 0.9)] {
        legacy.resize_split(&[], input)?;
        explicit.resize_split_in(None, &[], input)?;
        assert_eq!(legacy.serialize()?, explicit.serialize()?);
        assert!(matches!(legacy.root(),DockNode::Split {ratio,..} if *ratio==expected));
        assert_eq!(legacy.floating(), floating.as_slice());
    }
    Ok(())
}
