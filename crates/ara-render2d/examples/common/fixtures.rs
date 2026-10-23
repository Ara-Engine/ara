//! Regenerates the sample's committed sources: pixel art, atlas settings and the scene,
//! which is authored only through the typed command script `platformer.commands.json`.
use super::*;

/// Writes the source PNGs and atlas settings, then replays the command script.
pub fn prepare(root: &Path) -> Result<()> {
    let dir = root.join("sample");
    std::fs::create_dir_all(&dir)?;
    paint(&dir)?;
    std::fs::write(
        dir.join("platformer.scene"),
        author(&dir.join("platformer.commands.json"))?,
    )?;
    Ok(())
}

/// Applies the command script through the shared registry, exactly as an agent would over
/// the Engine Protocol (one batch = one undo item), and returns the canonical scene text.
/// IDs minted by commands come from a seeded generator, so the output is reproducible.
pub fn author(script: &Path) -> Result<String> {
    let mut db = schema();
    let mut registry = ara_cmd::Registry::builtin(&mut db);
    ara_2d::commands::register(&mut registry, &mut db);
    ara_physics2d::commands::register(&mut registry, &mut db);
    let mut workspace = ara_doc::Workspace::default();
    workspace.insert(
        "platformer.scene",
        ara_doc::Document::new(
            "scene",
            ara_doc::AssetId::parse("sc_platform1").map_err(anyhow::Error::msg)?,
        ),
    );
    let mut store = ara_ops::Store::with_ids(db, workspace, ara_doc::IdGen::seeded(2));
    let batches: Vec<ara_cmd::Batch> = serde_json::from_slice(&std::fs::read(script)?)?;
    for batch in &batches {
        registry
            .apply(&mut store, batch, &ara_cmd::Session::human())
            .map_err(|e| anyhow::anyhow!("{:?}: {e:?}", batch.why))?;
    }
    let doc = store
        .workspace()
        .doc("sc_platform1")
        .ok_or_else(|| anyhow::anyhow!("sample document absent"))?;
    Ok(ara_doc::write(doc, store.schema()))
}

/// Authored pixel fixtures: grass, masonry, soil, crate, hero, lantern, coins, parallax.
fn paint(dir: &Path) -> Result<()> {
    let mut color = image::RgbaImage::new(128, 96);
    let mut normal = image::RgbaImage::from_pixel(128, 96, image::Rgba([128, 128, 255, 255]));
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
            // Bevelled masonry normals; everything else stays flat.
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
                // The step frame shifts the legs apart by one pixel.
                let leg = |x: u32| {
                    if frame == 0 {
                        (3..7).contains(&x) || (9..13).contains(&x)
                    } else {
                        (2..6).contains(&x) || (10..14).contains(&x)
                    }
                };
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
                } else if y >= 17 && leg(x) {
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
    // Coin and its spin frame: a gold disc, then a narrow ellipse.
    for (frame, half_width) in [(0u32, 3.5f32), (1, 1.5)] {
        for y in 0..8 {
            for x in 0..8 {
                let dx = (x as f32 - 3.5) / half_width;
                let dy = (y as f32 - 3.5) / 3.5;
                let d = dx * dx + dy * dy;
                if d <= 1. {
                    let rim = d > 0.55;
                    let shine = x as f32 - 3.5 < -0.5 * half_width && y < 4;
                    color.put_pixel(
                        72 + frame * 8 + x,
                        y,
                        image::Rgba(if shine {
                            [255, 246, 196, 255]
                        } else if rim {
                            [214, 150, 38, 255]
                        } else {
                            [255, 205, 64, 255]
                        }),
                    );
                }
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
        ("coin", 72, 0, 8, 8),
        ("coinSpin", 80, 0, 8, 8),
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
    Ok(())
}
