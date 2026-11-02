//! Native material sampling and finite-support light forcing.
use super::*;
pub(super) struct NativeMaterials<'a> {
    pub(super) scene: &'a Scene,
    pub(super) textures: &'a [crate::scene::Texture],
    pub(super) bodies: &'a BTreeMap<u64, BodyClass>,
    pub(super) generation: u64,
}

impl NativeMaterials<'_> {
    pub(super) fn patch(&self, id: u64, charts: &Charts, local: u32) -> (Vec3, Vec3) {
        let instance = &self.scene.instances[id as usize];
        let geometry = &self.scene.geometry[instance.geometry];
        let mut albedo = Vec3::ZERO;
        let mut emission = Vec3::ZERO;
        let mut area = 0.;
        for fragment in charts.coverage(&charts.patches[local as usize]) {
            let triangle = &geometry.lods[0].indices
                [fragment.triangle as usize * 3..fragment.triangle as usize * 3 + 3];
            let original = std::array::from_fn::<_, 3, _>(|corner| {
                let i = triangle[corner];
                instance
                    .world
                    .transform_point3(Vec3::from(geometry.vertices[i as usize].position))
            });
            let points = fragment.barycentric.map(|uv| {
                original[0] * (1. - uv[0] - uv[1]) + original[1] * uv[0] + original[2] * uv[1]
            });
            let weight = (points[1] - points[0])
                .cross(points[2] - points[0])
                .length()
                * 0.5;
            let barycentric = std::array::from_fn(|axis| {
                fragment.barycentric.iter().map(|v| v[axis]).sum::<f32>() / 3.
            });
            // Material evaluation only needs original primitive/barycentrics.
            let hit = SurfaceHit {
                instance: trace::InstanceKey {
                    slot: 0,
                    generation: 0,
                },
                id,
                primitive: fragment.triangle,
                barycentric,
                distance: 0.,
                front: true,
            };
            let (base, emitted, transmission, metallic) = self.material(hit);
            albedo += base.truncate() * (weight * (1. - metallic) * (1. - transmission));
            emission += emitted * weight;
            area += weight;
        }
        if area > 0. {
            (albedo / area, emission / area)
        } else {
            (Vec3::ZERO, Vec3::ZERO)
        }
    }

    pub(super) fn material(&self, hit: SurfaceHit) -> (Vec4, Vec3, f32, f32) {
        let instance = &self.scene.instances[hit.id as usize];
        let geometry = &self.scene.geometry[instance.geometry];
        let triangle =
            &geometry.lods[0].indices[hit.primitive as usize * 3..hit.primitive as usize * 3 + 3];
        let weights = [
            1. - hit.barycentric[0] - hit.barycentric[1],
            hit.barycentric[0],
            hit.barycentric[1],
        ];
        let mut uv0 = [0.; 2];
        let mut uv1 = [0.; 2];
        for (&index, weight) in triangle.iter().zip(weights) {
            for axis in 0..2 {
                uv0[axis] += geometry.vertices[index as usize].uv[axis] * weight;
                uv1[axis] += geometry.vertices[index as usize].uv1[axis] * weight;
            }
        }
        let m = &self.scene.materials[instance.material];
        let sample = |slot: usize, index: u32| {
            let uv = m.extras.uv[slot].apply(uv0, uv1);
            sample_texture(self.textures, index, uv)
        };
        let base = Vec4::from(m.pbr.base_color) * sample(0, m.pbr.textures[0]);
        let emission = Vec3::from_array(m.pbr.emissive_roughness[..3].try_into().expect("RGB"))
            * sample(3, m.pbr.textures[3]).truncate();
        let transmission =
            m.extras.transmission_factor * sample(5, m.extras.transmission_texture).x;
        let metallic = m.pbr.surface[0] * sample(2, m.pbr.textures[2]).z;
        (base, emission, transmission, metallic)
    }
}
impl Materials for NativeMaterials<'_> {
    fn optical(&self, hit: SurfaceHit) -> Optical {
        let instance = &self.scene.instances[hit.id as usize];
        let bounds = instance.bounds(&self.scene.geometry[instance.geometry]);
        if !self
            .bodies
            .get(&hit.id)
            .copied()
            .unwrap_or_default()
            .owns_links()
            && (bounds[1] - bounds[0]).max_element() < 0.5
        {
            return Optical::Ignore;
        }
        let material = &self.scene.materials[instance.material];
        let (base, _, transmission, _) = self.material(hit);
        if material.pbr.flags[0] == 1 && base.w < material.pbr.surface[3] {
            return Optical::Ignore;
        }
        if transmission > 0. {
            return Optical::Transmit(base.truncate() * transmission);
        }
        if material.pbr.flags[0] == 2 {
            return Optical::Transmit(Vec3::splat(1. - base.w));
        }
        Optical::Surface
    }
    fn generation(&self, _: u64) -> u64 {
        self.generation
    }
}

fn sample_texture(textures: &[crate::scene::Texture], id: u32, uv: [f32; 2]) -> Vec4 {
    let Some(texture) = textures.get(id as usize) else {
        return Vec4::ONE;
    };
    let [width, height] = texture.size;
    let p = glam::Vec2::from(uv) * glam::Vec2::new(width as f32, height as f32)
        - glam::Vec2::splat(0.5);
    let base = p.floor().as_ivec2();
    let f = p - p.floor();
    let mut result = Vec4::ZERO;
    for y in 0..=1 {
        for x in 0..=1 {
            let ix = (base.x + x).rem_euclid(width as i32) as usize;
            let iy = (base.y + y).rem_euclid(height as i32) as usize;
            let i = (iy * width as usize + ix) * 4;
            let mut value = Vec4::new(
                texture.rgba[i] as f32,
                texture.rgba[i + 1] as f32,
                texture.rgba[i + 2] as f32,
                texture.rgba[i + 3] as f32,
            ) / 255.;
            if texture.srgb {
                for c in 0..3 {
                    value[c] = if value[c] <= 0.04045 {
                        value[c] / 12.92
                    } else {
                        ((value[c] + 0.055) / 1.055).powf(2.4)
                    };
                }
            }
            result += value
                * (if x == 0 { 1. - f.x } else { f.x })
                * (if y == 0 { 1. - f.y } else { f.y });
        }
    }
    result
}

pub(super) fn environment(scene: &Scene, direction: Vec3) -> Vec3 {
    let [w, h] = scene.environment_size;
    if w == 0 || h == 0 || scene.environment.is_empty() {
        return Vec3::ZERO;
    }
    let u = (direction.z.atan2(direction.x) / std::f32::consts::TAU + 0.5).rem_euclid(1.);
    let v = direction.y.clamp(-1., 1.).acos() / std::f32::consts::PI;
    let x = (u * w as f32) as u32;
    let y = ((v * h as f32) as u32).min(h - 1);
    let value = scene.environment[(y * w + x.min(w - 1)) as usize];
    Vec3::new(value[0], value[1], value[2])
}

pub(super) fn visibility(
    cpu: &CpuTrace<'_>,
    materials: &NativeMaterials<'_>,
    position: Vec3,
    direction: Vec3,
    distance: f32,
) -> Vec3 {
    let mut ray = Ray {
        origin: position,
        direction,
        t_min: 1e-4,
        t_max: distance,
    };
    let mut transmission = Vec3::ONE;
    while let Some(hit) = cpu.closest(ray, |h| !matches!(materials.optical(h), Optical::Ignore)) {
        match materials.optical(hit) {
            Optical::Surface => return Vec3::ZERO,
            Optical::Transmit(rgb) => transmission *= rgb,
            Optical::Ignore => {}
        }
        ray.t_min = f32::from_bits(hit.distance.to_bits() + 1);
    }
    transmission
}

pub(super) fn direct(
    scene: &Scene,
    cpu: &CpuTrace<'_>,
    materials: &NativeMaterials<'_>,
    position: Vec3,
    normal: Vec3,
    distance: f32,
) -> Vec3 {
    let sun = scene.sun_direction.normalize_or_zero();
    let mut incident = scene.sun_lux
        * normal.dot(sun).max(0.)
        * visibility(cpu, materials, position, sun, distance);
    for light in &scene.lights {
        incident += lamp(light, cpu, materials, position, normal);
    }
    incident
}
