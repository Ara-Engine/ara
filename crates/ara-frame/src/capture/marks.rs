use super::LegendEntry;
use image::{Rgba, RgbaImage};
use std::collections::BTreeMap;

pub(super) fn label(
    image: &RgbaImage,
    ids: &[u32],
    names: &[(u64, String)],
    max: usize,
) -> (RgbaImage, Vec<LegendEntry>) {
    let mut bounds = BTreeMap::<u32, [u32; 4]>::new();
    let w = image.width();
    for (pixel, &id) in ids.iter().enumerate().filter(|(_, id)| **id != 0) {
        let x = pixel as u32 % w;
        let y = pixel as u32 / w;
        let b = bounds.entry(id).or_insert([x, y, x, y]);
        b[0] = b[0].min(x);
        b[1] = b[1].min(y);
        b[2] = b[2].max(x);
        b[3] = b[3].max(y);
    }
    let mut marked = image.clone();
    let mut legend = Vec::new();
    for (index, (&id, &[x, y, right, bottom])) in bounds.iter().take(max).enumerate() {
        let centre = [(x + right) / 2, (y + bottom) / 2];
        let anchor = ids
            .iter()
            .enumerate()
            .filter(|(_, value)| **value == id)
            .min_by_key(|(i, _)| {
                let px = *i as i64 % i64::from(w);
                let py = *i as i64 / i64::from(w);
                (px - i64::from(centre[0])).pow(2) + (py - i64::from(centre[1])).pow(2)
            })
            .map(|(i, _)| [i as u32 % w, i as u32 / w])
            .unwrap_or(centre);
        number(&mut marked, anchor, index + 1);
        let (entity, name) = &names[id as usize - 1];
        legend.push(LegendEntry {
            mark: index + 1,
            entity: *entity,
            name: name.clone(),
            bbox: [x, y, right - x + 1, bottom - y + 1],
        });
    }
    (marked, legend)
}
fn number(image: &mut RgbaImage, anchor: [u32; 2], mark: usize) {
    const DIGITS: [[u8; 5]; 10] = [
        [7, 5, 5, 5, 7],
        [2, 6, 2, 2, 7],
        [7, 1, 7, 4, 7],
        [7, 1, 7, 1, 7],
        [5, 5, 7, 1, 1],
        [7, 4, 7, 1, 7],
        [7, 4, 7, 5, 7],
        [7, 1, 1, 1, 1],
        [7, 5, 7, 5, 7],
        [7, 5, 7, 1, 7],
    ];
    let text = mark.to_string();
    let width = text.len() as u32 * 8 + 4;
    let height = 14u32;
    let ox = anchor[0]
        .saturating_sub(width / 2)
        .min(image.width().saturating_sub(width));
    let oy = anchor[1]
        .saturating_sub(height / 2)
        .min(image.height().saturating_sub(height));
    for y in 0..height.min(image.height()) {
        for x in 0..width.min(image.width()) {
            image.put_pixel(
                ox + x,
                oy + y,
                Rgba(if x == 0 || y == 0 || x == width - 1 || y == height - 1 {
                    [255, 210, 64, 255]
                } else {
                    [15, 20, 28, 255]
                }),
            );
        }
    }
    for (i, digit) in text.bytes().enumerate() {
        for (y, &row) in DIGITS[(digit - b'0') as usize].iter().enumerate() {
            for x in 0..3 {
                if row & (1 << (2 - x)) != 0 {
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let px = ox + 3 + i as u32 * 8 + x * 2 + dx;
                            let py = oy + 2 + y as u32 * 2 + dy;
                            if px < image.width() && py < image.height() {
                                image.put_pixel(px, py, Rgba([255, 255, 255, 255]));
                            }
                        }
                    }
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn marks_use_visible_bounds_and_preserve_stable_ids() {
        let image = RgbaImage::new(32, 24);
        let mut ids = vec![0; 32 * 24];
        ids[5 * 32 + 4] = 1;
        ids[7 * 32 + 6] = 1;
        let (_, legend) = label(&image, &ids, &[(999, "Mesh".into())], 40);
        assert_eq!(legend[0].entity, 999);
        assert_eq!(legend[0].bbox, [4, 5, 3, 3]);
    }
}
