//! Emissive surfaces as direct lights for the raster path. Each emissive
//! instance carries its flux (pi x emitted radiance x area) into a local light:
//! flat emitters (panels, screens) become hemispherical spots facing their
//! emission direction, closed ones (bulbs) points. Small neighbouring emitters
//! (festoon strings) cluster into one light, on a grid that coarsens only when
//! the scene's light budget requires it. Emitters then light the scene with
//! inverse-square falloff, while the probe volume keeps only their bounce (the
//! GI bake zeroes [`is_emitter`] materials' emission).
use crate::gi_cpu::Shading;
use ara_render::{
    instanced::{Geometry, Scene},
    scene::Light,
};
use glam30::{Mat3, Mat4, Vec3};
use std::collections::BTreeMap;

/// Emitted radiance (luminance, cd/m2) above which a material lights the scene
/// directly; dimmer glow stays in the probe volume.
const EMITTER_RADIANCE: f32 = 5.;
/// Flux (lumens) below which an emitter is too small to be a light.
const MIN_FLUX: f32 = 1.;
/// Illuminance (lux) at which a light's influence ends.
const CUTOFF_LUX: f32 = 0.1;
/// Finest clustering cell (metres); doubled until the lights fit the budget.
const CELL: f32 = 0.75;

fn luminance(c: Vec3) -> f32 {
    c.dot(Vec3::new(0.2126, 0.7152, 0.0722))
}

/// Whether a material with `shading` lights the scene as emitter lights.
pub(crate) fn is_emitter(shading: Shading) -> bool {
    luminance(shading.emissive) > EMITTER_RADIANCE
}

/// Model-space emitting surface of a geometry (finest LOD).
#[derive(Clone, Copy, Default)]
struct Surface {
    area: f32,
    /// Area-weighted outward normal sum.
    normal: Vec3,
    /// Area-weighted centroid.
    centre: Vec3,
}

fn surface(geometry: &Geometry) -> Surface {
    let Some(lod) = geometry.lods.first() else {
        return Surface::default();
    };
    let mut s = Surface::default();
    let mut weighted = Vec3::ZERO;
    for tri in lod.indices.as_chunks::<3>().0 {
        let Some([a, b, c]) = tri
            .iter()
            .map(|&i| geometry.vertices.get(i as usize))
            .collect::<Option<Vec<_>>>()
            .and_then(|v| <[_; 3]>::try_from(v).ok())
        else {
            continue;
        };
        let [pa, pb, pc] = [a, b, c].map(|v| Vec3::from_array(v.position));
        let cross = (pb - pa).cross(pc - pa);
        let area = cross.length() * 0.5;
        if area <= 0. {
            continue;
        }
        let mut normal = cross / (2. * area);
        let shading =
            Vec3::from_array(a.normal) + Vec3::from_array(b.normal) + Vec3::from_array(c.normal);
        if normal.dot(shading) < 0. {
            normal = -normal;
        }
        s.area += area;
        s.normal += normal * area;
        weighted += (pa + pb + pc) / 3. * area;
    }
    if s.area > 0. {
        s.centre = weighted / s.area;
    }
    s
}

/// One emitting instance: its model-space surface and emitted radiance.
struct Emitter {
    instance: usize,
    surface: Surface,
    radiance: Vec3,
    /// The transform its current light was placed with.
    placed: Option<Mat4>,
}

/// The emissive instances of one content generation; their lights follow the
/// instances' current transforms.
pub(crate) struct Emitters {
    emitters: Vec<Emitter>,
    /// The light budget the current lights were clustered for.
    budget: Option<usize>,
}

impl Emitters {
    /// Finds the emitters of `scene`; `shading` is indexed by material.
    pub(crate) fn new(scene: &Scene, shading: &[Shading]) -> Self {
        let mut surfaces: BTreeMap<usize, Surface> = BTreeMap::new();
        let mut emitters = Vec::new();
        for (instance, i) in scene.instances.iter().enumerate() {
            let Some(&s) = shading.get(i.material) else {
                continue;
            };
            if !is_emitter(s) {
                continue;
            }
            let Some(geometry) = scene.geometry.get(i.geometry) else {
                continue;
            };
            let surface = *surfaces
                .entry(i.geometry)
                .or_insert_with(|| self::surface(geometry));
            if surface.area > 0. {
                emitters.push(Emitter {
                    instance,
                    surface,
                    radiance: s.emissive,
                    placed: None,
                });
            }
        }
        Self {
            emitters,
            budget: None,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.emitters.is_empty()
    }

    /// Emissive instances (before clustering).
    pub(crate) fn len(&self) -> usize {
        self.emitters.len()
    }

    /// Lights for the emitters at `scene`'s current transforms, at most `budget`;
    /// `None` while no emitter moved and the budget held since the last call
    /// (a per-frame check without allocation).
    pub(crate) fn update(&mut self, scene: &Scene, budget: usize) -> Option<Vec<Light>> {
        let mut moved = self.budget != Some(budget);
        for e in &mut self.emitters {
            let world = scene.instances.get(e.instance).map(|i| i.world);
            if world != e.placed {
                e.placed = world;
                moved = true;
            }
        }
        self.budget = Some(budget);
        moved.then(|| self.lights(budget))
    }

    fn lights(&self, budget: usize) -> Vec<Light> {
        let mut parts = Vec::with_capacity(self.emitters.len());
        for e in &self.emitters {
            let Some(world) = e.placed else {
                continue;
            };
            let linear = Mat3::from_mat4(world);
            let scale = linear.determinant().abs().powf(2. / 3.);
            let area = e.surface.area * scale;
            let flux = e.radiance * std::f32::consts::PI * area;
            if luminance(flux) < MIN_FLUX {
                continue;
            }
            let normal = linear.inverse().transpose() * e.surface.normal;
            // Flat emitters send their flux into one hemisphere.
            let flat = normal.length() / e.surface.area.max(1e-6) > 0.7;
            parts.push(Part {
                position: world.transform_point3(e.surface.centre),
                flux,
                direction: if flat {
                    normal.normalize_or_zero()
                } else {
                    Vec3::ZERO
                },
                radius: (area / std::f32::consts::PI).sqrt(),
            });
        }
        let mut cell = CELL;
        loop {
            let lights = cluster(&parts, cell);
            if lights.len() <= budget || cell > 1e4 {
                return lights.into_iter().take(budget).map(Part::light).collect();
            }
            cell *= 2.;
        }
    }
}

/// An emitter's light before clustering.
#[derive(Clone, Copy)]
struct Part {
    position: Vec3,
    flux: Vec3,
    /// Unit emission direction of a flat emitter; zero for a closed one.
    direction: Vec3,
    /// Radius of an equal-area disk: the light's near-field softening.
    radius: f32,
}

impl Part {
    fn light(self) -> Light {
        let lumens = luminance(self.flux).max(1e-6);
        let tint = self.flux / lumens;
        // Point-equivalent lumens (the shading convention): a Lambertian
        // hemisphere's peak intensity, flux / pi, needs four times its flux.
        let spot = self.direction != Vec3::ZERO;
        let intensity = if spot { lumens * 4. } else { lumens };
        let range = (intensity / (4. * std::f32::consts::PI * CUTOFF_LUX))
            .sqrt()
            .clamp(1., 80.);
        Light {
            position_range: [self.position.x, self.position.y, self.position.z, range],
            color_lumens: [tint.x, tint.y, tint.z, intensity],
            direction_outer: [self.direction.x, self.direction.y, self.direction.z, 0.],
            parameters: [f32::from(u8::from(spot)), 1., self.radius, 0.],
        }
    }
}

/// Merges parts sharing a grid cell (and, for flat ones, a facing) into one light
/// at their flux-weighted position; brightest first.
fn cluster(parts: &[Part], cell: f32) -> Vec<Part> {
    let mut cells: BTreeMap<([i64; 3], [i8; 3]), Part> = BTreeMap::new();
    for p in parts {
        let key = (
            (p.position / cell).floor().as_i64vec3().to_array(),
            (p.direction * 2.).round().as_i8vec3().to_array(),
        );
        cells
            .entry(key)
            .and_modify(|c| {
                let (a, b) = (luminance(c.flux), luminance(p.flux));
                let w = b / (a + b).max(1e-6);
                c.position = c.position.lerp(p.position, w);
                c.direction = (c.direction * a + p.direction * b).normalize_or_zero();
                c.radius = (c.radius * c.radius + p.radius * p.radius).sqrt();
                c.flux += p.flux;
            })
            .or_insert(*p);
    }
    let mut lights: Vec<Part> = cells.into_values().collect();
    lights.sort_by(|a, b| luminance(b.flux).total_cmp(&luminance(a.flux)));
    lights
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(x: f32, flux: f32, direction: Vec3) -> Part {
        Part {
            position: Vec3::new(x, 3., 0.),
            flux: Vec3::splat(flux),
            direction,
            radius: 0.05,
        }
    }

    #[test]
    fn nearby_emitters_cluster_only_as_far_as_the_budget_needs() {
        // A festoon string: twenty bulbs half a metre apart.
        let bulbs: Vec<Part> = (0..20)
            .map(|i| part(i as f32 * 0.5, 50., Vec3::ZERO))
            .collect();
        let fine = cluster(&bulbs, CELL);
        assert!(fine.len() >= 10, "{} lights", fine.len());
        let total: f32 = fine.iter().map(|p| p.flux.x).sum();
        assert!((total - 1000.).abs() < 1e-3, "flux is conserved: {total}");
        let coarse = cluster(&bulbs, CELL * 8.);
        assert!(coarse.len() < fine.len());
        // A downward panel and an upward one never merge.
        let panels = [part(0., 500., Vec3::NEG_Y), part(0.1, 500., Vec3::Y)];
        assert_eq!(cluster(&panels, 10.).len(), 2);
    }

    #[test]
    fn emitter_lights_follow_their_instances_and_recompute_only_on_motion() {
        use ara_render::instanced::{Instance, Lod, Vertex};
        // A 1 m2 ceiling panel at 3 m, emitting downward at 100 cd/m2.
        let vertices = [[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]]
            .map(|[x, z]| Vertex {
                position: [x, 3., z],
                normal: [0., -1., 0.],
                uv: [0.; 2],
                tangent: [1., 0., 0., 1.],
                uv1: [0.; 2],
                terrain_weights: 0,
                reserved: 0,
            })
            .to_vec();
        let mut scene = Scene {
            geometry: vec![Geometry {
                key: [1; 32],
                vertices,
                lods: vec![Lod {
                    indices: vec![0, 1, 2, 0, 2, 3],
                    error: 0.,
                }],
                bounds: [Vec3::new(-0.5, 3., -0.5), Vec3::new(0.5, 3., 0.5)],
                deforming: false,
                revision: 0,
            }],
            instances: vec![Instance {
                entity: 1,
                name: String::new(),
                geometry: 0,
                material: 0,
                world: Mat4::IDENTITY,
                previous_world: Mat4::IDENTITY,
            }],
            materials: Vec::new(),
            textures: 0,
            lights: Vec::new(),
            sun_direction: Vec3::Y,
            sun_lux: Vec3::ZERO,
            environment: Vec::new(),
            environment_size: [0, 0],
        };
        let shading = [Shading {
            albedo: Vec3::ONE,
            emissive: Vec3::splat(100.),
        }];
        let mut emitters = Emitters::new(&scene, &shading);
        let lights = emitters.update(&scene, 8).expect("first placement");
        let [light] = lights.as_slice() else {
            panic!("one panel, one light: {}", lights.len());
        };
        assert_eq!(light.position_range[..3], [0., 3., 0.]);
        assert_eq!(light.direction_outer[..3], [0., -1., 0.], "a downward spot");
        let flux = 100. * std::f32::consts::PI;
        assert!((light.color_lumens[3] - 4. * flux).abs() < 1e-2);
        assert!(emitters.update(&scene, 8).is_none(), "nothing moved");
        scene.instances[0].world = Mat4::from_translation(Vec3::new(2., 0., 0.));
        let moved = emitters.update(&scene, 8).expect("the panel moved");
        assert_eq!(moved[0].position_range[..3], [2., 3., 0.]);
        assert!(emitters.update(&scene, 4).is_some(), "a new budget");
    }

    #[test]
    fn a_flat_emitter_is_a_hemispherical_spot_with_lambertian_peak() {
        let light = part(0., 100., Vec3::NEG_Y).light();
        assert_eq!(light.parameters[0], 1., "spot");
        assert_eq!(light.direction_outer[3], 0., "a 90 degree outer cone");
        // Peak intensity flux / pi in the point-equivalent convention.
        let peak = light.color_lumens[3] / (4. * std::f32::consts::PI);
        assert!((peak - 100. / std::f32::consts::PI).abs() < 1e-3);
        let bulb = part(0., 100., Vec3::ZERO).light();
        assert_eq!(bulb.parameters[0], 0., "point");
        assert!((bulb.color_lumens[3] - 100.).abs() < 1e-3);
    }
}
