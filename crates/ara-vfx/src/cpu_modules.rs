use crate::{DepthQuery, Module, Particle, hash};
use glam::{Vec3, Vec4};

fn random(rng: &mut u32) -> f32 {
    *rng = hash(rng.wrapping_add(0x9e3779b9));
    (*rng >> 8) as f32 / 16_777_216.
}
fn random3(rng: &mut u32) -> Vec3 {
    Vec3::new(random(rng), random(rng), random(rng))
}
/// Apply one validated initializer to a freshly seeded particle.
pub(crate) fn initialize(m: &Module, p: &mut Particle, rng: &mut u32) {
    match m {
        Module::Lifetime {
            seconds,
            size,
            color,
        } => {
            p.life = *seconds;
            p.size = *size;
            p.color = Vec4::from(*color);
        }
        Module::Point { position } => p.position = Vec3::from(*position),
        Module::Sphere { center, radius } => {
            let r = random3(rng);
            let z = 2. * r.x - 1.;
            let a = std::f32::consts::TAU * r.y;
            let xy = (1. - z * z).max(0.).sqrt();
            p.position = Vec3::from(*center)
                + *radius * r.z.powf(1. / 3.) * Vec3::new(xy * a.cos(), z, xy * a.sin());
        }
        Module::Box { center, extent } => {
            p.position = Vec3::from(*center) + (2. * random3(rng) - Vec3::ONE) * Vec3::from(*extent)
        }
        Module::Cone {
            origin,
            height,
            radius,
        } => {
            let r = random3(rng);
            let h = r.x.powf(1. / 3.);
            let rad = r.y.sqrt() * h * radius;
            let a = std::f32::consts::TAU * r.z;
            p.position = Vec3::from(*origin) + Vec3::new(rad * a.cos(), h * height, rad * a.sin());
        }
        Module::Velocity { value, spread } => {
            p.velocity = Vec3::from(*value) + (2. * random3(rng) - Vec3::ONE) * Vec3::from(*spread)
        }
        Module::MeshSurface { triangles } => {
            let area = |t: &[[f32; 3]; 3]| {
                (Vec3::from(t[1]) - Vec3::from(t[0]))
                    .cross(Vec3::from(t[2]) - Vec3::from(t[0]))
                    .length()
            };
            let mut pick = random(rng) * triangles.iter().map(area).sum::<f32>();
            let mut selected = &triangles[triangles.len() - 1];
            for t in triangles {
                pick -= area(t);
                if pick <= 0. {
                    selected = t;
                    break;
                }
            }
            let u = random(rng).sqrt();
            let v = random(rng);
            p.position = (1. - u) * Vec3::from(selected[0])
                + u * (1. - v) * Vec3::from(selected[1])
                + u * v * Vec3::from(selected[2]);
        }
        _ => {}
    }
}
/// Apply an ordered update after fixed-tick movement; return whether contact occurred.
pub(crate) fn update(
    m: &Module,
    p: &mut Particle,
    previous: Vec3,
    dt: f32,
    depth: Option<&dyn DepthQuery>,
) -> bool {
    let t = (p.age / p.life).clamp(0., 1.);
    match m {
        Module::Gravity { acceleration } => p.velocity += Vec3::from(*acceleration) * dt,
        Module::Drag { coefficient } => p.velocity /= 1. + coefficient * dt,
        Module::CurlNoise {
            frequency,
            strength,
        } => {
            let q = p.position * *frequency + Vec3::splat(p.birth as f32 * 0.013);
            let (x, y, z) = (q.x, q.y, q.z);
            p.velocity +=
                *strength * dt * Vec3::new(y.cos() - z.cos(), z.cos() - x.cos(), x.cos() - y.cos());
        }
        Module::Vortex {
            center,
            axis,
            strength,
        } => {
            p.velocity += Vec3::from(*axis)
                .normalize()
                .cross(p.position - Vec3::from(*center))
                * *strength
                * dt
        }
        Module::Attractor {
            position,
            strength,
            softening,
        } => {
            let d = Vec3::from(*position) - p.position;
            p.velocity += d * (*strength * dt / (d.length_squared() + softening).powf(1.5));
        }
        Module::ColorOverLife { keys } => {
            p.color = Vec4::from(keys[0].value);
            for pair in keys.windows(2) {
                let f = ((t - pair[0].t) / (pair[1].t - pair[0].t)).clamp(0., 1.);
                p.color = p.color.lerp(Vec4::from(pair[1].value), f);
            }
        }
        Module::SizeOverLife { keys } => {
            p.size = keys[0].value;
            for pair in keys.windows(2) {
                let f = ((t - pair[0].t) / (pair[1].t - pair[0].t)).clamp(0., 1.);
                p.size += (pair[1].value - p.size) * f;
            }
        }
        Module::PlaneCollision {
            normal,
            offset,
            restitution,
        } => {
            let n = Vec3::from(*normal).normalize();
            let distance = p.position.dot(n) - offset;
            if distance < 0. {
                p.position -= distance * n;
                let vn = p.velocity.dot(n);
                if vn < 0. {
                    p.velocity -= (1. + restitution) * vn * n;
                    return true;
                }
            }
        }
        Module::DepthCollision {
            restitution,
            thickness,
        } => {
            if let Some((point, n)) =
                depth.and_then(|q| q.contact(previous, p.position, *thickness))
            {
                let n = n.normalize_or_zero();
                p.position = point;
                let vn = p.velocity.dot(n);
                if vn < 0. {
                    p.velocity -= (1. + restitution) * vn * n;
                    return true;
                }
            }
        }
        _ => {}
    }
    false
}
