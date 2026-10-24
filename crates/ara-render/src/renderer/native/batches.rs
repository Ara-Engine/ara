//! Reusable visible-instance lists; source vertices never move into world space.
use super::*;
use std::collections::BTreeMap;

pub(super) struct Batch {
    pub first: u32,
    pub count: u32,
    pub vertices: u32,
}
struct Group {
    geometry: usize,
    material: usize,
    first: u32,
    counts: Vec<u32>,
    errors: Vec<f32>,
    instances: Vec<usize>,
    transparent: bool,
}
pub(super) struct Groups {
    groups: Vec<Group>,
    identities: Vec<(u64, usize, usize)>,
}
impl Groups {
    pub(super) fn new(scene: &NativeScene, pool: &GeometryGpu) -> Result<Self> {
        let mut grouped = BTreeMap::<(usize, usize), Vec<usize>>::new();
        for (index, instance) in scene.instances.iter().enumerate() {
            grouped
                .entry((instance.geometry, instance.material))
                .or_default()
                .push(index);
        }
        let groups = grouped
            .into_iter()
            .map(|((geometry, material), instances)| {
                let range = pool
                    .lods(geometry)
                    .expect("validated instance geometry has LODs");
                Group {
                    geometry,
                    material,
                    first: range.first,
                    counts: (0..range.count)
                        .map(|lod| {
                            pool.index_count(range.first + lod)
                                .expect("uploaded LOD has indices")
                        })
                        .collect(),
                    errors: scene.geometry[geometry]
                        .lods
                        .iter()
                        .map(|lod| lod.error)
                        .collect(),
                    instances,
                    transparent: scene.materials[material].pbr.flags[0] == 2,
                }
            })
            .collect();
        Ok(Self {
            groups,
            identities: scene
                .instances
                .iter()
                .map(|i| (i.entity, i.geometry, i.material))
                .collect(),
        })
    }

    pub(super) fn validate(&self, scene: &NativeScene) -> Result<()> {
        for (instance, identity) in scene.instances.iter().zip(&self.identities) {
            ensure!(
                (instance.entity, instance.geometry, instance.material) == *identity,
                "Native instance identity/layout changed"
            );
        }
        for group in &self.groups {
            ensure!(
                (scene.materials[group.material].pbr.flags[0] == 2) == group.transparent,
                "Native alpha class changed"
            );
            ensure!(
                scene.geometry[group.geometry].lods.len() == group.counts.len(),
                "Native LOD layout changed"
            );
        }
        Ok(())
    }
}

impl Native {
    pub(super) fn prepare_batches(&mut self, frame: &frame::Frame, camera: &crate::scene::Camera) {
        ara_gpu::cpu_scope!("RasterNative.cull_lod");
        for batches in &mut self.draws {
            batches.clear();
        }
        self.visible_rows.clear();
        self.transparent.clear();
        for pass in 0..5 {
            let planes = planes(if pass < 4 {
                frame.shadows[pass]
            } else {
                camera.projection * camera.view
            });
            for group in &self.groups.groups {
                if group.transparent {
                    continue;
                }
                for lod in 0..group.counts.len() {
                    let first = self.visible_rows.len();
                    for &index in &group.instances {
                        if !visible(&planes, self.bounds[index]) {
                            continue;
                        }
                        let selected = select_lod(
                            group,
                            &self.rows[index],
                            self.bounds[index],
                            camera,
                            frame.extent[1],
                        );
                        if selected == lod {
                            self.visible_rows
                                .push([index as u32, group.first + lod as u32]);
                        }
                    }
                    if first != self.visible_rows.len() {
                        self.draws[pass].push(Batch {
                            first: first as u32,
                            count: (self.visible_rows.len() - first) as u32,
                            vertices: group.counts[lod],
                        });
                    }
                }
            }
        }
        let planes = planes(camera.projection * camera.view);
        for group in &self.groups.groups {
            if !group.transparent {
                continue;
            }
            for &index in &group.instances {
                if visible(&planes, self.bounds[index]) {
                    let distance = ((self.bounds[index][0] + self.bounds[index][1]) * 0.5)
                        .distance_squared(camera.position);
                    self.transparent.push((distance, index, group.first));
                }
            }
        }
        self.transparent
            .sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        for &(_, instance, entry) in &self.transparent {
            self.draws[5].push(Batch {
                first: self.visible_rows.len() as u32,
                count: 1,
                vertices: self
                    .pool
                    .index_count(entry)
                    .expect("prepared transparent LOD"),
            });
            self.visible_rows.push([instance as u32, entry]);
        }
    }
}

fn select_lod(
    group: &Group,
    row: &InstanceRow,
    bounds: [Vec3; 2],
    camera: &crate::scene::Camera,
    height: u32,
) -> usize {
    let scale = row
        .world
        .x_axis
        .length()
        .max(row.world.y_axis.length())
        .max(row.world.z_axis.length());
    let radius = (bounds[1] - bounds[0]).length() * 0.5;
    let distance = (((bounds[0] + bounds[1]) * 0.5).distance(camera.position) - radius).max(0.1);
    let pixels = camera.projection.y_axis.y.abs() * height as f32 * 0.5 / distance;
    group
        .errors
        .iter()
        .enumerate()
        .rev()
        .find_map(|(lod, error)| (*error * scale * pixels <= 1.).then_some(lod))
        .unwrap_or(0)
}

fn planes(matrix: Mat4) -> [Vec4; 6] {
    let x = matrix.row(0);
    let y = matrix.row(1);
    let z = matrix.row(2);
    let w = matrix.row(3);
    [w + x, w - x, w + y, w - y, z, w - z]
}
fn visible(planes: &[Vec4; 6], bounds: [Vec3; 2]) -> bool {
    let centre = (bounds[0] + bounds[1]) * 0.5;
    let extent = (bounds[1] - bounds[0]) * 0.5;
    planes.iter().all(|plane| {
        plane.truncate().dot(centre) + plane.w + plane.truncate().abs().dot(extent) >= 0.
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reversed_z_culling_keeps_intersections_and_rejects_offscreen_bounds() {
        let camera = crate::scene::CameraSpec::LookAt {
            position: Vec3::ZERO,
            target: -Vec3::Z,
            vertical_fov: 1.,
        }
        .resolve([256, 144])
        .unwrap();
        let planes = planes(camera.projection * camera.view);
        assert!(visible(
            &planes,
            [Vec3::new(-1., -1., -5.), Vec3::new(1., 1., -3.)]
        ));
        assert!(!visible(
            &planes,
            [Vec3::new(100., -1., -5.), Vec3::new(102., 1., -3.)]
        ));
        assert!(!visible(
            &planes,
            [Vec3::new(-1., -1., 1.), Vec3::new(1., 1., 3.)]
        ));
    }
}
