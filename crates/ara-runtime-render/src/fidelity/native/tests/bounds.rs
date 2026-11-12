use super::*;

#[test]
fn model_bounds_style_never_marks_authored_shapes_with_the_same_colour() {
    let mut frame = RenderFrame::default();
    frame.draws = vec![draw(1), draw(2)];
    for draw in &mut frame.draws {
        draw.color = crate::PENDING_MODEL_COLOR;
    }
    let prepared = prepare(
        &frame,
        Content {
            unpublished_models: BTreeMap::from([("e_1".into(), "as_pending".into())]),
            ..Default::default()
        },
        config(),
    )
    .unwrap();
    let flags = |i: usize| {
        prepared.scene.materials[prepared.scene.instances[i].material]
            .pbr
            .flags[1]
    };
    assert_ne!(flags(0) & ara_frame::scene::MODEL_BOUNDS, 0);
    assert_eq!(flags(1) & ara_frame::scene::MODEL_BOUNDS, 0);
}
