#[test]
fn checked_material_evaluator_matches_the_cooked_source() {
    let generated = ara_landscape::material::evaluator().expect("valid terrain material AST");
    assert_eq!(generated, include_str!("../shaders/terrain_material.slang"));
    assert!(generated.contains("ara_custom_splat"));
}
