use crate::{Exclusion, Paint, Rule, Species};
use ara_diag::Diagnostic;

/// Read-only surface sample obtained from evaluated terrain; holes return None.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceSample {
    /// World height in metres.
    pub height: f32,
    /// Unit upward surface normal.
    pub normal: [f32; 3],
    /// Normalized terrain material weights.
    pub weights: [f32; 4],
}
/// Surface sampling contract for deterministic worker placement.
pub trait Surface {
    /// Sample at world X/Z in metres, or None for absent tiles/holes.
    fn sample(&self, position: [f32; 2]) -> Option<SurfaceSample>;
}
impl<F: Fn([f32; 2]) -> Option<SurfaceSample>> Surface for F {
    fn sample(&self, p: [f32; 2]) -> Option<SurfaceSample> {
        self(p)
    }
}

/// Compact 48-byte persistent GPU instance input, stable across tile request order.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Instance {
    /// World XYZ and uniform scale.
    pub position_scale: [f32; 4],
    /// Yaw, wind phase, wind stiffness and bounding sphere radius.
    pub rotation_wind: [f32; 4],
    /// Stable ID low/high, species (0 grass/1 tree/2 rock), caller-owned rule index.
    /// Standalone scatter writes zero; terrain stamps its immutable rule catalog index.
    pub identity: [u32; 4],
}

/// Half-open scatter bounds, allowing independent tiles with no duplicates.
#[derive(Debug, Clone, Copy)]
pub struct Bounds {
    /// Inclusive world X/Z minimum.
    pub min: [f32; 2],
    /// Exclusive world X/Z maximum.
    pub max: [f32; 2],
}

/// Counter-based SplitMix64; all randomness derives from explicit input identity.
pub fn hash(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e3779b97f4a7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}
fn unit(x: u64) -> f32 {
    (hash(x) >> 40) as f32 / (1u32 << 24) as f32
}
fn key(seed: u64, x: i32, z: i32) -> u64 {
    hash(seed ^ hash(x as u32 as u64) ^ hash((z as u32 as u64).wrapping_add(0x100000000)))
}
fn excluded(p: [f32; 2], exclusions: &[Exclusion]) -> bool {
    exclusions
        .iter()
        .any(|e| (p[0] - e.center[0]).powi(2) + (p[1] - e.center[1]).powi(2) < e.radius * e.radius)
}
fn make(rule: &Rule, p: [f32; 2], s: SurfaceSample, id: u64) -> Instance {
    let scale = rule.scale[0] + (rule.scale[1] - rule.scale[0]) * unit(id.wrapping_add(3));
    let (species, radius, stiffness) = match rule.species {
        Species::Grass => (0, 1., 0.6),
        Species::Tree => (1, 15., 0.15),
        Species::Rock => (2, 1.4, 0.),
    };
    Instance {
        position_scale: [p[0], s.height, p[1], scale],
        rotation_wind: [
            unit(id.wrapping_add(4)) * std::f32::consts::TAU,
            unit(id.wrapping_add(5)) * std::f32::consts::TAU,
            stiffness,
            radius * scale,
        ],
        identity: [id as u32, (id >> 32) as u32, species, 0],
    }
}
fn valid_sample(rule: &Rule, s: &SurfaceSample) -> bool {
    let slope = s.normal[1].clamp(0., 1.).acos().to_degrees();
    s.height.is_finite()
        && s.normal.iter().all(|v| v.is_finite())
        && s.weights.iter().all(|v| v.is_finite())
        && s.height >= rule.height[0]
        && s.height <= rule.height[1]
        && slope >= rule.slope[0]
        && slope <= rule.slope[1]
        && s.weights[rule.channel as usize] >= rule.min_weight
}

/// Regenerates one bounded tile. `rule_id` is a stable project-owned rule identity.
/// Output order is global Z/X cell order and no RNG
/// state is shared between tiles; cancelling/retrying produces identical instances.
pub fn scatter(
    rule_id: u64,
    rule: &Rule,
    bounds: Bounds,
    surface: &impl Surface,
    exclusions: &[Exclusion],
) -> Result<Vec<Instance>, Diagnostic> {
    rule.validate()?;
    if bounds
        .min
        .iter()
        .chain(&bounds.max)
        .any(|v| !v.is_finite() || v.abs() > 1e7)
        || (0..2).any(|i| bounds.min[i] >= bounds.max[i])
        || exclusions.iter().any(|e| {
            !e.radius.is_finite() || e.radius < 0. || e.center.iter().any(|v| !v.is_finite())
        })
    {
        return Err(ara_terrain::invalid("invalid scatter bounds/exclusions"));
    }
    let lo = bounds.min.map(|v| (v / rule.cell_size).floor() as i32);
    let hi = bounds.max.map(|v| (v / rule.cell_size).ceil() as i32);
    if i64::from(hi[0] - lo[0]) * i64::from(hi[1] - lo[1]) > 4_194_304 {
        return Err(ara_terrain::invalid(
            "scatter job exceeds candidate budget; tile the request",
        ));
    }
    let mut instances = Vec::new();
    if !rule.enabled {
        return Ok(instances);
    }
    for z in lo[1]..hi[1] {
        for x in lo[0]..hi[0] {
            let id = key(rule.seed ^ hash(rule_id), x, z);
            let p = [
                (x as f32 + unit(id)) * rule.cell_size,
                (z as f32 + unit(id.wrapping_add(1))) * rule.cell_size,
            ];
            if (0..2).any(|i| p[i] < bounds.min[i] || p[i] >= bounds.max[i])
                || excluded(p, exclusions)
            {
                continue;
            }
            let Some(s) = surface.sample(p) else {
                continue;
            };
            if !valid_sample(rule, &s) {
                continue;
            }
            let noise = 1. - rule.noise
                + rule.noise * unit(key(rule.seed ^ 0xabcd, x.div_euclid(8), z.div_euclid(8)));
            let probability = rule.density
                * rule.cell_size
                * rule.cell_size
                * s.weights[rule.channel as usize].clamp(0., 1.)
                * noise;
            if unit(id.wrapping_add(2)) < probability {
                instances.push(make(rule, p, s, id));
            }
        }
    }
    Ok(instances)
}

/// Extracts a manual gesture using the same filters, holes and exclusions as scatter.
/// `stroke_key` must be a stable hash of the document's paint record ID.
pub fn painted(
    rule: &Rule,
    paint: &Paint,
    stroke_key: u64,
    surface: &impl Surface,
    exclusions: &[Exclusion],
) -> Result<Vec<Instance>, Diagnostic> {
    rule.validate()?;
    if paint.points.len() > 4096 || paint.points.iter().flatten().any(|v| !v.is_finite()) {
        return Err(ara_terrain::invalid("invalid paint gesture"));
    }
    let mut out = Vec::new();
    for (i, p) in paint.points.iter().enumerate() {
        if excluded(*p, exclusions) {
            continue;
        }
        if let Some(s) = surface.sample(*p)
            && valid_sample(rule, &s)
        {
            out.push(make(rule, *p, s, hash(paint.seed ^ stroke_key ^ i as u64)));
        }
    }
    Ok(out)
}

/// Complementary screen-door transition; depth, shadow and colour use one threshold.
pub fn lod_visible(noise: f32, blend: f32, coarse: bool) -> bool {
    if coarse {
        noise < blend.clamp(0., 1.)
    } else {
        noise >= blend.clamp(0., 1.)
    }
}
