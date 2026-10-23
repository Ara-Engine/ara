#![allow(dead_code)]
use ara_nav::*;

pub fn quad(g: &mut Geometry, vertices: [[f32; 3]; 4], area: u8) {
    let i = g.vertices.len() as u32;
    g.vertices.extend(vertices);
    g.triangles.extend([[i, i + 2, i + 1], [i, i + 3, i + 2]]);
    g.areas.extend([area; 2]);
}
pub fn floor(g: &mut Geometry, x: [f32; 2], z: [f32; 2], y: f32, area: u8) {
    quad(
        g,
        [
            [x[0], y, z[0]],
            [x[1], y, z[0]],
            [x[1], y, z[1]],
            [x[0], y, z[1]],
        ],
        area,
    );
}
pub fn input(geometry: Geometry, bounds: Bounds) -> BakeInput {
    let settings = BakeSettings {
        origin: [-4.0, -4.0, -4.0],
        tile_size: 32,
        ..Default::default()
    };
    let tiles = BakeInput::tiles_in(&settings, bounds).unwrap();
    BakeInput {
        settings,
        geometry,
        tiles,
        links: vec![],
        obstacles: vec![],
    }
}
pub fn plane(size: f32) -> BakeInput {
    let mut g = Geometry::default();
    floor(&mut g, [0.0, size], [0.0, size], 0.0, 0);
    input(
        g,
        Bounds {
            min: [0.0, -1.0, 0.0],
            max: [size, 1.0, size],
        },
    )
}
pub fn level() -> BakeInput {
    let mut g = Geometry::default();
    floor(&mut g, [0.0, 12.0], [0.0, 12.0], 0.0, 0);
    // Sloped ramp, followed by a raised room and climbable 20cm stairs.
    quad(
        &mut g,
        [
            [12.0, 0.0, 4.0],
            [20.0, 2.0, 4.0],
            [20.0, 2.0, 8.0],
            [12.0, 0.0, 8.0],
        ],
        0,
    );
    floor(&mut g, [20.0, 32.0], [0.0, 12.0], 2.0, 0);
    for step in 0..8 {
        let x = 32.0 + step as f32 * 0.5;
        floor(
            &mut g,
            [x, x + 0.5],
            [4.0, 8.0],
            2.0 + (step + 1) as f32 * 0.2,
            0,
        );
    }
    floor(&mut g, [36.0, 48.0], [0.0, 12.0], 3.6, 0);
    floor(&mut g, [50.0, 60.0], [0.0, 12.0], 3.6, 0);
    // Bridge above an independently walkable lower room.
    floor(&mut g, [0.0, 12.0], [12.0, 24.0], 0.0, 0);
    floor(&mut g, [8.0, 40.0], [16.0, 20.0], 4.0, 0);
    quad(
        &mut g,
        [
            [36.0, 3.6, 12.0],
            [40.0, 3.6, 12.0],
            [40.0, 4.0, 16.0],
            [36.0, 4.0, 16.0],
        ],
        0,
    );
    let mut request = input(
        g,
        Bounds {
            min: [0.0, -1.0, 0.0],
            max: [60.0, 6.0, 24.0],
        },
    );
    request.links.push(OffMeshLink {
        id: 7,
        start: [47.0, 3.6, 6.0],
        end: [51.0, 3.6, 6.0],
        radius: 0.8,
        area: 1,
        bidirectional: true,
        enabled: true,
        ..OffMeshLink::default()
    });
    request
}
pub fn bake(input: &BakeInput) -> (BakeArtifact, NavMesh) {
    let artifact = Baker::new(None).bake(input, None).unwrap();
    let mesh = NavMesh::from_artifact(&artifact).unwrap();
    (artifact, mesh)
}
