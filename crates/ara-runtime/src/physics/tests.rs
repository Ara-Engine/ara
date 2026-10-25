#![allow(clippy::unwrap_used)]
use super::*;
use crate::{
    animation::Library,
    assets::MemoryAssets,
    model::selection_tests::{imported, scene},
};
use ara_components::Transform;
use ara_physics::{Collider, ColliderKind, MotionType, PhysicsId, PhysicsWorld, Query, QueryHit};
use ara_schema::AssetRef;
use ara_sim::Identity;
use std::collections::BTreeMap;

fn hit(world: &World, origin: [f32; 3]) -> Option<QueryHit> {
    world
        .resource::<PhysicsWorld<JoltBackend>>()
        .query_first(Query::Ray {
            origin,
            displacement: [0., -10., 0.],
            filter: Default::default(),
        })
        .unwrap()
}

#[test]
fn center_moves_the_rotated_shape_without_moving_the_body_origin() {
    let mut world = World::new();
    let transform = Transform {
        position: [10., 0., 0.],
        rotation: [0., 90., 0.],
        ..Default::default()
    };
    let entity = world
        .spawn((
            Identity("e_offset".into()),
            transform,
            RigidBody::default(),
            Collider {
                center: [2., 0., 0.],
                ..Default::default()
            },
        ))
        .id();
    install(
        &mut world,
        &PhysicsConfig::default(),
        &[(entity, Pose::from_transform(&transform))],
    )
    .unwrap();
    assert!(hit(&world, [10., 3., 0.]).is_none());
    let surface = hit(&world, [10., 3., -2.]).unwrap();
    assert!((surface.point[1] - 0.5).abs() < 0.01);
    assert_eq!(
        world.get::<PhysicsPose>(entity).unwrap().position,
        [10., 0., 0.]
    );
    let parent = world
        .spawn(Transform {
            position: [30., 0., 0.],
            ..Default::default()
        })
        .id();
    let child = world
        .spawn((
            Identity("e_child".into()),
            Transform {
                position: [2., 0., 0.],
                ..Default::default()
            },
            ChildOf(parent),
            RigidBody::default(),
            Collider {
                center: [1., 0., 0.],
                ..Default::default()
            },
        ))
        .id();
    adopt(&mut world);
    step(&mut world);
    assert_eq!(
        world.get::<PhysicsPose>(child).unwrap().position,
        [32., 0., 0.]
    );
    assert!(hit(&world, [33., 3., 0.]).is_some());
}

#[test]
fn automatic_ids_reserve_explicit_values_and_survive_insertion_order_and_adoption() {
    fn actors(reverse: bool) -> BTreeMap<String, u64> {
        let mut world = World::new();
        let names = if reverse {
            ["e_b", "e_a"]
        } else {
            ["e_a", "e_b"]
        };
        let mut poses = Vec::new();
        for (index, name) in names.into_iter().enumerate() {
            let entity = world
                .spawn((
                    Identity(name.into()),
                    Transform::default(),
                    RigidBody::default(),
                    Collider::default(),
                ))
                .id();
            if index == 0 {
                world.entity_mut(entity).insert(PhysicsId::default());
            }
            poses.push((entity, Pose::IDENTITY));
        }
        let reserved = world
            .spawn((
                Identity("e_reserved".into()),
                PhysicsId { value: 1 },
                RigidBody::default(),
                Collider::default(),
            ))
            .id();
        poses.push((reserved, Pose::IDENTITY));
        install(&mut world, &PhysicsConfig::default(), &poses).unwrap();
        let new = world
            .spawn((
                Identity("e_new".into()),
                Transform::default(),
                RigidBody::default(),
                Collider::default(),
            ))
            .id();
        adopt(&mut world);
        step(&mut world);
        assert!(world.resource::<PhysicsStatus>().error.is_none());
        assert!(world.get::<PhysicsId>(new).is_some());
        let ids: BTreeMap<_, _> = world
            .query::<(&Identity, &PhysicsId)>()
            .iter(&world)
            .map(|(name, id)| (name.0.clone(), id.value))
            .collect();
        assert_eq!(
            ids.values()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            4
        );
        assert_eq!(ids["e_reserved"], 1);
        assert!(ids.values().all(|&id| id > 0));
        ids
    }
    assert_eq!(actors(false), actors(true));
}

fn model_world(
    kind: ColliderKind,
    node: &str,
    scale: [f32; 3],
    center: [f32; 3],
    motion: MotionType,
) -> (World, Entity) {
    let source = MemoryAssets([("as_kit".into(), imported(scene()))].into());
    let mut library = Library::default();
    library.models.load(&source, "as_kit").unwrap();
    let mut world = World::new();
    world.insert_resource(library);
    let transform = Transform {
        position: [5., 0., 0.],
        scale,
        ..Default::default()
    };
    let entity = world
        .spawn((
            Identity("e_model".into()),
            transform,
            RigidBody {
                motion,
                ..Default::default()
            },
            Collider {
                kind,
                asset: Some(AssetRef("@kit.gltf#as_kit".into())),
                node: Some(node.into()),
                center,
                ..Default::default()
            },
        ))
        .id();
    install(
        &mut world,
        &PhysicsConfig::default(),
        &[(entity, Pose::from_transform(&transform))],
    )
    .unwrap();
    (world, entity)
}

#[test]
fn cooked_model_hull_and_mesh_use_selected_pivots_scale_and_center() {
    let (mut convex, entity) = model_world(
        ColliderKind::ModelConvex,
        "PieceA",
        [1.; 3],
        [1., 0., 0.],
        MotionType::Dynamic,
    );
    assert!(hit(&convex, [5.5, 4., -1.]).is_none());
    assert!((hit(&convex, [7., 4., -1.]).unwrap().point[1] - 2.).abs() < 0.03);
    let cooked = *convex.get::<CookedCollider>(entity).unwrap();
    convex.get_mut::<Transform>(entity).unwrap().rotation = [13., 27., 19.];
    convex.get_mut::<Collider>(entity).unwrap().friction = 0.3;
    adopt(&mut convex);
    assert_eq!(
        *convex.get::<CookedCollider>(entity).unwrap(),
        cooked,
        "rotation and material edits reuse cooked geometry"
    );
    step(&mut convex);
    assert!(convex.get::<PhysicsPose>(entity).unwrap().position[1] < 0.);
    convex.get_mut::<Transform>(entity).unwrap().scale = [2., 1., 1.];
    adopt(&mut convex);
    assert_ne!(
        *convex.get::<CookedCollider>(entity).unwrap(),
        cooked,
        "scale edits rebuild geometry"
    );
    let (mut mesh, _) = model_world(
        ColliderKind::ModelMesh,
        "Bolt",
        [-1., 1., 2.],
        [0., 0.5, 0.],
        MotionType::Static,
    );
    let before = hit(&mesh, [4., 3., -2.]).unwrap();
    assert!((before.point[1] - 0.5).abs() < 0.03);
    teardown(&mut mesh);
    build(&mut mesh, &PhysicsConfig::default()).unwrap();
    let restored = hit(&mesh, [4., 3., -2.]).unwrap();
    assert_eq!(before.user_data, restored.user_data);
    assert!((restored.point[1] - 0.5).abs() < 0.03);
}

#[test]
fn typed_model_collider_command_is_undoable_and_plays_without_a_mesh_renderer() {
    use ara_cmd::{Batch, Registry, Session};
    use ara_doc::{AssetId, Document, Workspace};
    use ara_ops::{Author, Store};
    use serde_json::json;
    let mut schema = crate::schema(&[]).unwrap();
    let mut registry = Registry::builtin(&mut schema);
    commands::register(&mut registry, &mut schema);
    let mut index = ara_doc::refs::AssetIndex::new();
    index.insert("as_kit", "kit.gltf", Some("import".into()));
    let mut workspace = Workspace::new(index);
    workspace.insert(
        "test.scene",
        Document::new("scene", AssetId::parse("sc_test").unwrap()),
    );
    let mut store = Store::new(schema.clone(), workspace);
    registry.apply(&mut store,&Batch::new("sc_test",[("entity.create",json!({"id":"e_model","components":{"Transform":{},"MeshRenderer":{"model":"@kit.gltf#as_kit","node":"PieceA"}}}))]),&Session::human()).unwrap();
    let before = ara_doc::write(store.workspace().doc("sc_test").unwrap(), &schema);
    let invalid=registry.apply(&mut store,&Batch::new("sc_test",[
        ("component.add",json!({"entity":"e_model","component":"RigidBody","value":{"motion":"dynamic"}})),
        ("physics.collider.fromModel",json!({"entity":"e_model","mode":"mesh"}))
    ]),&Session::human()).unwrap_err();
    assert!(
        invalid
            .iter()
            .any(|error| error.message.contains("static RigidBody"))
    );
    assert_eq!(
        before,
        ara_doc::write(store.workspace().doc("sc_test").unwrap(), &schema)
    );
    registry
        .apply(
            &mut store,
            &Batch::new(
                "sc_test",
                [
                    ("physics.collider.fromModel", json!({"entity":"e_model"})),
                    (
                        "component.remove",
                        json!({"entity":"e_model","component":"MeshRenderer"}),
                    ),
                ],
            ),
            &Session::human(),
        )
        .unwrap();
    let source = MemoryAssets([("as_kit".into(), imported(scene()))].into());
    let mut runtime = crate::Runtime::new(
        schema.clone(),
        store.workspace().clone(),
        AssetId::parse("sc_test").unwrap(),
        Box::new(MemoryAssets::default()),
        crate::RuntimeConfig::default(),
        vec![],
    )
    .unwrap();
    assert!(runtime.asset_diagnostics().any(|diagnostic| {
        diagnostic.code == crate::ASSET_PLACEHOLDER
            && diagnostic.message.contains("e_model")
            && diagnostic.message.contains("as_kit")
            && diagnostic.message.contains("box bounds")
    }));
    let cooked = {
        let world = runtime.play.session_mut().world_mut();
        *world.query::<&CookedCollider>().single(world).unwrap()
    };
    assert!((hit(runtime.world(), [0., 4., 0.]).unwrap().point[1] - 0.5).abs() < 0.03);
    runtime.step(2, &[]).unwrap();
    {
        let world = runtime.play.session_mut().world_mut();
        assert_eq!(
            *world.query::<&CookedCollider>().single(world).unwrap(),
            cooked
        );
    }
    let loaded = runtime.reload_assets(Box::new(source)).unwrap();
    assert!(
        loaded
            .iter()
            .any(|(id, result)| id == "as_kit" && matches!(result, Ok(true)))
    );
    runtime.step(1, &[]).unwrap();
    {
        let world = runtime.play.session_mut().world_mut();
        assert_ne!(
            *world.query::<&CookedCollider>().single(world).unwrap(),
            cooked
        );
    }
    assert!((hit(runtime.world(), [1., 4., -1.]).unwrap().point[1] - 2.).abs() < 0.03);
    assert!(
        runtime
            .changes()
            .unwrap()
            .iter()
            .filter(|change| change.path.starts_with("PhysicsId"))
            .all(|change| change.driven)
    );
    let identity = hit(runtime.world(), [1., 4., -1.]).unwrap().user_data;
    let snapshot = runtime.play().snapshot().unwrap().to_bytes().unwrap();
    runtime.step(2, &[]).unwrap();
    runtime
        .restore(&ara_sim::Snapshot::from_bytes(&snapshot).unwrap())
        .unwrap();
    assert_eq!(
        identity,
        hit(runtime.world(), [1., 4., -1.]).unwrap().user_data
    );
    store.undo_last(&Author::Human).unwrap();
    assert_eq!(
        before,
        ara_doc::write(store.workspace().doc("sc_test").unwrap(), &schema)
    );
}
