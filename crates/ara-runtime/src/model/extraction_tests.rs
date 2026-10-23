use super::selection_tests::{imported, scene};
use crate::{
    animation::Library,
    assets::MemoryAssets,
    components::MeshRenderer,
    extract::{Geometry, RenderFrame},
};
use ara_schema::AssetRef;
use ara_sim::Identity;
use bevy_ecs::world::World;
use std::collections::BTreeMap;

#[test]
fn mesh_selection_and_emission_controls_reach_the_renderer_without_derived_models() {
    let assets = MemoryAssets(BTreeMap::from([("as_kit".into(), imported(scene()))]));
    let mut library = Library::default();
    library.models.load(&assets, "as_kit").expect("model");
    let mut world = World::new();
    world.insert_resource(library);
    let entity = world
        .spawn((
            Identity("e_lamp".into()),
            MeshRenderer {
                model: Some(AssetRef("@kit.gltf#as_kit".into())),
                node: Some("PieceA".into()),
                ..Default::default()
            },
        ))
        .id();
    let mut frame = RenderFrame::default();
    frame.extract(&world, 0, 0.);
    assert_eq!(frame.draws.len(), 2);
    assert_eq!(frame.draws[0].world.w_axis.to_array(), [-12., 0., -23., 1.]);
    assert_eq!(
        frame.draws[0].emissive, [1.; 3],
        "imported emission is enabled by default"
    );
    assert_eq!(frame.draws[0].emissive_factor, None);
    let second_identity = frame.draws[1].entity;
    {
        let mut renderer = world.get_mut::<MeshRenderer>(entity).expect("renderer");
        renderer.primitive = Some(1);
        renderer.emissive_factor = Some([10., 8., 6.]);
        renderer.emissive_materials = vec!["Layered".into()];
        renderer.emissive_strength = 2.;
    }
    frame.extract(&world, 0, 0.);
    assert_eq!(frame.draws.len(), 1);
    assert_eq!(
        frame.draws[0].entity, second_identity,
        "filtering preserves source primitive identity"
    );
    assert_eq!(frame.draws[0].emissive, [2.; 3]);
    assert_eq!(frame.draws[0].emissive_factor, Some([10., 8., 6.]));
    assert!(matches!(
        &frame.draws[0].geometry,
        Geometry::Model { primitive: 1, .. }
    ));
    {
        let mut renderer = world.get_mut::<MeshRenderer>(entity).expect("renderer");
        renderer.emissive_enabled = false;
    }
    frame.extract(&world, 0, 0.);
    assert_eq!(frame.draws[0].emissive, [0.; 3]);
    {
        let mut renderer = world.get_mut::<MeshRenderer>(entity).expect("renderer");
        renderer.emissive_enabled = true;
        renderer.emissive_materials = vec!["Other".into()];
        renderer.emissive = [2., 1., 0.5];
    }
    frame.extract(&world, 0, 0.);
    assert_eq!(frame.draws[0].emissive, [4., 2., 1.]);
    assert_eq!(
        frame.draws[0].emissive_factor, None,
        "a factor override targets only matching source materials"
    );
    world
        .get_mut::<MeshRenderer>(entity)
        .expect("renderer")
        .node = Some("Missing".into());
    frame.extract(&world, 0, 0.);
    assert_eq!(
        frame.draws.len(),
        1,
        "an invalid selector must not draw the entire kit"
    );
    assert!(matches!(frame.draws[0].geometry, Geometry::Shape { .. }));
    assert_eq!(frame.draws[0].color, [1., 0., 1., 1.]);
}

#[test]
fn procedural_emission_retains_radiance_semantics() {
    let mut world = World::new();
    let entity = world
        .spawn((
            Identity("e_sign".into()),
            MeshRenderer {
                emissive: [3., 2., 1.],
                emissive_strength: 2.,
                ..Default::default()
            },
        ))
        .id();
    let mut frame = RenderFrame::default();
    frame.extract(&world, 0, 0.);
    assert_eq!(frame.draws[0].emissive, [6., 4., 2.]);
    world
        .get_mut::<MeshRenderer>(entity)
        .expect("renderer")
        .emissive_enabled = false;
    frame.extract(&world, 0, 0.);
    assert_eq!(frame.draws[0].emissive, [0.; 3]);
}
