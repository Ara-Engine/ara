use super::*;
pub fn prepare(root: &Path) -> Result<()> {
    let dir = root.join("sample");
    std::fs::create_dir_all(&dir)?;
    let mut color = image::RgbaImage::new(128, 96);
    let mut normal = image::RgbaImage::from_pixel(128, 96, image::Rgba([128, 128, 255, 255]));
    // Authored pixel fixtures: grass, masonry, soil, crate, hero, lantern and parallax.
    for y in 0..16 {
        for x in 0..16 {
            let shade = ((x * 13 + y * 7) % 5) as u8;
            color.put_pixel(
                x,
                y,
                image::Rgba(if y < 3 {
                    [80 + shade * 9, 155 + shade * 7, 87, 255]
                } else {
                    [57 + shade * 5, 61 + shade * 4, 76 + shade * 5, 255]
                }),
            );
            color.put_pixel(
                16 + x,
                y,
                image::Rgba(if y % 8 == 0 || (x + (y / 8) * 8) % 16 == 0 {
                    [38, 46, 65, 255]
                } else {
                    [86 + shade * 6, 98 + shade * 6, 116 + shade * 6, 255]
                }),
            );
            color.put_pixel(
                32 + x,
                y,
                image::Rgba([62 + shade * 6, 48 + shade * 4, 54 + shade * 4, 255]),
            );
            let edge = x < 2 || y < 2 || x > 13 || y > 13;
            let cross = (x as i32 - y as i32).abs() < 2 || (x + y).abs_diff(15) < 2;
            color.put_pixel(
                48 + x,
                y,
                image::Rgba(if edge {
                    [58, 48, 52, 255]
                } else if cross {
                    [172, 119, 74, 255]
                } else {
                    [110 + shade * 4, 75 + shade * 3, 61, 255]
                }),
            );
            let nx = (x as f32 / 15. - 0.5) * 0.8;
            let ny = (0.5 - y as f32 / 15.) * 0.8;
            let nz = (1. - nx * nx - ny * ny).sqrt();
            normal.put_pixel(
                16 + x,
                y,
                image::Rgba([
                    ((nx + 1.) * 127.5) as u8,
                    ((ny + 1.) * 127.5) as u8,
                    ((nz + 1.) * 127.5) as u8,
                    255,
                ]),
            );
        }
    }
    for frame in 0..2 {
        for y in 0..24 {
            for x in 0..16 {
                let pixel = if y < 7 && (3..13).contains(&x) {
                    Some(if y < 2 {
                        [43, 54, 75, 255]
                    } else if x == 10 && y == 4 {
                        [20, 30, 45, 255]
                    } else {
                        [240, 188, 138, 255]
                    })
                } else if (7..17).contains(&y) && (2..14).contains(&x) {
                    Some(if x < 6 {
                        [243, 186, 73, 255]
                    } else {
                        [67, 158, 181, 255]
                    })
                } else if y >= 17 && ((3..7).contains(&x) || (9..13).contains(&x)) {
                    Some(if y > 21 {
                        [38, 45, 62, 255]
                    } else {
                        [51, 68, 101, 255]
                    })
                } else {
                    None
                };
                if let Some(pixel) = pixel {
                    color.put_pixel(frame * 16 + x, 16 + y, image::Rgba(pixel));
                }
            }
        }
    }
    for y in 0..16 {
        for x in 0..8 {
            if (2..6).contains(&x) || y == 2 || y == 13 {
                color.put_pixel(
                    64 + x,
                    y,
                    image::Rgba(if (4..12).contains(&y) {
                        [255, 211, 107, 255]
                    } else {
                        [60, 67, 80, 255]
                    }),
                );
            }
        }
    }
    for y in 0..32 {
        for x in 0..64 {
            let peak = 15. - (x as f32 - 20.).abs() * 0.5;
            let peak2 = 19. - (x as f32 - 48.).abs() * 0.7;
            if y as f32 > 32. - peak.max(peak2) {
                color.put_pixel(
                    x,
                    64 + y,
                    image::Rgba(if y < 18 {
                        [62, 81, 120, 255]
                    } else {
                        [48, 66, 102, 255]
                    }),
                );
            }
            if ((x as f32 - 20.) / 19.).powi(2) + ((y as f32 - 19.) / 7.).powi(2) < 1.
                || ((x as f32 - 38.) / 14.).powi(2) + ((y as f32 - 16.) / 10.).powi(2) < 1.
            {
                color.put_pixel(64 + x, 64 + y, image::Rgba([110, 137, 168, 150]));
            }
        }
    }
    color.save(dir.join("platformer.png"))?;
    normal.save(dir.join("platformer-normal.png"))?;
    let regions = [
        ("grass", 0, 0, 16, 16),
        ("stone", 16, 0, 16, 16),
        ("dirt", 32, 0, 16, 16),
        ("crate", 48, 0, 16, 16),
        ("heroIdle", 0, 16, 16, 24),
        ("heroStep", 16, 16, 16, 24),
        ("lantern", 64, 0, 8, 16),
        ("mountain", 0, 64, 64, 32),
        ("cloud", 64, 64, 64, 32),
    ];
    let settings = ara_import2d::AtlasSettings {
        max_size: 256,
        padding: 2,
        regions: regions
            .into_iter()
            .map(|(name, x, y, width, height)| ara_import2d::SourceRegion {
                name: name.into(),
                source: String::new(),
                region: SpriteRegion {
                    x,
                    y,
                    width,
                    height,
                    borders: if name == "crate" { [3.; 4] } else { [0.; 4] },
                    ..Default::default()
                },
            })
            .collect(),
    };
    std::fs::write(
        dir.join("atlas.settings.json"),
        ara_2d::canonical(&settings)?,
    )?;
    let mut map = Tilemap {
        atlas: Some(AssetRef(ATLAS.into())),
        normal_atlas: Some(AssetRef(NORMAL.into())),
        definitions: [
            (
                "1".into(),
                TileDefinition {
                    region: "grass".into(),
                    solid: true,
                    lit: true,
                },
            ),
            (
                "2".into(),
                TileDefinition {
                    region: "stone".into(),
                    solid: true,
                    lit: true,
                },
            ),
            (
                "3".into(),
                TileDefinition {
                    region: "dirt".into(),
                    solid: true,
                    lit: true,
                },
            ),
        ]
        .into(),
        layers: vec![
            TileLayer {
                id: "l_ground".into(),
                sorting_layer: 0,
                ..Default::default()
            },
            TileLayer {
                id: "l_ruins".into(),
                sorting_layer: 1,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    for x in -24..24 {
        for y in -6..=-3 {
            map.layers[0].cells.insert(
                ara_2d::tilemap::key(x, y),
                Tile {
                    tile: if y == -3 { 1 } else { 3 },
                    terrain: 1,
                },
            );
        }
    }
    for (left, right, y) in [(-7, -3, 0), (2, 6, -1)] {
        for x in left..=right {
            map.layers[1].cells.insert(
                ara_2d::tilemap::key(x, y),
                Tile {
                    tile: 2,
                    terrain: 2,
                },
            );
        }
    }
    for y in -2..3 {
        map.layers[1].cells.insert(
            ara_2d::tilemap::key(7, y),
            Tile {
                tile: 2,
                terrain: 2,
            },
        );
    }
    let sprite = |region: &str, size: [f32; 2], layer: i32, lit: bool, parallax: [f32; 2]| Sprite {
        atlas: Some(AssetRef(ATLAS.into())),
        normal_atlas: Some(AssetRef(NORMAL.into())),
        region: region.into(),
        size,
        sorting_layer: layer,
        lit,
        parallax,
        ..Default::default()
    };
    let mut records = serde_json::Map::new();
    records.insert(
        "e_map1".into(),
        serde_json::json!({"name":"Ruins tilemap","Tilemap":map}),
    );
    records.insert("e_hero1".into(),serde_json::json!({"name":"Scout","Transform":{"position":[-1.5,-1.75,0]},"Sprite":sprite("heroIdle",[1.,1.5],3,true,[1.;2]),"PhysicsPose2d":{"position":[-1.5,-1.75]},"CharacterController2d":{"height":1.5},"Flipbook":{"frames":[{"region":"heroIdle","ticks":8},{"region":"heroStep","ticks":8}],"looping":true}}));
    records.insert("e_crate1".into(),serde_json::json!({"name":"Nine-slice supply crate","Transform":{"position":[1.,-1.75,0]},"Sprite":Sprite {nine_slice:true,..sprite("crate",[1.75,1.5],2,true,[1.;2])}}));
    for (id, x, y) in [("e_lamp1", -4., 1.), ("e_lamp2", 5., 0.)] {
        records.insert(id.into(),serde_json::json!({"name":"Amber lantern","Transform":{"position":[x,y,0]},"Sprite":sprite("lantern",[0.5,1.],4,false,[1.;2])}));
    }
    records.insert("e_light1".into(),serde_json::json!({"name":"Lantern light","Light2d":Light2d {position:[-3.8,1.2,1.8],color:[3.4,2.1,0.9],radius:8.,shadows:true}}));
    records.insert("e_light2".into(),serde_json::json!({"name":"Blue rim light","Light2d":Light2d {position:[6.,2.4,1.5],color:[0.5,1.3,2.8],radius:6.,shadows:true}}));
    for (id, x) in [("e_peak1", -7.), ("e_peak2", 7.)] {
        records.insert(id.into(),serde_json::json!({"name":"Parallax mountain","Transform":{"position":[x,-0.25,0]},"Sprite":sprite("mountain",[16.,8.],-10,false,[0.3,0.3])}));
    }
    records.insert("e_cloud1".into(),serde_json::json!({"name":"Parallax clouds","Transform":{"position":[-2.,3.,0]},"Sprite":sprite("cloud",[10.,5.],-9,false,[0.15,0.15])}));
    records.insert(
        "e_camera1".into(),
        serde_json::json!({"name":"Pixel camera","Camera2d":camera()}),
    );
    let value =
        serde_json::json!({"$schema":"ara://schema/scene/1","id":"sc_platform1","records":records});
    let db = schema();
    let read = ara_doc::read(
        &serde_json::to_string(&value)?,
        "platformer.scene",
        &db,
        None,
    );
    ensure!(!read.diags.iter().any(|d| d.is_error()), "{:?}", read.diags);
    std::fs::write(
        dir.join("platformer.scene"),
        ara_doc::write(
            &read
                .doc
                .ok_or_else(|| anyhow::anyhow!("sample document absent"))?,
            &db,
        ),
    )?;
    let manifest = root.join("shaders/pipelines.json");
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&manifest)?)?;
    ara_gpu::cooked::write_json(&manifest, &value)?;
    Ok(())
}
