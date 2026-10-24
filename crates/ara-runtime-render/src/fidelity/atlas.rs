//! Texture atlases for the fidelity snapshot's 96-texture budget: every material
//! whose geometry samples only the [0, 1] UV square gets one cell in a set of
//! parallel 4096² pages (base colour, normal, metal/roughness, emissive), and its
//! UVs are remapped into the cell. Tiling materials (kit pieces, terrain, chain-link)
//! keep their own textures. The largest surfaces get 1024² cells, the rest 512².
use anyhow::Result;
use ara_render::scene::{Material, Texture, Vertex};
use image::{RgbaImage, imageops};
use std::collections::BTreeMap;

/// Atlas page side in texels.
const PAGE: u32 = 4096;
/// Gutter around each cell, in texels (limits mip bleeding between cells).
const GUTTER: u32 = 6;
/// Cells that get 1024² instead of 512².
const LARGE_CELLS: usize = 20;

/// The neutral texel of each map: white colour/ARM/emissive, flat normal.
const NEUTRAL: [[u8; 4]; 4] = [[255; 4], [128, 128, 255, 255], [255; 4], [255; 4]];
const SRGB: [bool; 4] = [true, false, false, true];

/// Packs atlasable materials; `vertices` is each material's merged geometry and
/// `area` its world-space surface area. Appends page textures to `textures` and
/// rewrites materials and UVs. Returns the number of materials packed.
pub fn pack(
    textures: &mut Vec<Texture>,
    materials: &mut [Material],
    vertices: &mut BTreeMap<usize, &mut Vec<Vertex>>,
    area: &[f64],
) -> Result<usize> {
    // Distinct texture tuples of materials whose UVs stay inside the unit square.
    let mut tuples: BTreeMap<[u32; 4], Vec<usize>> = BTreeMap::new();
    for (m, material) in materials.iter().enumerate() {
        let Some(verts) = vertices.get(&m) else {
            continue;
        };
        let textured = material.textures.iter().any(|&t| t > 1);
        let inside = verts
            .iter()
            .all(|v| v.uv.iter().all(|c| (-0.002..=1.002).contains(c)));
        if textured && inside {
            tuples.entry(material.textures).or_default().push(m);
        }
    }
    if tuples.is_empty() {
        return Ok(0);
    }
    let mut order: Vec<([u32; 4], f64)> = tuples
        .iter()
        .map(|(t, ms)| (*t, ms.iter().map(|&m| area[m]).sum()))
        .collect();
    order.sort_by(|a, b| b.1.total_cmp(&a.1));
    // Shelf-pack cells: large ones first, then small ones, page by page.
    let mut cells: BTreeMap<[u32; 4], (usize, u32, u32, u32)> = BTreeMap::new();
    let (mut page, mut x, mut y, mut shelf) = (0usize, 0u32, 0u32, 0u32);
    for (i, (tuple, _)) in order.iter().enumerate() {
        let size = if i < LARGE_CELLS { 1024 } else { 512 };
        if x + size > PAGE {
            x = 0;
            y += shelf;
            shelf = 0;
        }
        if y + size > PAGE {
            page += 1;
            x = 0;
            y = 0;
            shelf = 0;
        }
        cells.insert(*tuple, (page, x, y, size));
        x += size;
        shelf = shelf.max(size);
    }
    let pages = page + 1;
    // Which maps any packed material uses; only those get pages.
    let used: [bool; 4] = std::array::from_fn(|slot| cells.keys().any(|t| t[slot] > 1));
    let mut images: Vec<[Option<RgbaImage>; 4]> = (0..pages)
        .map(|_| {
            std::array::from_fn(|slot| {
                used[slot].then(|| RgbaImage::from_pixel(PAGE, PAGE, image::Rgba(NEUTRAL[slot])))
            })
        })
        .collect();
    let mut decoded: BTreeMap<u32, RgbaImage> = BTreeMap::new();
    for (tuple, &(page, x, y, size)) in &cells {
        for slot in 0..4 {
            let Some(target) = images[page][slot].as_mut() else {
                continue;
            };
            let t = tuple[slot];
            if t <= 1 {
                continue;
            }
            let source = decoded.entry(t).or_insert_with(|| {
                let tex = &textures[t as usize];
                RgbaImage::from_raw(tex.size[0], tex.size[1], tex.rgba.clone())
                    .unwrap_or_else(|| RgbaImage::new(1, 1))
            });
            let inner = size - 2 * GUTTER;
            let cell = imageops::resize(source, inner, inner, imageops::FilterType::Triangle);
            // Edge-extend into the gutter so filtering at the cell border stays inside.
            for gy in 0..size {
                for gx in 0..size {
                    let sx = gx.saturating_sub(GUTTER).min(inner - 1);
                    let sy = gy.saturating_sub(GUTTER).min(inner - 1);
                    target.put_pixel(x + gx, y + gy, *cell.get_pixel(sx, sy));
                }
            }
        }
    }
    let mut page_slots: Vec<[u32; 4]> = Vec::new();
    for page_images in images {
        let mut slots = [0, 1, 0, 0];
        for (slot, image) in page_images.into_iter().enumerate() {
            if let Some(image) = image {
                slots[slot] = textures.len() as u32;
                textures.push(Texture {
                    size: [PAGE, PAGE],
                    rgba: image.into_raw(),
                    srgb: SRGB[slot],
                });
            }
        }
        page_slots.push(slots);
    }
    let mut packed = 0;
    for (tuple, ms) in &tuples {
        let (page, x, y, size) = cells[tuple];
        let scale = (size - 2 * GUTTER) as f32 / PAGE as f32;
        let offset = [
            (x + GUTTER) as f32 / PAGE as f32,
            (y + GUTTER) as f32 / PAGE as f32,
        ];
        for &m in ms {
            // Untextured slots keep their neutral 1×1 textures.
            for slot in 0..4 {
                if tuple[slot] > 1 {
                    materials[m].textures[slot] = page_slots[page][slot];
                }
            }
            if let Some(verts) = vertices.get_mut(&m) {
                for v in verts.iter_mut() {
                    v.uv = [
                        offset[0] + v.uv[0].clamp(0.0, 1.0) * scale,
                        offset[1] + v.uv[1].clamp(0.0, 1.0) * scale,
                    ];
                }
            }
            packed += 1;
        }
    }
    Ok(packed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texture(value: u8) -> Texture {
        Texture {
            size: [8, 8],
            rgba: vec![value; 8 * 8 * 4],
            srgb: true,
        }
    }

    fn vertex(uv: [f32; 2]) -> Vertex {
        Vertex {
            position: [0.0; 3],
            normal: [0.0, 1.0, 0.0],
            uv,
            tangent: [1.0, 0.0, 0.0, 1.0],
        }
    }

    #[test]
    fn unit_square_materials_move_into_cells_and_tiling_ones_stay() {
        let mut textures = vec![texture(255), texture(128), texture(10), texture(20)];
        let mut materials = vec![Material::default(), Material::default()];
        materials[0].textures[0] = 2;
        materials[1].textures[0] = 3;
        let mut packed = vec![vertex([0.0, 0.0]), vertex([1.0, 1.0])];
        let mut tiling = vec![vertex([0.0, 0.0]), vertex([4.0, 1.0])];
        let mut vertices = BTreeMap::from([(0, &mut packed), (1, &mut tiling)]);
        let n = pack(&mut textures, &mut materials, &mut vertices, &[10.0, 5.0]).expect("pack");
        assert_eq!(n, 1);
        assert_eq!(textures.len(), 5, "one colour page added");
        assert_eq!(materials[0].textures[0], 4);
        assert_eq!(
            materials[1].textures[0], 3,
            "tiling material keeps its texture"
        );
        for v in &packed {
            assert!(
                v.uv.iter().all(|c| (0.0..=1024.0 / 4096.0).contains(c)),
                "{:?}",
                v.uv
            );
        }
        assert_eq!(tiling[1].uv, [4.0, 1.0]);
    }
}
