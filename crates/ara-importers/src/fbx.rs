use crate::{MeshSettings, TextureSettings, TextureUsage, Vertex, cook_mesh, invalid, json_bytes};
use ara_asset::{
    Artifact, ArtifactKind, ImportContext, ImportInput, ImportOutput, Importer, Result,
};
use ara_asset_format::{Builder, Channel, Clip, Joint, Kind, write_skeleton};
use rayon::prelude::*;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Autodesk FBX importer through ufbx, with explicit external input snapshots.
pub struct FbxImporter;
fn load(source: &[u8]) -> Result<ufbx::SceneRoot> {
    ufbx::load_memory(
        source,
        ufbx::LoadOpts {
            load_external_files: false,
            generate_missing_normals: true,
            normalize_normals: true,
            normalize_tangents: true,
            target_axes: ufbx::CoordinateAxes::right_handed_y_up(),
            target_unit_meters: 1.0,
            space_conversion: ufbx::SpaceConversion::ModifyGeometry,
            geometry_transform_handling: ufbx::GeometryTransformHandling::HelperNodes,
            clean_skin_weights: true,
            ..Default::default()
        },
    )
    .map_err(|e| invalid(format!("{e:?}")))
}
impl Importer for FbxImporter {
    fn id(&self) -> &str {
        "ara.fbx"
    }
    fn version(&self) -> &str {
        "4-ufbx0115-meshopt070-ispc050-tiled-f32text-arag1"
    }
    fn register_settings(&self, db: &mut ara_schema::SchemaDb) {
        db.register::<MeshSettings>();
    }
    fn dependencies(&self, source: &[u8]) -> Result<Vec<String>> {
        let scene = load(source)?;
        let mut deps = BTreeSet::new();
        for texture in &scene.texture_files {
            if texture.content.is_empty() {
                deps.insert(texture_path(&texture.relative_filename, &texture.filename)?);
            }
        }
        Ok(deps.into_iter().collect())
    }
    fn import(&self, input: &ImportInput, ctx: &mut ImportContext<'_>) -> Result<ImportOutput> {
        ctx.report(0.0, "decode FBX with ufbx")?;
        let scene = load(&input.source)?;
        let settings = if input.settings.is_null() {
            MeshSettings::default()
        } else {
            serde_json::from_value(input.settings.clone())?
        };
        let mut output = ImportOutput::default();
        let mut meshes = Vec::new();
        let mut mesh_jobs = Vec::new();
        let cancellation = ctx.cancellation;
        let metrics = ctx.metrics;
        for mesh in &scene.meshes {
            ctx.report(0.1, "optimize FBX geometry")?;
            let mut vertices = Vec::with_capacity(mesh.num_indices);
            for i in 0..mesh.num_indices {
                let mut v = Vertex {
                    position: vec3(mesh.vertex_position[i]),
                    normal: vec3(mesh.vertex_normal[i]),
                    source_index: mesh.vertex_indices[i],
                    color: [1.0; 4],
                    ..Default::default()
                };
                if mesh.vertex_uv.exists {
                    let uv = mesh.vertex_uv[i];
                    v.uv0 = [uv.x as f32, uv.y as f32];
                }
                if mesh.uv_sets.len() > 1 {
                    let uv = mesh.uv_sets[1].vertex_uv[i];
                    v.uv1 = [uv.x as f32, uv.y as f32];
                }
                if mesh.vertex_tangent.exists {
                    let t = vec3(mesh.vertex_tangent[i]);
                    let n = v.normal;
                    let sign = if mesh.vertex_bitangent.exists {
                        let b = vec3(mesh.vertex_bitangent[i]);
                        let cross = [
                            n[1] * t[2] - n[2] * t[1],
                            n[2] * t[0] - n[0] * t[2],
                            n[0] * t[1] - n[1] * t[0],
                        ];
                        if cross.iter().zip(b).map(|(a, b)| a * b).sum::<f32>() < 0.0 {
                            -1.0
                        } else {
                            1.0
                        }
                    } else {
                        1.0
                    };
                    v.tangent = [t[0], t[1], t[2], sign];
                }
                if mesh.vertex_color.exists {
                    let c = mesh.vertex_color[i];
                    v.color = [c.x as f32, c.y as f32, c.z as f32, c.w as f32];
                }
                if let Some(skin) = mesh.skin_deformers.first() {
                    let sv = skin
                        .vertices
                        .get(mesh.vertex_indices[i] as usize)
                        .ok_or_else(|| invalid("invalid FBX skin vertex"))?;
                    let end = sv.weight_begin as usize + sv.num_weights as usize;
                    let weights = skin
                        .weights
                        .get(sv.weight_begin as usize..end)
                        .ok_or_else(|| invalid("invalid FBX skin weights"))?;
                    if weights.len() > 8 {
                        return Err(invalid(
                            "FBX vertex has more than eight skin influences; reduce weights explicitly in the source",
                        ));
                    }
                    for (j, w) in weights.iter().enumerate() {
                        if j < 4 {
                            v.joints[j] = u16::try_from(w.cluster_index).map_err(invalid)?;
                            v.weights[j] = w.weight as f32;
                        } else {
                            v.joints1[j - 4] = u16::try_from(w.cluster_index).map_err(invalid)?;
                            v.weights1[j - 4] = w.weight as f32;
                        }
                    }
                }
                vertices.push(v);
            }
            let mut by_material: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
            let mut triangulated = vec![0; mesh.max_face_triangles * 3];
            for (i, &face) in mesh.faces.iter().enumerate() {
                let count = ufbx::triangulate_face(&mut triangulated, mesh, face) as usize * 3;
                let material = mesh.face_material.get(i).copied().unwrap_or(0);
                by_material
                    .entry(material)
                    .or_default()
                    .extend_from_slice(&triangulated[..count]);
            }
            let vertices = Arc::new(vertices);
            let material_count = by_material.len();
            let deformed = !mesh.skin_deformers.is_empty() || !mesh.blend_deformers.is_empty();
            for (material, indices) in by_material {
                if indices.is_empty() {
                    continue;
                }
                let name = format!("meshes/{}-{material}.mesh", mesh.element.typed_id);
                mesh_jobs.push((
                    name.clone(),
                    Arc::clone(&vertices),
                    indices,
                    deformed,
                    material_count,
                ));
                meshes.push(json!({"mesh":mesh.element.typed_id,"primitive":material,"artifact":name,"material":mesh.materials.get(material as usize).map(|m|m.element.typed_id),"skin":mesh.skin_deformers.first().map(|s|s.element.typed_id)}));
            }
        }
        let mesh_timer = metrics.map(|m| m.start(ara_asset::ImportStage::Mesh));
        let artifacts: Result<Vec<_>> = mesh_jobs
            .into_par_iter()
            .map(|(name, vertices, indices, deformed, materials)| {
                cancellation.check()?;
                let (virtual_artifact, virtual_metadata) = crate::virtual_geometry::cook(
                    &name, &vertices, &indices, settings, deformed, materials,
                )?;
                let artifact = Artifact {
                    name: name.clone(),
                    kind: ArtifactKind::Mesh,
                    bytes: cook_mesh(&vertices, &indices, settings)?,
                };
                Ok((artifact, virtual_artifact, virtual_metadata))
            })
            .collect();
        drop(mesh_timer);
        for (metadata, (artifact, virtual_artifact, virtual_metadata)) in
            meshes.iter_mut().zip(artifacts?)
        {
            metadata["virtualGeometry"] = virtual_metadata;
            output.artifacts.push(artifact);
            output.artifacts.extend(virtual_artifact);
        }
        let mut skins = Vec::new();
        for skin in &scene.skin_deformers {
            let joint_index: BTreeMap<_, _> = skin
                .clusters
                .iter()
                .enumerate()
                .filter_map(|(i, c)| c.bone_node.as_ref().map(|n| (n.element.typed_id, i as i32)))
                .collect();
            let mut joints = Vec::new();
            let mut names = Vec::new();
            for cluster in &skin.clusters {
                let node = cluster
                    .bone_node
                    .as_ref()
                    .ok_or_else(|| invalid("FBX skin cluster lacks a bone node"))?;
                let mut parent = -1;
                let mut p = node.parent.as_ref();
                while let Some(n) = p {
                    if let Some(&index) = joint_index.get(&n.element.typed_id) {
                        parent = index;
                        break;
                    }
                    p = n.parent.as_ref();
                }
                joints.push(Joint {
                    parent,
                    node: node.element.typed_id,
                    inverse_bind: matrix(cluster.geometry_to_bone),
                    rest: transform(node.local_transform),
                });
                names.push(node.element.name.as_ref());
            }
            let bytes = write_skeleton(&joints, &names)?;
            let name = format!("skeletons/{}.skeleton", skin.element.typed_id);
            output.artifacts.push(Artifact {
                name: name.clone(),
                kind: ArtifactKind::Skeleton,
                bytes,
            });
            skins.push(json!({"skin":skin.element.typed_id,"artifact":name}));
        }
        let mut textures = Vec::new();
        let mut texture_jobs = Vec::new();
        let mut materials = Vec::new();
        let mut cooked = BTreeSet::new();
        for material in &scene.materials {
            let p = &material.pbr;
            let mut maps = serde_json::Map::new();
            for (semantic, map, usage) in [
                ("baseColor", &p.base_color, TextureUsage::Color),
                ("metallic", &p.metalness, TextureUsage::Mask),
                ("roughness", &p.roughness, TextureUsage::Mask),
                ("normal", &p.normal_map, TextureUsage::Normal),
                ("occlusion", &p.ambient_occlusion, TextureUsage::Mask),
                ("emissive", &p.emission_color, TextureUsage::Color),
            ] {
                let texture = map.texture.as_ref();
                let mut artifact = None;
                if let Some(t) = texture {
                    let id = t.element.typed_id;
                    let name = format!("textures/{id}-{usage:?}.ktx2");
                    artifact = Some(name.clone());
                    if cooked.insert((id, usage as u8)) {
                        let bytes = if !t.content.is_empty() {
                            t.content.as_ref().to_vec()
                        } else {
                            let path = texture_path(&t.relative_filename, &t.filename)?;
                            input
                                .dependencies
                                .get(&path)
                                .cloned()
                                .ok_or_else(|| invalid(format!("missing FBX texture {path}")))?
                        };
                        texture_jobs.push((name.clone(), usage, bytes));
                        textures.push(json!({"texture":id,"usage":usage,"artifact":name}));
                    }
                }
                let value = map.value_vec4;
                maps.insert(semantic.into(),json!({"value":[value.x,value.y,value.z,value.w],"hasValue":map.has_value,"artifact":artifact}));
            }
            materials.push(json!({"material":material.element.typed_id,"name":material.element.name.as_ref(),"maps":maps}));
        }
        let (names, jobs): (Vec<_>, Vec<_>) = texture_jobs
            .into_iter()
            .map(|(name, usage, source)| {
                (
                    name,
                    (
                        TextureSettings {
                            usage,
                            ..Default::default()
                        },
                        source,
                    ),
                )
            })
            .unzip();
        let cooked = crate::texture::cook_texture_batch(&jobs, input.target, input.preset, ctx)?;
        output
            .artifacts
            .extend(names.into_iter().zip(cooked).map(|(name, bytes)| Artifact {
                name,
                bytes,
                kind: ArtifactKind::Texture,
            }));
        let mut clips = Vec::new();
        for stack in &scene.anim_stacks {
            ctx.report(0.9, "bake FBX animation")?;
            let baked = ufbx::bake_anim(
                &scene,
                &stack.anim,
                ufbx::BakeOpts {
                    resample_rate: 60.0,
                    trim_start_time: true,
                    ..Default::default()
                },
            )
            .map_err(|e| invalid(format!("{e:?}")))?;
            let mut channels = Vec::new();
            let mut times = Vec::<f32>::new();
            let mut values = Vec::<f32>::new();
            for node in &baked.nodes {
                for (path, keys) in [(0, &node.translation_keys), (2, &node.scale_keys)] {
                    if keys.is_empty() {
                        continue;
                    }
                    let time_offset = times.len() as u32;
                    let value_offset = values.len() as u32;
                    for k in keys.iter() {
                        times.push(k.time as f32);
                        values.extend(vec3(k.value));
                    }
                    channels.push(Channel {
                        node: node.typed_id,
                        path,
                        interpolation: 1,
                        time_offset,
                        key_count: keys.len() as u32,
                        value_offset,
                        components: 3,
                        reserved: 0,
                    });
                }
                if !node.rotation_keys.is_empty() {
                    let time_offset = times.len() as u32;
                    let value_offset = values.len() as u32;
                    for k in &node.rotation_keys {
                        times.push(k.time as f32);
                        values.extend([
                            k.value.x as f32,
                            k.value.y as f32,
                            k.value.z as f32,
                            k.value.w as f32,
                        ]);
                    }
                    channels.push(Channel {
                        node: node.typed_id,
                        path: 1,
                        interpolation: 1,
                        time_offset,
                        key_count: node.rotation_keys.len() as u32,
                        value_offset,
                        components: 4,
                        reserved: 0,
                    });
                }
            }
            let mut b = Builder::new(Kind::Clip);
            b.section(*b"CHAN", 32, bytemuck::cast_slice(&channels).to_vec())
                .section(*b"TIME", 4, bytemuck::cast_slice(&times).to_vec())
                .section(*b"VALU", 4, bytemuck::cast_slice(&values).to_vec());
            let bytes = b.finish()?;
            Clip::read(&bytes)?;
            let name = format!("clips/{}.clip", stack.element.typed_id);
            output.artifacts.push(Artifact {
                name: name.clone(),
                kind: ArtifactKind::Clip,
                bytes,
            });
            clips.push(json!({"name":stack.element.name.as_ref(),"artifact":name}));
        }
        let nodes:Vec<_>=scene.nodes.iter().map(|n|json!({"node":n.element.typed_id,"name":n.element.name.as_ref(),"parent":n.parent.as_ref().map(|p|p.element.typed_id),"children":n.children.iter().map(|c|c.element.typed_id).collect::<Vec<_>>(),"mesh":n.mesh.as_ref().map(|m|m.element.typed_id),"materials":n.materials.iter().map(|m|m.element.typed_id).collect::<Vec<_>>(),"localTransform":transform(n.local_transform),"geometryTransform":matrix(n.geometry_to_node)})).collect();
        let manifest = json!({"schema":"ara://cooked/scene/1","sourceFormat":"fbx","nodes":nodes,"meshes":meshes,"materials":materials,"textures":textures,"skeletons":skins,"clips":clips});
        output.artifacts.push(Artifact {
            name: "scene.json".into(),
            kind: ArtifactKind::Scene,
            bytes: json_bytes(&manifest)?,
        });
        Ok(output)
    }
}
fn texture_path(relative: &str, filename: &str) -> Result<String> {
    let path = if relative.is_empty() {
        filename
    } else {
        relative
    }
    .replace('\\', "/");
    if path.is_empty() || path.contains(':') || path.starts_with('/') {
        return Err(invalid("FBX texture must use a project-relative path"));
    }
    Ok(path)
}
fn vec3(v: ufbx::Vec3) -> [f32; 3] {
    [v.x as f32, v.y as f32, v.z as f32]
}
fn transform(t: ufbx::Transform) -> [f32; 10] {
    [
        t.translation.x as f32,
        t.translation.y as f32,
        t.translation.z as f32,
        t.rotation.x as f32,
        t.rotation.y as f32,
        t.rotation.z as f32,
        t.rotation.w as f32,
        t.scale.x as f32,
        t.scale.y as f32,
        t.scale.z as f32,
    ]
}
fn matrix(m: ufbx::Matrix) -> [f32; 16] {
    [
        m.m00 as f32,
        m.m10 as f32,
        m.m20 as f32,
        0.0,
        m.m01 as f32,
        m.m11 as f32,
        m.m21 as f32,
        0.0,
        m.m02 as f32,
        m.m12 as f32,
        m.m22 as f32,
        0.0,
        m.m03 as f32,
        m.m13 as f32,
        m.m23 as f32,
        1.0,
    ]
}
