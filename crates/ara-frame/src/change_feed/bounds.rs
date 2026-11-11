use glam::{Mat4, Vec3};

/// Finite ordered axis-aligned bounds in metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    /// Minimum corner.
    pub min: Vec3,
    /// Maximum corner.
    pub max: Vec3,
}
impl Aabb {
    /// Construct bounds; feed entry points check finiteness and order.
    pub fn new(min: Vec3, max: Vec3) -> Self {
        Self { min, max }
    }
    /// Per-axis size.
    pub fn extent(self) -> Vec3 {
        self.max - self.min
    }
    /// Conservative union.
    pub fn union(self, other: Self) -> Self {
        Self::new(self.min.min(other.min), self.max.max(other.max))
    }
    /// Expand by a nonnegative radius.
    pub fn expanded(self, radius: f32) -> Self {
        Self::new(
            self.min - Vec3::splat(radius),
            self.max + Vec3::splat(radius),
        )
    }
    /// Affine AABB transform, including shear, mirrors and nonuniform scale.
    pub fn transformed(self, world: Mat4) -> Self {
        let centre = world.transform_point3((self.min + self.max) * 0.5);
        let half = self.extent() * 0.5;
        let radius = world.x_axis.truncate().abs() * half.x
            + world.y_axis.truncate().abs() * half.y
            + world.z_axis.truncate().abs() * half.z;
        Self::new(centre - radius, centre + radius)
    }
    pub(super) fn valid(self) -> bool {
        self.min.is_finite() && self.max.is_finite() && self.min.cmple(self.max).all()
    }
    pub(super) fn corners(self) -> impl Iterator<Item = Vec3> {
        (0..8).map(move |i| {
            Vec3::new(
                if i & 1 == 0 { self.min.x } else { self.max.x },
                if i & 2 == 0 { self.min.y } else { self.max.y },
                if i & 4 == 0 { self.min.z } else { self.max.z },
            )
        })
    }
}
/// Conservative affine motion summary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotionBounds {
    /// Supported motion path, not just endpoint boxes.
    pub swept: Aabb,
    /// Maximum endpoint point displacement, including deformation and wind.
    pub displacement: f32,
}
/// Bound affine endpoints and continuous rotation around the local origin.
/// Translation follows its straight segment; intermediate linear-operator norm must not exceed
/// the endpoint Frobenius bound (true for SRT with linear scale and arbitrary rotation, and
/// affine matrix interpolation). Curved translation or scale overshoot requires subdivision.
/// The sphere sweep covers rotation interiors missed by endpoint AABB unions. Displacement is
/// the maximum affine endpoint difference on old box corners plus transformed deformation and
/// both wind amplitudes. It measures endpoint change, not distance travelled during a full turn.
/// Deforming vertices follow their uploaded endpoint segments; nonlinear skin trajectories
/// require a host-supplied bound covering that path or subdivided vertex revisions.
pub fn motion_bounds(
    previous: super::InstanceState,
    current: super::InstanceState,
    local_displacement: f32,
) -> MotionBounds {
    let a = previous.world;
    let b = current.world;
    let scale = linear_norm(a).max(linear_norm(b));
    let local = previous.bounds.union(current.bounds);
    let mut swept = previous.world_bounds().union(current.world_bounds());
    if a.x_axis != b.x_axis || a.y_axis != b.y_axis || a.z_axis != b.z_axis {
        let radius = local.corners().map(Vec3::length).fold(0., f32::max) * scale
            + previous.wind.max(current.wind);
        let p = a.w_axis.truncate();
        let q = b.w_axis.truncate();
        swept = swept.union(Aabb::new(p.min(q), p.max(q)).expanded(radius));
    }
    let deformation = local_displacement * scale;
    swept = swept.expanded(deformation);
    let rigid = previous
        .bounds
        .corners()
        .map(|p| (b.transform_point3(p) - a.transform_point3(p)).length())
        .fold(0., f32::max);
    let displacement = rigid + deformation + previous.wind + current.wind;
    let padding = 16. * f32::EPSILON * (1. + swept.min.abs().max(swept.max.abs()).max_element());
    MotionBounds {
        swept: swept.expanded(padding),
        displacement: if displacement == 0. {
            0.
        } else {
            displacement + padding
        },
    }
}
/// Frobenius norm bounds shear and is invariant under rotation of SRT endpoints.
pub(super) fn linear_norm(world: Mat4) -> f32 {
    (world.x_axis.truncate().length_squared()
        + world.y_axis.truncate().length_squared()
        + world.z_axis.truncate().length_squared())
    .sqrt()
}
