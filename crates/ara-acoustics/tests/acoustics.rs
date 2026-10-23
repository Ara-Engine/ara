#![allow(clippy::unwrap_used)] // Tests may panic on unexpected failures.
//! Geometry, occlusion, rooms, propagation, baking and cell data.

use ara_acoustics::{
    occlusion::{OcclusionQuery, OcclusionSolver},
    propagation::curve_gain,
    *,
};
use ara_audio::{AudioListener, AudioSource};

fn concrete() -> AcousticMaterial {
    MaterialPreset::Concrete.material()
}

/// Two 10×3×10 m rooms side by side along X, sharing a wall at x = 0 with a door.
fn two_rooms() -> MeshScene {
    let mut scene = MeshScene::new();
    scene
        .add_box(
            [-0.1, 0.0, -5.0],
            [0.1, 3.0, -0.5],
            1,
            "wall north",
            concrete(),
        )
        .unwrap();
    scene
        .add_box(
            [-0.1, 0.0, 0.5],
            [0.1, 3.0, 5.0],
            2,
            "wall south",
            concrete(),
        )
        .unwrap();
    scene.commit();
    scene
}

#[test]
fn schemas_register_with_documented_defaults() {
    let mut db = ara_schema::SchemaDb::new();
    register(&mut db);
    assert!(db.verify().is_empty(), "{:?}", db.verify());
    for (name, value) in [
        (
            "AcousticSource",
            ara_json::to_value(&AcousticSource::default()).unwrap(),
        ),
        (
            "ReverbZone",
            ara_json::to_value(&ReverbZone::default()).unwrap(),
        ),
        (
            "AcousticPortal",
            ara_json::to_value(&AcousticPortal::default()).unwrap(),
        ),
        (
            "AcousticsSettings",
            ara_json::to_value(&AcousticsSettings::default()).unwrap(),
        ),
        (
            "AcousticMaterial",
            ara_json::to_value(&AcousticMaterial::default()).unwrap(),
        ),
        (
            "AcousticSurface",
            ara_json::to_value(&AcousticSurface::default()).unwrap(),
        ),
    ] {
        let desc = db.get(name).unwrap();
        for field in desc.fields() {
            assert!(!field.doc.is_empty(), "{name}.{}", field.name);
            assert!(
                ara_json::json_eq(
                    &db.canonical_value(&field.ty, &value[&field.name]),
                    &db.canonical_value(&field.ty, &db.field_default(field))
                ),
                "{name}.{}",
                field.name
            );
        }
    }
}

#[test]
fn bvh_matches_brute_force_closest_hits() {
    let mut scene = MeshScene::new();
    for i in 0..40 {
        let x = (i % 8) as f32 * 2.0 - 8.0;
        let z = (i / 8) as f32 * 2.0 - 5.0;
        scene
            .add_box(
                [x, 0.0, z],
                [x + 0.7, 1.0 + i as f32 * 0.05, z + 0.9],
                i,
                "crate",
                concrete(),
            )
            .unwrap();
    }
    let brute = scene.clone();
    scene.commit();
    for i in 0..200 {
        let origin = [-10.0 + (i % 20) as f32, 0.5, -8.0 + (i / 20) as f32 * 0.3];
        let t = i as f32 * 0.37;
        let direction = [t.cos(), 0.05 * (t * 3.0).sin(), t.sin()];
        let len = (direction.iter().map(|v| v * v).sum::<f32>()).sqrt();
        let direction = direction.map(|v| v / len);
        assert_eq!(
            scene.closest(origin, direction, 50.0),
            brute.closest(origin, direction, 50.0)
        );
    }
}

#[test]
fn occlusion_counts_each_solid_once_and_samples_partial_visibility() {
    let mut scene = two_rooms();
    let mut solver = OcclusionSolver::new();
    let blocked = solver
        .solve(
            &mut scene,
            OcclusionQuery {
                source: [3.0, 1.5, -3.0],
                listener: [-3.0, 1.5, -3.0],
                radius: 0.0,
                rays: 1,
            },
        )
        .unwrap();
    assert_eq!(blocked.visibility, 0.0);
    // Entering and leaving the 20 cm wall is one concrete crossing.
    assert_eq!(blocked.transmission, concrete().transmission);
    assert_eq!(blocked.blockers.len(), 1);
    assert!((blocked.blockers[0].distance - 2.9).abs() < 1e-3);
    let clear = solver
        .solve(
            &mut scene,
            OcclusionQuery {
                source: [3.0, 1.5, 0.0],
                listener: [-3.0, 1.5, 0.0],
                radius: 0.0,
                rays: 1,
            },
        )
        .unwrap();
    assert_eq!(clear.visibility, 1.0);
    // Near the door edge some of the volumetric rays clip the wall.
    let partial = solver
        .solve(
            &mut scene,
            OcclusionQuery {
                source: [3.0, 1.5, -0.6],
                listener: [-3.0, 1.5, -0.6],
                radius: 0.5,
                rays: 17,
            },
        )
        .unwrap();
    assert!(
        partial.visibility > 0.0 && partial.visibility < 1.0,
        "{partial:?}"
    );
}

#[test]
fn propagation_air_doppler_and_curves() {
    let settings = AcousticsSettings::default();
    let air = settings.air(100.0);
    assert!((air[2] - (-1.82_f32).exp()).abs() < 1e-4);
    assert!(air[0] > air[1] && air[1] > air[2]);
    let approaching = settings.doppler(
        1.0,
        ([10.0, 0.0, 0.0], [-34.3, 0.0, 0.0]),
        ([0.0; 3], [0.0; 3]),
    );
    assert!((approaching - 343.0 / (343.0 - 34.3)).abs() < 1e-4);
    let receding = settings.doppler(
        1.0,
        ([10.0, 0.0, 0.0], [34.3, 0.0, 0.0]),
        ([0.0; 3], [0.0; 3]),
    );
    assert!(receding < 1.0);
    assert_eq!(
        settings.doppler(
            0.0,
            ([10.0, 0.0, 0.0], [-34.3, 0.0, 0.0]),
            ([0.0; 3], [0.0; 3])
        ),
        1.0
    );
    let curve = [[1.0, 1.0], [10.0, 0.5], [40.0, 0.0]];
    assert_eq!(curve_gain(&curve, 0.5), Some(1.0));
    assert_eq!(curve_gain(&curve, 5.5), Some(0.75));
    assert_eq!(curve_gain(&curve, 99.0), Some(0.0));
    assert_eq!(curve_gain(&[[2.0, 1.0], [1.0, 0.5]], 1.5), None);
}

fn room(name: &str, x: f32) -> ReverbZone {
    ReverbZone {
        name: name.into(),
        position: [x, 1.5, 0.0],
        half_extents: [5.0, 1.5, 5.0],
        material: MaterialPreset::Concrete,
        ..ReverbZone::default()
    }
}

#[test]
fn eyring_room_model_orders_rooms_sensibly() {
    let settings = AcousticsSettings::default();
    let zones = ZoneSet::new(
        &[
            room("hall", -5.0),
            ReverbZone {
                material: MaterialPreset::Carpet,
                ..room("studio", 5.0)
            },
        ],
        &[],
        settings,
    )
    .unwrap();
    let hall = zones.room(0).rt60(&settings);
    let studio = zones.room(1).rt60(&settings);
    assert!(hall[1] > 1.0 && hall[1] < 4.0, "{hall:?}");
    assert!(studio[1] < 0.3, "{studio:?}");
    // High frequencies decay faster through air in the hall.
    assert!(hall[2] < hall[0]);
}

#[test]
fn zones_crossfade_route_through_portals_and_respect_closed_doors() {
    let settings = AcousticsSettings::default();
    let door = AcousticPortal {
        a: "west".into(),
        b: "east".into(),
        position: [0.0, 1.2, 0.0],
        width: 1.0,
        height: 2.2,
        open: 1.0,
    };
    let zones = ZoneSet::new(
        &[room("west", -5.0), room("east", 5.0)],
        std::slice::from_ref(&door),
        settings,
    )
    .unwrap();
    assert_eq!(zones.zone_at([-3.0, 1.0, 0.0]), Some(0));
    assert_eq!(zones.zone_at([30.0, 1.0, 0.0]), None);
    let deep = zones.listener([-5.0, 1.5, 0.0]);
    let edge = zones.listener([-0.2, 1.5, 0.0]);
    assert_eq!(deep.weight, 1.0);
    assert!(edge.weight < 0.75 && edge.weight >= 0.5, "{edge:?}");
    let route = zones.route([4.0, 1.5, -3.0], [-4.0, 1.5, -3.0]).unwrap();
    assert_eq!(route.portals, vec![0]);
    assert_eq!(route.apparent, door.position);
    assert!(route.length > 8.0 && route.gains[2] < route.gains[0]);
    let closed = ZoneSet::new(
        &[room("west", -5.0), room("east", 5.0)],
        &[AcousticPortal { open: 0.0, ..door }],
        settings,
    )
    .unwrap();
    assert!(closed.route([4.0, 1.5, -3.0], [-4.0, 1.5, -3.0]).is_none());
    assert!(
        ZoneSet::new(&[room("west", -5.0), room("west", 5.0)], &[], settings).is_err(),
        "duplicate zone names"
    );
}

#[test]
fn cooked_cells_roundtrip_and_reject_corruption() {
    let cell = AcousticCell {
        key: CellKey {
            x: -3,
            z: 7,
            slot: 1,
        },
        baker: "ara-geometric/1".into(),
        probes: vec![ReverbProbe {
            position: [1.0, 2.0, 3.0],
            rt60: [1.2, 1.0, 0.7],
            mean_free_path: 4.5,
            volume: 300.0,
            absorption: [0.05, 0.07, 0.08],
            escape: 0.0,
        }],
    };
    let bytes = cell.encode().unwrap();
    assert_eq!(AcousticCell::decode(&bytes).unwrap(), cell);
    let mut corrupt = bytes.clone();
    corrupt[20] ^= 1;
    assert!(AcousticCell::decode(&corrupt).is_err());
    assert!(AcousticCell::decode(&bytes[..bytes.len() - 1]).is_err());
}

fn shoebox(material: AcousticMaterial) -> MeshScene {
    let mut scene = MeshScene::new();
    scene
        .add_box([-5.0, 0.0, -4.0], [5.0, 3.0, 4.0], 10, "room", material)
        .unwrap();
    scene.commit();
    scene
}

#[test]
fn geometric_baker_matches_eyring_and_is_deterministic() {
    let scene = shoebox(concrete());
    let positions = floor_positions(&scene);
    assert_eq!(positions.len(), 9);
    let mut baker = GeometricBaker {
        rays: 512,
        ..GeometricBaker::default()
    };
    let probes = baker.bake(&scene, &positions).unwrap();
    assert_eq!(probes, baker.bake(&scene, &positions).unwrap());
    let reference = RoomAcoustics {
        volume: 240.0,
        surface_area: 2.0 * (10.0 * 3.0 + 3.0 * 8.0 + 10.0 * 8.0),
        absorption: concrete().absorption,
    };
    let expected = reference.rt60(&AcousticsSettings::default());
    let center = &probes[4];
    for b in 0..3 {
        let ratio = center.rt60[b] / expected[b];
        assert!(
            (0.7..1.3).contains(&ratio),
            "band {b}: {:?} vs {expected:?}",
            center.rt60
        );
    }
    assert!((center.mean_free_path / reference.mean_free_path() - 1.0).abs() < 0.2);
    assert!(
        (center.volume / 240.0 - 1.0).abs() < 0.25,
        "{}",
        center.volume
    );
    let mut field = ProbeField::new();
    field.load(AcousticCell {
        key: CellKey {
            x: 0,
            z: 0,
            slot: 0,
        },
        baker: baker.name(),
        probes: probes.clone(),
    });
    let sample = field.sample(center.position, 30.0, |_| true).unwrap();
    assert_eq!(sample.nearest, 0.0);
    assert!((sample.rt60[1] - center.rt60[1]).abs() < 0.05);
    assert!(field.sample([500.0, 0.0, 0.0], 30.0, |_| true).is_none());
}

fn floor_positions(scene: &MeshScene) -> Vec<[f32; 3]> {
    bake::floor_probes(scene, [-3.0, 0.0, -3.0], [3.0, 3.0, 3.0], 3.0, 1.6)
}

#[test]
#[ignore = "requires the pinned Steam Audio runtime: run crates/ara-audio/tools/fetch_runtime.ps1"]
fn steam_audio_bakes_parametric_reverb() {
    let path = std::env::var("ARA_STEAM_AUDIO").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../ara-audio/runtime/steam/steamaudio/lib/windows-x64/phonon.dll"
        )
        .into()
    });
    let mut baker = SteamBaker::load(path).unwrap();
    // Long concrete tails need enough bounces to cover the decay (16 bounces truncate it).
    baker.settings.rays = 8192;
    let scene = shoebox(concrete());
    let probes = baker.bake(&scene, &[[0.0, 1.5, 0.0]]).unwrap();
    let eyring = RoomAcoustics {
        volume: 240.0,
        surface_area: 268.0,
        absorption: concrete().absorption,
    }
    .rt60(&AcousticsSettings::default());
    println!("steam {:?} eyring {eyring:?}", probes[0].rt60);
    // Steam models little air loss in baked tails, so only low/mid follow Eyring closely.
    for b in 0..2 {
        let ratio = probes[0].rt60[b] / eyring[b];
        assert!(
            (0.6..1.6).contains(&ratio),
            "{:?} vs {eyring:?}",
            probes[0].rt60
        );
    }
    assert!(probes[0].rt60[2] <= probes[0].rt60[0]);
    let carpet = baker
        .bake(
            &shoebox(MaterialPreset::Carpet.material()),
            &[[0.0, 1.5, 0.0]],
        )
        .unwrap();
    assert!(carpet[0].rt60[1] < probes[0].rt60[1] * 0.5);
}

#[test]
fn evaluate_combines_occlusion_portals_air_and_doppler() {
    let mut scene = two_rooms();
    let mut acoustics = Acoustics::new(AcousticsSettings::default()).unwrap();
    acoustics
        .set_zones(
            &[room("west", -5.0), room("east", 5.0)],
            &[AcousticPortal {
                a: "west".into(),
                b: "east".into(),
                position: [0.0, 1.2, 0.0],
                ..AcousticPortal::default()
            }],
        )
        .unwrap();
    let listener = AudioListener {
        position: [-4.0, 1.5, -3.0],
        ..AudioListener::default()
    };
    let behind_wall = AudioSource {
        position: [4.0, 1.5, -3.0],
        max_distance: 60.0,
        ..AudioSource::default()
    };
    let same_room = AudioSource {
        position: [-4.0, 1.5, 3.0],
        velocity: [0.0, 0.0, -20.0],
        max_distance: 60.0,
        ..AudioSource::default()
    };
    let acoustic = AcousticSource::default();
    let mut out = Vec::new();
    acoustics
        .evaluate(
            &mut scene,
            &listener,
            &[
                Emitter {
                    label: "e_radio",
                    source: &behind_wall,
                    acoustic: &acoustic,
                },
                Emitter {
                    label: "e_car",
                    source: &same_room,
                    acoustic: &acoustic,
                },
            ],
            &mut out,
        )
        .unwrap();
    let (radio, car) = (&out[0], &out[1]);
    assert!(radio.record.occluded);
    assert_eq!(radio.record.blockers[0].entity, "wall north");
    assert_eq!(radio.record.blockers[0].material, "concrete");
    assert_eq!(radio.record.portals, vec!["west<->east".to_owned()]);
    assert_eq!(radio.record.zone.as_deref(), Some("east"));
    // The door path dominates the wall: sound now arrives from the doorway's side.
    let bands = radio.parameters.path.band_gains();
    assert!(bands[1] > concrete().transmission[1] * 2.0, "{bands:?}");
    // Listener faces -Z; the doorway is behind-right, the wall-blocked straight path is right.
    assert!(
        radio.parameters.direction[2] > 0.3,
        "{:?}",
        radio.parameters.direction
    );
    assert!(!car.record.occluded);
    assert!(car.parameters.rate > 1.0, "approaching car pitches up");
    assert!(car.parameters.path.reverb_send > radio.parameters.path.reverb_send);
    let reverb = acoustics.room(&listener);
    assert_eq!(reverb.early_count, ara_acoustics::reverb::MAX_TAPS);
    assert!(reverb.rt60[1] > 1.0);
}
