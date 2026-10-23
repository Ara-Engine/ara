use crate::{Operation, Terrain, Tile, TileCoord, cache::TileSource, invalid};
use ara_diag::Diagnostic;
use std::collections::BTreeMap;

impl Terrain {
    /// Compiles one tile on a worker. Erosion reads a full dependency halo so borders
    /// agree regardless of tile request order; no neighboring runtime tile is mutated.
    pub fn evaluate(&self, coord: TileCoord, source: &impl TileSource) -> Result<Tile, Diagnostic> {
        self.validate()?;
        if !self.header.contains(coord) {
            return Err(invalid("tile outside terrain"));
        }
        let mut layers: Vec<_> = self
            .layers
            .iter()
            .filter(|(_, l)| l.enabled && l.opacity > 0.)
            .collect();
        layers.sort_by_key(|(id, l)| (l.priority, *id));
        let mut strokes = Vec::new();
        for (id, l) in layers {
            let mut ordered: Vec<_> = self
                .strokes
                .iter()
                .filter(|(_, s)| &s.layer == id)
                .collect();
            ordered.sort_by_key(|(id, s)| (s.sequence, *id));
            strokes.extend(ordered.into_iter().map(|(_, s)| (s, l.opacity)));
        }
        let halo: usize = strokes
            .iter()
            .filter(|(s, _)| s.operation == Operation::Erode)
            .map(|(s, _)| s.iterations as usize * s.dabs.len())
            .sum();
        if halo > 256 {
            return Err(invalid(
                "erosion dependency halo exceeds 256 samples; bake a checkpoint",
            ));
        }
        let n = self.header.cells as usize + 1;
        let side = n + halo * 2;
        let mut heights = vec![0.; side * side];
        let mut weights = vec![[255, 0, 0, 0]; side * side];
        let mut holes = vec![0; side * side];
        let mut bases = BTreeMap::new();
        let cells = self.header.cells as i32;
        for z in 0..side {
            for x in 0..side {
                let gx = coord.x * cells + x as i32 - halo as i32;
                let gz = coord.z * cells + z as i32 - halo as i32;
                let c = TileCoord {
                    x: gx
                        .div_euclid(cells)
                        .clamp(self.header.min.x, self.header.max.x - 1),
                    z: gz
                        .div_euclid(cells)
                        .clamp(self.header.min.z, self.header.max.z - 1),
                };
                if let std::collections::btree_map::Entry::Vacant(e) = bases.entry(c) {
                    let tile = self
                        .tiles
                        .values()
                        .find(|t| t.coord == c)
                        .and_then(|t| t.base.as_ref())
                        .map(|h| source.load(h))
                        .transpose()?;
                    if tile.as_ref().is_some_and(|t| t.cells != self.header.cells) {
                        return Err(invalid("base tile grid mismatch"));
                    }
                    e.insert(tile);
                }
                if let Some(Some(tile)) = bases.get(&c) {
                    let sx = (gx - c.x * cells).clamp(0, cells) as usize;
                    let sz = (gz - c.z * cells).clamp(0, cells) as usize;
                    let i = sz * n + sx;
                    let dst = z * side + x;
                    heights[dst] = tile.heights[i];
                    weights[dst] = tile.weights[i];
                    holes[dst] = tile.holes[i];
                }
            }
        }
        let world = |x: usize, z: usize| {
            [
                self.header.origin[0]
                    + (coord.x * cells + x as i32 - halo as i32) as f32 * self.header.spacing,
                self.header.origin[1]
                    + (coord.z * cells + z as i32 - halo as i32) as f32 * self.header.spacing,
            ]
        };
        let mut scratch = vec![0.; heights.len()];
        for (stroke, opacity) in strokes {
            let stamp = stroke.stamp.as_ref().map(|h| source.load(h)).transpose()?;
            for dab in &stroke.dabs {
                let iterations = if stroke.operation == Operation::Erode {
                    stroke.iterations
                } else {
                    1
                };
                for _ in 0..iterations {
                    if stroke.operation == Operation::Erode {
                        scratch.copy_from_slice(&heights);
                    }
                    for z in 0..side {
                        for x in 0..side {
                            let p = world(x, z);
                            let dx = p[0] - dab.center[0];
                            let dz = p[1] - dab.center[1];
                            let r2 = (dx * dx + dz * dz) / (dab.radius * dab.radius);
                            if stroke.operation == Operation::Import {
                                let c = stroke
                                    .source_tile
                                    .ok_or_else(|| invalid("import missing destination"))?;
                                let width = self.header.tile_width();
                                let u = (p[0] - self.header.origin[0] - c.x as f32 * width) / width;
                                let v = (p[1] - self.header.origin[1] - c.z as f32 * width) / width;
                                if (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v) {
                                    let tile = stamp
                                        .as_ref()
                                        .ok_or_else(|| invalid("import payload missing"))?;
                                    let i = z * side + x;
                                    heights[i] += (bilinear(tile, u, v) - heights[i]) * opacity;
                                    let at = ((v * tile.cells as f32).round() as usize)
                                        * (tile.cells as usize + 1)
                                        + (u * tile.cells as f32).round() as usize;
                                    let mut remaining = 255u32;
                                    for (weight, source) in
                                        weights[i].iter_mut().zip(tile.weights[at])
                                    {
                                        let mixed = (f32::from(*weight) * (1. - opacity)
                                            + f32::from(source) * opacity)
                                            .round()
                                            as u32;
                                        *weight = mixed.min(remaining) as u8;
                                        remaining -= u32::from(*weight);
                                    }
                                    weights[i][0] = (u32::from(weights[i][0]) + remaining) as u8;
                                    if opacity >= 0.5 {
                                        holes[i] = tile.holes[at];
                                    }
                                }
                                continue;
                            }
                            if r2 >= 1. {
                                continue;
                            }
                            let strength = (1. - r2).powi(2) * opacity;
                            let i = z * side + x;
                            match stroke.operation {
                                Operation::Raise => heights[i] += dab.amount * strength,
                                Operation::Paint => paint(
                                    &mut weights[i],
                                    stroke.channel as usize,
                                    (dab.amount * strength).clamp(0., 1.),
                                ),
                                Operation::Hole => {
                                    if strength > 0.5 {
                                        holes[i] = u8::from(dab.amount > 0.);
                                    }
                                }
                                Operation::Stamp => {
                                    if let Some(stamp) = &stamp {
                                        let (sin, cos) = stroke.angle.sin_cos();
                                        let u = (dx * cos - dz * sin) / dab.radius * 0.5 + 0.5;
                                        let v = (dx * sin + dz * cos) / dab.radius * 0.5 + 0.5;
                                        heights[i] += bilinear(stamp, u, v) * dab.amount * strength;
                                    }
                                }
                                Operation::Erode => {
                                    if x > 0 && z > 0 && x + 1 < side && z + 1 < side {
                                        let avg = (scratch[i - 1]
                                            + scratch[i + 1]
                                            + scratch[i - side]
                                            + scratch[i + side])
                                            * 0.25;
                                        heights[i] += (avg - scratch[i])
                                            * (dab.amount * strength).clamp(0., 0.5);
                                    }
                                }
                                Operation::Import => {}
                            }
                        }
                    }
                }
            }
        }
        let mut tile = Tile::flat(self.header.cells)?;
        for z in 0..n {
            for x in 0..n {
                let src = (z + halo) * side + x + halo;
                let dst = z * n + x;
                tile.heights[dst] = heights[src];
                tile.weights[dst] = weights[src];
                tile.holes[dst] = holes[src];
            }
        }
        tile.validate()?;
        Ok(tile)
    }
}
fn paint(w: &mut [u8; 4], channel: usize, strength: f32) {
    let mut remaining = 255u32;
    let mut result = [0u8; 4];
    for i in 0..4 {
        let target = if i == channel { 255. } else { 0. };
        let v = (f32::from(w[i]) + (target - f32::from(w[i])) * strength).round() as u32;
        result[i] = v.min(remaining) as u8;
        remaining -= u32::from(result[i]);
    }
    result[channel] = (u32::from(result[channel]) + remaining) as u8;
    *w = result;
}
fn bilinear(t: &Tile, u: f32, v: f32) -> f32 {
    let x = u.clamp(0., 1.) * t.cells as f32;
    let z = v.clamp(0., 1.) * t.cells as f32;
    let ix = (x.floor() as usize).min(t.cells as usize - 1);
    let iz = (z.floor() as usize).min(t.cells as usize - 1);
    let fx = x - ix as f32;
    let fz = z - iz as f32;
    let side = t.cells as usize + 1;
    let a = t.heights[iz * side + ix] * (1. - fx) + t.heights[iz * side + ix + 1] * fx;
    let b = t.heights[(iz + 1) * side + ix] * (1. - fx) + t.heights[(iz + 1) * side + ix + 1] * fx;
    a * (1. - fz) + b * fz
}
