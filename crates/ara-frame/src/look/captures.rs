//! Reflection-capture specs and automatic placement. Baking stays in `ara-render`.
use anyhow::{Result, ensure};
use glam::{Mat3, Mat4, Quat, Vec3, Vec4};

/// Bytes of one `CaptureRecord` (look-captures.slangh).
pub const RECORD_BYTES: usize = 128;

/// Most captures a renderer keeps (atlas columns; the relight dispatch's width).
pub const MAX_CAPTURES: usize = 16;
/// One reflection capture in world space, as the renderer bakes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaptureSpec {
    /// Origin of the capture's local frame (metres): the influence box's centre.
    pub position: Vec3,
    /// Orientation of the local frame (world from local); boxes follow its axes.
    pub rotation: Quat,
    /// Influence box half-extents (local axes, metres): surfaces inside reflect
    /// this capture.
    pub influence: Vec3,
    /// Distance inside the influence box over which the capture fades out (metres).
    pub blend: f32,
    /// Projection (parallax) box centre relative to `position` (local axes).
    pub projection_centre: Vec3,
    /// Projection box half-extents (local axes, metres): reflected directions are
    /// looked up where they leave it, as seen from the capture point.
    pub projection: Vec3,
    /// Capture point relative to `position` (local axes); keep it in open space.
    pub capture_offset: Vec3,
    /// Where captures overlap, higher priorities blend first (then smaller boxes).
    pub priority: i32,
    /// Level-0 interior texels per side of the octahedral map (a power of two,
    /// 64 to 1024); zero uses the quality's.
    pub resolution: u32,
    /// Radiance multiplier.
    pub intensity: f32,
    /// Stable identity of an authored probe (its entity's render ID); zero for
    /// automatic placements.
    pub key: u64,
    /// Bakes only when requested ([`crate::Renderer::request_reflection_capture`]) or
    /// restored from a cache ([`crate::Renderer::load_reflection_capture`]), never on
    /// content or lighting changes: an authored probe's image then always matches a
    /// receipt, and goes stale instead of changing under it. Automatic placements bake
    /// themselves.
    pub explicit: bool,
}

impl CaptureSpec {
    /// World position the faces are rendered from.
    pub fn capture_point(&self) -> Vec3 {
        self.position + self.rotation * self.capture_offset
    }

    /// Rejects values the shaders cannot use.
    pub fn validate(&self) -> Result<()> {
        let finite = [
            self.position,
            self.influence,
            self.projection_centre,
            self.projection,
            self.capture_offset,
        ]
        .iter()
        .all(|v| v.is_finite());
        ensure!(
            finite
                && self.rotation.is_finite()
                && self.rotation.length() > 0.5
                && self.influence.min_element() > 0.
                && self.projection.min_element() > 0.
                && self.blend.is_finite()
                && self.blend >= 0.
                && (self.resolution == 0
                    || (self.resolution.is_power_of_two()
                        && (64..=1024).contains(&self.resolution)))
                && self.intensity.is_finite()
                && self.intensity >= 0.,
            "Invalid reflection capture"
        );
        Ok(())
    }

    /// Whether the influence box may meet the frustum of `view_projection`: false
    /// only when all its corners lie beyond one clip plane (sides or behind the eye).
    pub fn in_view(&self, view_projection: Mat4) -> bool {
        let corners = (0..8).map(|i| {
            let sign = Vec3::new(
                if i & 1 == 0 { -1. } else { 1. },
                if i & 2 == 0 { -1. } else { 1. },
                if i & 4 == 0 { -1. } else { 1. },
            );
            view_projection * (self.position + self.rotation * (self.influence * sign)).extend(1.)
        });
        let mut outside = [true; 5];
        for c in corners {
            let planes: [fn(Vec4) -> bool; 5] = [
                |c| c.x < -c.w,
                |c| c.x > c.w,
                |c| c.y < -c.w,
                |c| c.y > c.w,
                |c| c.w <= 0.,
            ];
            for (out, plane) in outside.iter_mut().zip(planes) {
                *out &= plane(c);
            }
        }
        !outside.contains(&true)
    }

    /// Influence-box volume, used to prefer smaller captures where they overlap.
    pub fn volume(&self) -> f32 {
        self.influence.x * self.influence.y * self.influence.z
    }

    /// The shader record (look-captures.slangh `CaptureRecord`) for atlas column
    /// `column` (texel origin) with `resolution` level-0 interior texels.
    pub fn record(&self, column: [u32; 2], resolution: u32) -> [u8; RECORD_BYTES] {
        let rotation = Mat3::from_quat(self.rotation.normalize());
        let mut floats = [0f32; 28];
        // World to local: the transpose's rows are the rotation's columns.
        for i in 0..3 {
            let axis = rotation.col(i);
            floats[i * 4..i * 4 + 3].copy_from_slice(&axis.to_array());
            floats[i * 4 + 3] = -axis.dot(self.position);
        }
        floats[12..15].copy_from_slice(&self.influence.to_array());
        floats[15] = self.blend;
        floats[16..19].copy_from_slice(&self.projection_centre.to_array());
        floats[20..23].copy_from_slice(&self.projection.to_array());
        floats[23] = self.intensity;
        floats[24..27].copy_from_slice(&self.capture_point().to_array());
        let mut bytes = [0u8; RECORD_BYTES];
        bytes[..112].copy_from_slice(bytemuck::cast_slice(&floats));
        let tile = [column[0], column[1], resolution, 0];
        bytes[112..].copy_from_slice(bytemuck::cast_slice(&tile));
        bytes
    }
}

/// Reflection captures for content without authored ones: one over the static
/// enclosure (up to 200 m across), taken from its most open point near the largest
/// cluster of smooth objects (metals, glossy and clear surfaces), and one more over
/// each other cluster far from that point, where an open point exists near it. Open
/// points keep at least [`OPEN`] metres from every object (and from walls and
/// ceilings), so no neighbour fills a capture's view. All project onto the
/// enclosure's box, its floor at the widest thin slab's top. At most `limit`.
pub fn auto_place(scene: &crate::instanced::Scene, limit: usize) -> Vec<CaptureSpec> {
    let boxes: Vec<_> = scene
        .instances
        .iter()
        .filter_map(|instance| {
            let bounds = instance.bounds(scene.geometry.get(instance.geometry)?);
            let material = scene.materials.get(instance.material)?;
            let (roughness, metal) = (material.pbr.emissive_roughness[3], material.pbr.surface[0]);
            let smooth =
                roughness <= 0.4 || metal >= 0.5 || material.extras.transmission_factor > 0.;
            Some((bounds, smooth))
        })
        .collect();
    place(&boxes, limit)
}

fn union(a: [Vec3; 2], b: [Vec3; 2]) -> [Vec3; 2] {
    [a[0].min(b[0]), a[1].max(b[1])]
}

/// Clearance (m) that makes a capture point open: nearer geometry would fill its view.
pub const OPEN: f32 = 1.5;
/// Capture heights above the floor that candidates sample (eye level first).
const HEIGHTS: [f32; 4] = [1.6, 1.2, 2.0, 2.5];

/// Distance from `p` to box `b` (zero inside).
pub fn outside(b: &[Vec3; 2], p: Vec3) -> f32 {
    (b[0] - p).max(p - b[1]).max(Vec3::ZERO).length()
}

/// [`auto_place`] over instance bounds and whether each is smooth.
pub fn place(boxes: &[([Vec3; 2], bool)], limit: usize) -> Vec<CaptureSpec> {
    let valid: Vec<_> = boxes
        .iter()
        .copied()
        .filter(|(b, _)| b[0].is_finite() && b[1].is_finite() && b[0].cmple(b[1]).all())
        .collect();
    let Some(enclosure) = valid.iter().map(|(b, _)| *b).reduce(union) else {
        return Vec::new();
    };
    let size = enclosure[1] - enclosure[0];
    let footprint = size.x.max(0.01) * size.z.max(0.01);
    let area = |b: &[Vec3; 2]| (b[1].x - b[0].x) * (b[1].z - b[0].z);
    // The floor: the widest thin slab spanning a quarter of the footprint.
    let slab = valid
        .iter()
        .map(|(b, _)| *b)
        .filter(|b| {
            let s = b[1] - b[0];
            area(b) >= footprint * 0.25 && s.y <= 0.1 * s.x.min(s.z)
        })
        .max_by(|a, b| area(a).total_cmp(&area(b)));
    let floor = slab.map_or(enclosure[0].y, |b| b[1].y);
    // Above the highest capture point: open scenes project the sky high enough.
    let top = enclosure[1].y.max(floor + HEIGHTS[3] + 0.5);
    // Clearance from geometry: objects by their bounds; shells spanning the enclosure
    // (rooms, terrain) by their faces from inside. The floor slab is the projection
    // box's own floor and costs nothing. Very large scenes score their largest 2048.
    let mut solids: Vec<([Vec3; 2], bool)> = valid
        .iter()
        .filter(|(b, _)| Some(*b) != slab)
        .map(|(b, _)| (*b, area(b) >= footprint * 0.25))
        .collect();
    let volume = |b: &[Vec3; 2]| (b[1] - b[0]).max(Vec3::splat(0.01)).element_product();
    solids.sort_by(|a, b| volume(&b.0).total_cmp(&volume(&a.0)));
    solids.truncate(2048);
    let clearance = |p: Vec3| {
        let mut nearest = OPEN;
        for (b, shell) in &solids {
            let inside = p.cmpge(b[0]).all() && p.cmple(b[1]).all();
            let d = if inside && *shell {
                (p - b[0]).min(b[1] - p).min_element()
            } else {
                outside(b, p)
            };
            nearest = nearest.min(d);
        }
        (p.y - floor).min(nearest).max(0.)
    };
    // The most open point of a horizontal region (clearance capped at OPEN), nearest
    // `anchor` and eye level among equals.
    let open = |region: [Vec3; 2], anchor: Vec3| {
        let mut best: Option<(f32, f32, Vec3)> = None;
        for height in HEIGHTS {
            let y = floor + height;
            for i in 0..=16 {
                for k in 0..=16 {
                    let p = Vec3::new(
                        region[0].x + (region[1].x - region[0].x) * i as f32 / 16.,
                        y,
                        region[0].z + (region[1].z - region[0].z) * k as f32 / 16.,
                    );
                    let score = clearance(p);
                    let away = Vec3::new(p.x - anchor.x, 0., p.z - anchor.z).length()
                        + 0.5 * (height - HEIGHTS[0]).abs();
                    let better = best
                        .is_none_or(|(s, a, _)| score > s + 1e-3 || (score > s - 1e-3 && away < a));
                    if better {
                        best = Some((score, away, p));
                    }
                }
            }
        }
        best.map(|(score, _, p)| (score, p))
    };
    let projection = [
        Vec3::new(enclosure[0].x, floor, enclosure[0].z),
        Vec3::new(enclosure[1].x, top, enclosure[1].z),
    ];
    let spec = |influence: [Vec3; 2], blend: f32, point: Vec3, priority: i32| {
        let position = (influence[0] + influence[1]) * 0.5;
        CaptureSpec {
            position,
            rotation: Quat::IDENTITY,
            influence: (influence[1] - influence[0]) * 0.5,
            blend,
            projection_centre: (projection[0] + projection[1]) * 0.5 - position,
            projection: ((projection[1] - projection[0]) * 0.5).max(Vec3::splat(0.05)),
            capture_offset: point - position,
            priority,
            resolution: 0,
            intensity: 1.,
            key: 0,
            explicit: false,
        }
    };
    let objects: Vec<_> = valid
        .iter()
        .filter(|(b, _)| area(b) < footprint * 0.25)
        .copied()
        .collect();
    let smooth: Vec<_> = objects
        .iter()
        .filter(|(_, smooth)| *smooth)
        .map(|(b, _)| *b)
        .collect();
    let centre = |b: &[Vec3; 2]| (b[0] + b[1]) * 0.5;
    let around = |b: [Vec3; 2], margin: f32| {
        [
            (b[0] - margin).max(enclosure[0]),
            (b[1] + margin).min(enclosure[1]),
        ]
    };
    let mut clusters = cluster(&smooth, 1.5, 12.);
    clusters.sort_by(|a, b| area(b).total_cmp(&area(a)));
    // Where reflections matter most: around the largest smooth cluster, else around
    // the objects, else the enclosure's centre.
    let (region, anchor) = match clusters.first() {
        Some(c) => (around(*c, 4.), centre(c)),
        None => {
            let content = objects
                .iter()
                .map(|(b, _)| *b)
                .reduce(union)
                .unwrap_or(enclosure);
            (around(content, 2.), centre(&content))
        }
    };
    let mut specs = Vec::new();
    let mut main = None;
    if size.x.max(size.z) <= 200.
        && limit > 0
        && let Some((_, point)) = open(region, anchor)
    {
        let blend = (0.1 * size.x.min(size.z)).clamp(0.5, 4.);
        specs.push(spec(
            [enclosure[0] - 0.5, enclosure[1] + 0.5],
            blend,
            point,
            0,
        ));
        main = Some(point);
    }
    // Smooth clusters far from the enclosure's capture get their own, where open.
    for c in clusters {
        if specs.len() >= limit {
            break;
        }
        let middle = centre(&c);
        if main.is_some_and(|p: Vec3| Vec3::new(p.x - middle.x, 0., p.z - middle.z).length() < 6.) {
            continue;
        }
        let Some((score, point)) = open(around(c, 3.), middle) else {
            continue;
        };
        if score < OPEN - 1e-3 {
            continue;
        }
        let extent = c[1] - c[0];
        let margin = (0.35 * extent.x.max(extent.z)).clamp(1., 4.);
        let influence = [
            Vec3::new(
                c[0].x - margin,
                (c[0].y - margin).max(floor - 0.1),
                c[0].z - margin,
            ),
            c[1] + margin,
        ];
        specs.push(spec(influence, margin * 0.6, point, 1));
    }
    specs
}

/// Groups boxes whose gap is at most `gap` metres while a group stays within
/// `extent` metres horizontally (boxes first bin into `extent / 3` cells).
fn cluster(boxes: &[[Vec3; 2]], gap: f32, extent: f32) -> Vec<[Vec3; 2]> {
    let cell = extent / 3.;
    let mut cells: Vec<((i32, i32), [Vec3; 2], usize)> = Vec::new();
    for b in boxes {
        let centre = (b[0] + b[1]) * 0.5;
        let key = (
            (centre.x / cell).floor() as i32,
            (centre.z / cell).floor() as i32,
        );
        match cells.iter_mut().find(|(k, _, _)| *k == key) {
            Some((_, bounds, count)) => {
                *bounds = union(*bounds, *b);
                *count += 1;
            }
            None => cells.push((key, *b, 1)),
        }
    }
    // Keep the busiest cells of very large scenes (the merge below is cubic).
    cells.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    cells.truncate(256);
    let mut groups: Vec<[Vec3; 2]> = cells.into_iter().map(|(_, b, _)| b).collect();
    loop {
        let mut merged = None;
        'search: for i in 0..groups.len() {
            for j in i + 1..groups.len() {
                let (a, b) = (groups[i], groups[j]);
                let separation = (a[0] - b[1]).max(b[0] - a[1]).max(Vec3::ZERO);
                let u = union(a, b);
                if separation.length() <= gap && (u[1].x - u[0].x).max(u[1].z - u[0].z) <= extent {
                    merged = Some((i, j, u));
                    break 'search;
                }
            }
        }
        let Some((i, j, u)) = merged else {
            return groups;
        };
        groups[i] = u;
        groups.remove(j);
    }
}

