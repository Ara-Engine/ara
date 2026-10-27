//! Cached local-light shadows ([`LocalShadows`]): one D32 atlas of square face
//! tiles. Each frame the brightest lights in view hold faces: a point light, or
//! a spot wider than 120 degrees, a cube of six 90-degree faces; a narrower spot
//! one face over its cone. A face is redrawn only when its light changed or
//! changed casters entered its frustum, at most `redraws` faces a frame, and a
//! light shades unshadowed until all its faces are drawn; moved casters in front
//! of a light patch only the texels they reach (`LocalDraw::patch`, beyond the
//! budget), as cascades do. One table serves
//! both: per light (first record, count); per tile its current view-projection,
//! which the shadow draws read as cascade 4 + tile; then per shading record the
//! face's view-projection and atlas tile.
use super::{LocalShadows, cascades::Patch};
use crate::{
    graph::State,
    scene::{Light, MAX_LOCAL_LIGHTS},
};
use anyhow::Result;
use ara_gpu::{Access, Buffer, Device, GpuDevice, Image, Queue, Resource, vk};
use glam::{Mat4, Vec2, Vec3, Vec4};
use std::collections::BTreeSet;

/// Tiles per atlas row.
const ROW: u32 = 8;
/// Near plane of the face projections, metres.
const NEAR: f32 = 0.05;
/// Field-of-view widening (past 90 degrees, or a spot's cone), so filtering at a
/// face's edge stays inside it.
const MARGIN: f32 = 1.08;
/// Narrowest spot cone (outer cosine) shadowed by one face rather than a cube.
const SINGLE_FACE: f32 = 0.5;
/// Most face patches a frame draws; more wait for the next frame.
pub(crate) const PATCHES: usize = 4;
/// Most tiles a budget may hold (16 lights x 6 faces).
pub(crate) const MAX_FACES: usize = 96;
/// Byte offset of the per-tile matrices in the table, after one (first, count)
/// pair per light.
pub(crate) const FACES: usize = MAX_LOCAL_LIGHTS * 8;
/// Byte offset of the shading records in the table.
pub(crate) const RECORDS: usize = FACES + MAX_FACES * 64;
/// Bytes per face record: view-projection, then the tile's uv offset and scale.
pub(crate) const RECORD_BYTES: usize = 80;

/// A light's identity for caching: its exact placement, reach and cone.
type Key = [u32; 9];

fn key(light: &Light) -> Key {
    let [x, y, z, range] = light.position_range;
    let [dx, dy, dz, outer] = light.direction_outer;
    [x, y, z, range, dx, dy, dz, outer, light.parameters[0]].map(f32::to_bits)
}

/// The face view-projections of `light` (reversed-Z, near 5 cm, far at its
/// range): six cube faces in +X, -X, +Y, -Y, +Z, -Z order, or one for a narrow
/// spot. Shading picks a cube face by the major axis of the light-to-point vector.
pub(crate) fn faces(light: &Light) -> Vec<Mat4> {
    let position = Vec3::from_slice(&light.position_range[..3]);
    let far = light.position_range[3].max(NEAR * 2.);
    let projection = |fov: f32| Mat4::perspective_rh(fov.min(170f32.to_radians()), 1., far, NEAR);
    let outer = light.direction_outer[3];
    if light.parameters[0] > 0.5 && outer >= SINGLE_FACE {
        let direction = Vec3::from_slice(&light.direction_outer[..3]).normalize_or(Vec3::NEG_Y);
        let up = if direction.y.abs() > 0.99 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        let fov = 2. * outer.min(1.).acos() * MARGIN;
        return vec![projection(fov) * Mat4::look_at_rh(position, position + direction, up)];
    }
    [
        (Vec3::X, Vec3::Y),
        (Vec3::NEG_X, Vec3::Y),
        (Vec3::Y, Vec3::Z),
        (Vec3::NEG_Y, Vec3::Z),
        (Vec3::Z, Vec3::Y),
        (Vec3::NEG_Z, Vec3::Y),
    ]
    .map(|(direction, up)| {
        projection(std::f32::consts::FRAC_PI_2 * MARGIN)
            * Mat4::look_at_rh(position, position + direction, up)
    })
    .to_vec()
}

/// Clip planes of `matrix` (reversed-Z): inside when every dot is non-negative.
fn planes(matrix: Mat4) -> [Vec4; 6] {
    let (x, y, z, w) = (matrix.row(0), matrix.row(1), matrix.row(2), matrix.row(3));
    [w + x, w - x, w + y, w - y, z, w - z]
}

/// Whether world bounds `lo..hi` reach the frustum of `matrix`.
pub(crate) fn overlaps(matrix: Mat4, lo: Vec3, hi: Vec3) -> bool {
    let (centre, extent) = ((lo + hi) * 0.5, (hi - lo) * 0.5);
    planes(matrix)
        .iter()
        .all(|p| p.truncate().dot(centre) + p.w + p.truncate().abs().dot(extent) >= 0.)
}

/// The texels of a `tile`-texel face drawn with `matrix` that world bounds
/// `lo..hi` reach (`texel_rect`); `None` when a corner is not in front of the
/// light, where the projection cannot bound it.
fn texels(matrix: Mat4, lo: Vec3, hi: Vec3, tile: u32) -> Option<[u32; 4]> {
    let (mut min, mut max) = (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
    for corner in 0..8u32 {
        let pick = |bit: u32, axis: usize| {
            if corner >> bit & 1 == 0 {
                lo[axis]
            } else {
                hi[axis]
            }
        };
        let clip = matrix * Vec3::new(pick(0, 0), pick(1, 1), pick(2, 2)).extend(1.);
        if clip.w < NEAR {
            return None;
        }
        let p = clip.truncate().truncate() / clip.w;
        (min, max) = (min.min(p), max.max(p));
    }
    super::cascades::texel_rect(min, max, tile)
}

/// Whether a sphere reaches the frustum of `matrix`.
fn sphere_visible(planes: &[Vec4; 6], centre: Vec3, radius: f32) -> bool {
    planes.iter().all(|p| {
        let n = p.truncate();
        n.dot(centre) + p.w >= -radius * n.length()
    })
}

/// One atlas tile: which light holds it, with what matrix, and what it holds.
#[derive(Clone, Copy)]
struct Face {
    owner: Option<Key>,
    matrix: Mat4,
    /// The matrix the tile was drawn with; `None` when it must be drawn.
    drawn: Option<Mat4>,
    /// Texels changed casters reach in the drawn tile: it needs a patch.
    dirty: Option<[u32; 4]>,
}

/// One face redraw this frame: its whole tile, or (`patch`) only the texels
/// moved casters reach, through the cropped matrix the table gives that tile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LocalDraw {
    pub face: usize,
    /// The face's view-projection.
    pub matrix: Mat4,
    pub patch: Option<Patch>,
}

/// This frame's local-shadow work.
pub(crate) struct LocalPlan {
    /// Faces to draw.
    pub draws: Vec<LocalDraw>,
    /// The shading table, uploaded whole each frame.
    pub table: Vec<u8>,
}

/// Which lights hold which tiles, and what each tile holds.
pub(crate) struct LocalCache {
    faces: Vec<Face>,
    settings: LocalShadows,
    /// Patches drawn since creation (diagnostics and tests).
    pub patched: u64,
}

impl LocalCache {
    pub(crate) fn new(settings: LocalShadows) -> Self {
        Self {
            faces: vec![
                Face {
                    owner: None,
                    matrix: Mat4::IDENTITY,
                    drawn: None,
                    dirty: None,
                };
                capacity(settings)
            ],
            settings,
            patched: 0,
        }
    }

    /// Marks the texels of the tiles whose frustum holds changed casters
    /// (`bounds`: their old and new world bounds), or forgets those it cannot
    /// bound and, for new content, every tile.
    pub(crate) fn observe(&mut self, all: bool, bounds: &[[Vec3; 2]]) {
        let tile = self.settings.resolution;
        for face in &mut self.faces {
            let Some(matrix) = face.drawn else {
                continue;
            };
            for &[lo, hi] in bounds.iter().filter(|_| !all) {
                if !overlaps(matrix, lo, hi) {
                    continue;
                }
                match texels(matrix, lo, hi, tile) {
                    Some(rect) => face.dirty = Some(super::cascades::union(face.dirty, rect)),
                    None => {
                        face.drawn = None;
                        break;
                    }
                }
            }
            if all
                || face
                    .dirty
                    .is_some_and(|r| !super::cascades::patchable(r, tile))
            {
                face.drawn = None;
            }
            if face.drawn.is_none() {
                face.dirty = None;
            }
        }
    }

    /// Picks this frame's shadowed lights (the `lights` budget's brightest at the
    /// camera among those reaching its view, held lights favoured), assigns their
    /// tiles, and plans the redraws (all of them when `unlimited`, for captures).
    pub(crate) fn plan(
        &mut self,
        lights: &[Light],
        view_projection: Mat4,
        camera: Vec3,
        atlas: [u32; 2],
        unlimited: bool,
    ) -> LocalPlan {
        let view = planes(view_projection);
        let held: BTreeSet<Key> = self.faces.iter().filter_map(|f| f.owner).collect();
        let mut ranked: Vec<(f32, usize)> = lights
            .iter()
            .enumerate()
            .take(MAX_LOCAL_LIGHTS)
            .filter_map(|(i, light)| {
                let position = Vec3::from_slice(&light.position_range[..3]);
                if !sphere_visible(&view, position, light.position_range[3]) {
                    return None;
                }
                let favour = if held.contains(&key(light)) { 1.5 } else { 1. };
                let distance = position.distance_squared(camera).max(1.);
                Some((light.color_lumens[3] / distance * favour, i))
            })
            .collect();
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
        ranked.truncate(self.settings.lights as usize);
        let selected: Vec<(usize, Key, Vec<Mat4>)> = ranked
            .iter()
            .map(|&(_, i)| (i, key(&lights[i]), faces(&lights[i])))
            .collect();
        let keep: BTreeSet<Key> = selected.iter().map(|(_, k, _)| *k).collect();
        // Tiles of lights no longer selected are free; never-used ones go first.
        let mut free: Vec<usize> = (0..self.faces.len())
            .filter(|&f| self.faces[f].owner.is_none_or(|k| !keep.contains(&k)))
            .collect();
        free.sort_by_key(|&f| self.faces[f].owner.is_some());
        free.reverse();
        let mut assigned: Vec<(usize, Vec<usize>)> = Vec::new();
        for (light, k, matrices) in &selected {
            let mut mine: Vec<usize> = (0..self.faces.len())
                .filter(|&f| self.faces[f].owner == Some(*k))
                .collect();
            if mine.len() != matrices.len() {
                for &f in &mine {
                    self.faces[f].owner = None;
                    self.faces[f].drawn = None;
                    self.faces[f].dirty = None;
                    free.push(f);
                }
                if free.len() < matrices.len() {
                    continue;
                }
                mine = free.split_off(free.len() - matrices.len());
                // Ascending, as later frames find them: face k keeps matrix k.
                mine.sort_unstable();
                for &f in &mine {
                    self.faces[f].owner = Some(*k);
                    self.faces[f].drawn = None;
                    self.faces[f].dirty = None;
                }
            }
            for (&f, &matrix) in mine.iter().zip(matrices) {
                self.faces[f].matrix = matrix;
            }
            assigned.push((*light, mine));
        }
        // Redraws in light order, within the frame's budget.
        let budget = if unlimited {
            usize::MAX
        } else {
            self.settings.redraws as usize
        };
        let tile = self.settings.resolution;
        let assigned_faces = || assigned.iter().flat_map(|(_, faces)| faces.iter().copied());
        let mut draws: Vec<LocalDraw> = assigned_faces()
            .filter(|&f| self.faces[f].drawn != Some(self.faces[f].matrix))
            .take(budget)
            .map(|f| LocalDraw {
                face: f,
                matrix: self.faces[f].matrix,
                patch: None,
            })
            .collect();
        // Moved casters in otherwise current tiles: patches, beyond the budget.
        draws.extend(
            assigned_faces()
                .filter_map(|f| {
                    let face = &self.faces[f];
                    let rect = face.dirty.filter(|_| face.drawn == Some(face.matrix))?;
                    Some(LocalDraw {
                        face: f,
                        matrix: face.matrix,
                        patch: Some(Patch::new(rect, tile, face.matrix)),
                    })
                })
                .take(PATCHES),
        );
        // A light shades with its faces once all of them hold its current matrix.
        let ready = |f: usize| {
            self.faces[f].drawn == Some(self.faces[f].matrix) || draws.iter().any(|d| d.face == f)
        };
        let mut entries = vec![[u32::MAX, 0u32]; MAX_LOCAL_LIGHTS];
        let mut records: Vec<u8> = Vec::new();
        for (light, faces) in &assigned {
            if !faces.iter().all(|&f| ready(f)) {
                continue;
            }
            entries[*light] = [(records.len() / RECORD_BYTES) as u32, faces.len() as u32];
            for &f in faces {
                let origin = origin(f, tile);
                let rect = [
                    origin[0] as f32 / atlas[0] as f32,
                    origin[1] as f32 / atlas[1] as f32,
                    tile as f32 / atlas[0] as f32,
                    tile as f32 / atlas[1] as f32,
                ];
                records
                    .extend_from_slice(bytemuck::cast_slice(&self.faces[f].matrix.to_cols_array()));
                records.extend_from_slice(bytemuck::cast_slice(&rect));
            }
        }
        let mut table = bytemuck::cast_slice(&entries).to_vec();
        let mut matrices = vec![Mat4::IDENTITY; MAX_FACES];
        for (slot, face) in matrices.iter_mut().zip(&self.faces) {
            *slot = face.matrix;
        }
        for draw in &draws {
            if let Some(patch) = draw.patch {
                matrices[draw.face] = patch.matrix;
            }
        }
        table.extend(
            matrices
                .iter()
                .flat_map(|m| bytemuck::bytes_of(&m.to_cols_array()).to_vec()),
        );
        table.extend_from_slice(&records);
        LocalPlan { draws, table }
    }

    /// `draw` landed: a whole tile now holds its matrix; a patch only brings
    /// the drawn one up to date.
    pub(crate) fn drawn(&mut self, draw: &LocalDraw) {
        if let Some(f) = self.faces.get_mut(draw.face) {
            f.dirty = None;
            if draw.patch.is_some() {
                self.patched += 1;
            } else {
                f.drawn = Some(draw.matrix);
            }
        }
    }
}

/// Tiles a budget needs: six per shadowed light.
pub(crate) fn capacity(settings: LocalShadows) -> usize {
    settings.lights as usize * 6
}

/// Atlas texels for a budget: `ROW` tiles per row.
pub(crate) fn atlas_size(settings: LocalShadows) -> [u32; 2] {
    let rows = (capacity(settings) as u32).div_ceil(ROW);
    [ROW * settings.resolution, rows * settings.resolution]
}

/// Top-left texel of tile `face`.
pub(crate) fn origin(face: usize, resolution: u32) -> [u32; 2] {
    let face = face as u32;
    [face % ROW * resolution, face / ROW * resolution]
}

/// The local-shadow atlas and its table.
pub(crate) struct LocalMap {
    pub atlas: Image,
    pub sampled: u32,
    pub state: State,
    pub size: [u32; 2],
    pub settings: LocalShadows,
    /// Host-written table (`LocalPlan::table`), one bindless storage slot.
    pub table: Buffer,
    pub table_index: u32,
    pub cache: LocalCache,
}

impl LocalMap {
    pub(crate) fn new(gpu: &mut GpuDevice, settings: LocalShadows) -> Result<Self> {
        let size = atlas_size(settings);
        let atlas = gpu.create_image(
            size[0],
            size[1],
            vk::Format::D32_SFLOAT,
            vk::ImageUsageFlags::SAMPLED
                | vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
        )?;
        let mut buffers = Vec::new();
        let parts = (|| {
            let sampled = gpu.sampled_image(atlas, true)?.index;
            let table = gpu.create_buffer(
                (RECORDS + capacity(settings) * RECORD_BYTES) as u64,
                vk::BufferUsageFlags::STORAGE_BUFFER,
                true,
            )?;
            buffers.push(table);
            let table_index = gpu.storage_buffer(table)?.index;
            Ok::<_, anyhow::Error>((sampled, table, table_index))
        })();
        let (sampled, table, table_index) = match parts {
            Ok(parts) => parts,
            Err(error) => {
                gpu.retire(Resource::Image(atlas))?;
                for buffer in buffers {
                    gpu.retire(Resource::Buffer(buffer))?;
                }
                gpu.collect()?;
                return Err(error);
            }
        };
        Ok(Self {
            atlas,
            sampled,
            state: State {
                access: Access::Undefined,
                queue: Queue::Graphics,
            },
            size,
            settings,
            table,
            table_index,
            cache: LocalCache::new(settings),
        })
    }

    pub(crate) fn destroy(self, gpu: &mut GpuDevice) -> Result<()> {
        gpu.retire(Resource::Image(self.atlas))?;
        gpu.retire(Resource::Buffer(self.table))?;
        gpu.collect()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(x: f32, lumens: f32) -> Light {
        Light {
            position_range: [x, 3., 0., 10.],
            color_lumens: [1., 1., 1., lumens],
            direction_outer: [0., -1., 0., 0.],
            parameters: [0., 1., 0., 0.],
        }
    }

    fn settings(lights: u32, redraws: u32) -> LocalShadows {
        LocalShadows {
            lights,
            resolution: 256,
            redraws,
            in_gi_and_fog: false,
        }
    }

    fn camera() -> Mat4 {
        Mat4::perspective_infinite_reverse_rh(1.2, 1., 0.1)
            * Mat4::look_at_rh(Vec3::new(0., 2., 12.), Vec3::new(0., 2., 0.), Vec3::Y)
    }

    fn entry(plan: &LocalPlan, light: usize) -> [u32; 2] {
        let words: &[u32] = bytemuck::cast_slice(&plan.table[..RECORDS]);
        [words[light * 2], words[light * 2 + 1]]
    }

    #[test]
    fn the_brightest_lights_in_view_get_faces_and_shade_once_drawn() {
        let s = settings(1, 4);
        let atlas = atlas_size(s);
        let mut cache = LocalCache::new(s);
        let lights = [point(0., 100.), point(2., 2000.), point(0., 5000.)];
        // All three are in view; the brightest at the camera wins the one slot.
        let plan = cache.plan(&lights, camera(), Vec3::new(0., 2., 12.), atlas, false);
        assert_eq!(plan.draws.len(), 4, "a cube's first four faces this frame");
        assert_eq!(entry(&plan, 2)[1], 0, "unshadowed until all six are drawn");
        for draw in &plan.draws {
            cache.drawn(draw);
        }
        let plan = cache.plan(&lights, camera(), Vec3::new(0., 2., 12.), atlas, false);
        assert_eq!(plan.draws.len(), 2, "the rest of the cube");
        assert_eq!(entry(&plan, 2), [0, 6], "shadowed with all six faces");
        assert_eq!(entry(&plan, 1)[1], 0, "over budget");
        for draw in &plan.draws {
            cache.drawn(draw);
        }
        let plan = cache.plan(&lights, camera(), Vec3::new(0., 2., 12.), atlas, false);
        assert!(plan.draws.is_empty(), "a still scene draws nothing");
        assert_eq!(plan.table.len(), RECORDS + 6 * RECORD_BYTES);
        let tiles: &[f32] = bytemuck::cast_slice(&plan.table[FACES..RECORDS]);
        assert_eq!(
            &tiles[..16],
            &faces(&lights[2])[0].to_cols_array(),
            "tile 0's draw matrix"
        );
    }

    #[test]
    fn changed_casters_redraw_only_the_faces_they_reach() {
        let s = settings(1, 6);
        let atlas = atlas_size(s);
        let mut cache = LocalCache::new(s);
        let lights = [point(0., 1000.)];
        let plan = cache.plan(&lights, camera(), Vec3::new(0., 2., 12.), atlas, true);
        for draw in &plan.draws {
            cache.drawn(draw);
        }
        // A box right below the light reaches only the -Y face, and patches it.
        cache.observe(
            false,
            &[[Vec3::new(-0.5, 0., -0.5), Vec3::new(0.5, 0.5, 0.5)]],
        );
        let plan = cache.plan(&lights, camera(), Vec3::new(0., 2., 12.), atlas, false);
        assert_eq!(plan.draws.len(), 1, "{:?}", plan.draws.len());
        let draw = plan.draws[0];
        let patch = draw.patch.expect("a patch");
        assert!(patch.size < 128, "{patch:?}");
        let tiles: &[f32] = bytemuck::cast_slice(&plan.table[FACES..RECORDS]);
        assert_eq!(
            &tiles[draw.face * 16..draw.face * 16 + 16],
            &patch.matrix.to_cols_array(),
            "the patched tile draws through its cropped matrix"
        );
        cache.drawn(&draw);
        assert_eq!(cache.patched, 1);
        let plan = cache.plan(&lights, camera(), Vec3::new(0., 2., 12.), atlas, false);
        assert!(plan.draws.is_empty());
        // Casters behind a face's eye plane, or across most of it, redraw it whole.
        cache.observe(false, &[[Vec3::new(-4., -1., -4.), Vec3::new(4., 2.9, 4.)]]);
        let plan = cache.plan(&lights, camera(), Vec3::new(0., 2., 12.), atlas, false);
        assert!(!plan.draws.is_empty() && plan.draws.iter().all(|d| d.patch.is_none()));
        for draw in &plan.draws {
            cache.drawn(draw);
        }
        cache.observe(true, &[]);
        let plan = cache.plan(&lights, camera(), Vec3::new(0., 2., 12.), atlas, true);
        assert_eq!(plan.draws.len(), 6, "new content redraws every face");
    }

    #[test]
    fn a_narrow_spot_uses_one_face_and_a_moved_light_redraws() {
        let s = settings(2, 12);
        let atlas = atlas_size(s);
        let mut cache = LocalCache::new(s);
        let mut spot = point(0., 1000.);
        spot.parameters[0] = 1.;
        spot.direction_outer[3] = 0.8;
        assert_eq!(faces(&spot).len(), 1);
        let plan = cache.plan(&[spot], camera(), Vec3::new(0., 2., 12.), atlas, false);
        assert_eq!(plan.draws.len(), 1);
        for draw in &plan.draws {
            cache.drawn(draw);
        }
        spot.position_range[0] = 1.;
        let plan = cache.plan(&[spot], camera(), Vec3::new(0., 2., 12.), atlas, false);
        assert_eq!(plan.draws.len(), 1, "the moved spot's face");
        // The view-projection keeps points in front of the light inside its frustum.
        let below = Vec3::new(1., 0., 0.);
        assert!(overlaps(plan.draws[0].matrix, below - 0.1, below + 0.1));
        assert!(!overlaps(
            plan.draws[0].matrix,
            Vec3::new(1., 5.9, 0.),
            Vec3::new(1., 6.1, 0.)
        ));
    }
}
