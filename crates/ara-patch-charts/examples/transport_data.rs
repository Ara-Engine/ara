//! Opt-in CPU data benchmark. Uses the actual renderer data modules, without GPU
//! initialization, shader work or changes to rendering defaults.
#![cfg_attr(test, allow(clippy::unwrap_used))]

#[path = "../../ara-render/src/look/transport/links.rs"]
mod links;
#[path = "../../ara-render/src/look/transport/patches.rs"]
mod patches;
#[path = "../../ara-render/src/look/transport/reverse.rs"]
mod reverse;

use ara_patch_charts::{Charts, Mesh, Settings, Triangle, cache};
use glam::Mat4;
use links::{Candidate, GeometryKey, GeometryStamp, LinkStore, Visibility};
use patches::{PatchRegistry, Placement};
use reverse::{CrossingIndex, Memory, Region, ReverseLinks};
use std::{collections::BTreeMap, path::Path, sync::Arc, time::Instant};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let mut registry = PatchRegistry::default();
    let mut sizes = Sizes::default();
    let start = Instant::now();
    let label;
    if args.first().is_none_or(|s| s == "--synthetic") {
        label = "synthetic-162500-patches".to_string();
        let mesh = Mesh {
            positions: &[
                [0., 0., 0.],
                [125., 0., 0.],
                [125., 81.25, 0.],
                [0., 81.25, 0.],
            ],
            triangles: &[
                Triangle {
                    vertices: [0, 1, 2],
                    material: 0,
                    albedo: [0.5; 3],
                },
                Triangle {
                    vertices: [0, 2, 3],
                    material: 0,
                    albedo: [0.5; 3],
                },
            ],
        };
        let charts = ara_patch_charts::build(mesh, Settings::default())?;
        sizes.record(&charts);
        registry
            .publish(
                1,
                cache::Key::new(mesh, Settings::default()),
                Arc::new(charts),
                0,
                Placement::default(),
            )
            .map_err(|e| format!("{e:?}"))?;
    } else {
        let path = Path::new(&args[0]);
        label = path.display().to_string();
        load_scene(path, &mut registry, &mut sizes)?;
    }
    let charts_ms = start.elapsed().as_secs_f64() * 1000.;
    let snapshot_start = Instant::now();
    let patches = Arc::new(registry.snapshot().map_err(|e| format!("{e:?}"))?);
    let snapshot_ms = snapshot_start.elapsed().as_secs_f64() * 1000.;
    let count = patches.len();
    sizes.report();
    drop(registry);
    let degree: u32 = args.get(1).map(|s| s.parse()).transpose()?.unwrap_or(48);
    let stamps = args.iter().any(|s| s == "--witness-stress");
    println!(
        "scene={label}, charts_ms={charts_ms:.3}, snapshot_ms={snapshot_ms:.3}, patches={count}, links_per_patch={degree}, witness_stress={stamps}"
    );
    let start = Instant::now();
    let graph = LinkStore::build(
        Arc::clone(&patches),
        vec![],
        count * degree as usize,
        |receiver| {
            Ok((0..degree)
                .map(|ordinal| {
                    let source = ((u64::from(receiver) + 1 + u64::from(ordinal) * 997)
                        % count as u64) as u32;
                    // Deliberately simple visibility, not a radiance or production trace result.
                    let blocked = receiver % 17 == source % 17;
                    Candidate {
                        ordinal,
                        source,
                        weight: 1. / degree as f32,
                        segment: None,
                        visibility: Visibility {
                            transmittance: if blocked { [0.; 3] } else { [1.; 3] },
                            geometry: if stamps || blocked {
                                vec![GeometryStamp {
                                    key: GeometryKey {
                                        instance: 1,
                                        incarnation: 1,
                                        primitive: if stamps { source } else { 0 },
                                    },
                                    generation: 1,
                                }]
                            } else {
                                vec![]
                            },
                        },
                    }
                })
                .collect())
        },
    )
    .map_err(|e| format!("{e:?}"))?;
    let forward_ms = start.elapsed().as_secs_f64() * 1000.;
    let start = Instant::now();
    let reverse = ReverseLinks::build(&graph).map_err(|e| format!("{e:?}"))?;
    let reverse_ms = start.elapsed().as_secs_f64() * 1000.;
    let start = Instant::now();
    let index = CrossingIndex::build(&graph);
    let index_ms = start.elapsed().as_secs_f64() * 1000.;
    let memory = Memory::measure(&graph, &reverse, &index);
    println!(
        "links={}, forward_ms={forward_ms:.3}, reverse_ms={reverse_ms:.3}, index_ms={index_ms:.3}",
        graph.len()
    );
    for (name, bytes) in [
        ("patch_buffers", memory.patches),
        ("forward", memory.forward),
        ("reverse", memory.reverse),
        ("index_including_witnesses", memory.index),
        ("links_reverse_index", memory.link_bytes()),
        ("all_arrays", memory.resident_bytes()),
    ] {
        println!(
            "{name}: bytes={bytes}, MB={:.6}, MiB={:.6}",
            bytes as f64 / 1e6,
            bytes as f64 / 1048576.
        );
    }
    println!(
        "index_B_per_link={:.6}, data_lines_pass={}; peak process and publication overlap are additional G3 charges",
        memory.index_bytes_per_link(),
        memory.within_g3_data_lines()
    );
    if count > 0 {
        let center = patches.position((count / 2) as u32);
        let region = Region::new(center.map(|v| v - 0.25), center.map(|v| v + 0.25))
            .ok_or("invalid benchmark region")?;
        let start = Instant::now();
        let mut found = 0usize;
        index.region(&graph, region, |_| found += 1);
        println!(
            "region_links={found}, region_query_ms={:.3}",
            start.elapsed().as_secs_f64() * 1000.
        );
    }
    Ok(())
}

#[derive(Default)]
struct Sizes {
    edges: Vec<f32>,
    areas: Vec<f32>,
    bytes: usize,
    instances: usize,
}
impl Sizes {
    fn record(&mut self, charts: &Charts) {
        self.instances += 1;
        self.bytes += charts.resident_bytes();
        for p in &charts.patches {
            self.edges
                .push((0..3).map(|i| p.max[i] - p.min[i]).fold(0., f32::max));
            self.areas.push(p.area);
        }
    }
    fn report(&mut self) {
        self.edges.sort_by(f32::total_cmp);
        self.areas.sort_by(f32::total_cmp);
        for (label, values) in [("max_axis_edge_m", &self.edges), ("area_m2", &self.areas)] {
            if !values.is_empty() {
                println!(
                    "{label}: min={:.9}, median={:.9}, max={:.9}",
                    values[0],
                    values[values.len() / 2],
                    values[values.len() - 1]
                );
            }
        }
        println!(
            "mesh_primitive_instances={}, chart_bytes_counting_instances={}",
            self.instances, self.bytes
        );
    }
}

fn load_scene(path: &Path, registry: &mut PatchRegistry, sizes: &mut Sizes) -> Result<()> {
    let gltf::Gltf { document, blob } = gltf::Gltf::open(path)?;
    let buffers = gltf::import_buffers(&document, path.parent(), blob)?;
    let mut cached = BTreeMap::new();
    let scene = document
        .default_scene()
        .or_else(|| document.scenes().next())
        .ok_or("glTF has no scene")?;
    let mut stack: Vec<_> = scene.nodes().map(|n| (n, Mat4::IDENTITY)).collect();
    while let Some((node, parent)) = stack.pop() {
        let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        stack.extend(node.children().map(|n| (n, world)));
        let Some(mesh) = node.mesh() else {
            continue;
        };
        let (scale, rotation, translation) = world.to_scale_rotation_translation();
        let rebuilt = Mat4::from_scale_rotation_translation(scale, rotation, translation);
        if rebuilt
            .to_cols_array()
            .into_iter()
            .zip(world.to_cols_array())
            .any(|(a, b)| (a - b).abs() > 0.001)
        {
            return Err(format!(
                "node {} has shear; bake affine geometry before charting",
                node.index()
            )
            .into());
        }
        let placement = Placement {
            rotation: glam::Mat3::from_quat(rotation).to_cols_array_2d(),
            translation: translation.to_array(),
        };
        for primitive in mesh.primitives() {
            if primitive.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let variant = (
                mesh.index(),
                primitive.index(),
                scale.to_array().map(f32::to_bits),
            );
            let (key, charts) = if let Some((key, charts)) = cached.get(&variant) {
                (*key, Arc::clone(charts))
            } else {
                let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()].0));
                let positions: Vec<_> = reader
                    .read_positions()
                    .ok_or("mesh lacks positions")?
                    .collect();
                let indices: Vec<_> = reader
                    .read_indices()
                    .map(|i| i.into_u32().collect())
                    .unwrap_or_else(|| (0..positions.len() as u32).collect());
                let rgba = primitive
                    .material()
                    .pbr_metallic_roughness()
                    .base_color_factor();
                let triangles: Vec<_> = indices
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|&vertices| Triangle {
                        vertices,
                        material: primitive.material().index().unwrap_or(0) as u32,
                        albedo: [rgba[0], rgba[1], rgba[2]],
                    })
                    .collect();
                let mesh = Mesh {
                    positions: &positions,
                    triangles: &triangles,
                };
                let settings = Settings {
                    scale: scale.to_array(),
                    ..Settings::default()
                };
                let key = cache::Key::new(mesh, settings);
                let charts = Arc::new(ara_patch_charts::build(mesh, settings)?);
                cached.insert(variant, (key, Arc::clone(&charts)));
                (key, charts)
            };
            sizes.record(&charts);
            let id = ((node.index() as u64 + 1) << 32) | (primitive.index() as u64 + 1);
            // glTF has no authored room IDs: all patches form one scene range.
            registry
                .publish(id, key, charts, 0, placement)
                .map_err(|e| format!("{e:?}"))?;
        }
    }
    // Keep the import-source buffers out of the link construction residency.
    Ok(())
}
