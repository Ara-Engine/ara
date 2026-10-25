use crate::{MeshSettings, TextureSettings, TextureUsage, Vertex, invalid, json_bytes};
use ara_asset::{
    Artifact, ArtifactKind, ImportContext, ImportInput, ImportOutput, Importer, Result,
};
use ara_asset_format::{Builder, Channel, Clip, Joint, Kind, write_skeleton};
use base64::Engine;
use rayon::prelude::*;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
mod opacity;
mod tangents;

/// glTF 2.0/GLB importer: geometry, skins, clips, PBR textures and scene hierarchy.
pub struct GltfImporter;
impl Importer for GltfImporter {
    fn id(&self) -> &str {
        "ara.gltf"
    }
    fn version(&self) -> &str {
        "7-gltf14-meshopt070-ispc050-texture-presets-tiled-f32text-arag1-khr-opacity-deep-lods-prefix"
    }
    fn register_settings(&self, db: &mut ara_schema::SchemaDb) {
        db.register::<MeshSettings>();
    }
    fn dependencies(&self, source: &[u8]) -> Result<Vec<String>> {
        let g = gltf::Gltf::from_slice(source).map_err(invalid)?;
        let mut deps = BTreeSet::new();
        for b in g.buffers() {
            if let gltf::buffer::Source::Uri(uri) = b.source()
                && !uri.starts_with("data:")
            {
                deps.insert(uri_path(uri)?);
            }
        }
        for image in g.images() {
            if let gltf::image::Source::Uri { uri, .. } = image.source()
                && !uri.starts_with("data:")
            {
                deps.insert(uri_path(uri)?);
            }
        }
        Ok(deps.into_iter().collect())
    }
    fn dependencies_with_settings(
        &self,
        source: &[u8],
        settings: &serde_json::Value,
    ) -> Result<Vec<String>> {
        let settings: MeshSettings = if settings.is_null() {
            MeshSettings::default()
        } else {
            serde_json::from_value(settings.clone())?
        };
        let mut dependencies: BTreeSet<_> = self.dependencies(source)?.into_iter().collect();
        dependencies.extend(opacity::dependencies(&settings.opacity_maps)?);
        Ok(dependencies.into_iter().collect())
    }
    fn import(&self, input: &ImportInput, ctx: &mut ImportContext<'_>) -> Result<ImportOutput> {
        self.import_with_memo(input, ctx, None)
    }
    fn import_with_memo(
        &self,
        input: &ImportInput,
        ctx: &mut ImportContext<'_>,
        memo: Option<&dyn ara_asset::CacheBackend>,
    ) -> Result<ImportOutput> {
        let mesh_settings = if input.settings.is_null() {
            MeshSettings::default()
        } else {
            serde_json::from_value(input.settings.clone())?
        };
        let parse_timer = ctx.metrics.map(|m| m.start(ara_asset::ImportStage::Parse));
        let gltf = gltf::Gltf::from_slice(&input.source).map_err(invalid)?;
        drop(parse_timer);
        let opacity_timer = ctx
            .metrics
            .map(|m| m.start(ara_asset::ImportStage::Opacity));
        let opacity_maps = opacity::resolve(&gltf, input, &mesh_settings.opacity_maps)?;
        drop(opacity_timer);
        let mut buffers = Vec::new();
        for b in gltf.buffers() {
            let data = match b.source() {
                gltf::buffer::Source::Bin => gltf
                    .blob
                    .clone()
                    .ok_or_else(|| invalid("GLB has no binary chunk"))?,
                gltf::buffer::Source::Uri(uri) => resource(uri, input)?,
            };
            if data.len() < b.length() {
                return Err(invalid("truncated glTF buffer"));
            }
            buffers.push(data);
        }
        let mut output = ImportOutput::default();
        ctx.report(0.05, "optimize glTF geometry")?;
        let cancellation = ctx.cancellation;
        let metrics = ctx.metrics;
        let skinned: BTreeSet<_> = gltf
            .nodes()
            .filter(|n| n.skin().is_some())
            .filter_map(|n| n.mesh().map(|m| m.index()))
            .collect();
        let geometry_key = memo
            .map(|_| crate::geometry_memo::key(input, &mesh_settings))
            .transpose()?;
        let mut mesh_map;
        if let Some(geometry) = crate::geometry_memo::load(memo, geometry_key)? {
            output.artifacts.extend(geometry.artifacts);
            mesh_map = geometry.meshes;
        } else {
            let material_counts: BTreeMap<_, _> = gltf
                .meshes()
                .map(|m| (m.index(), m.primitives().count()))
                .collect();
            let primitives: Vec<_> = gltf
                .meshes()
                .flat_map(|mesh| {
                    let index = mesh.index();
                    mesh.primitives().map(move |p| (index, p))
                })
                .collect();
            let mesh_timer = metrics.map(|m| m.start(ara_asset::ImportStage::Mesh));
            let meshes: Result<Vec<_>> = primitives.into_par_iter().map(|(mesh_index, primitive)| {
                cancellation.check()?;
                let reader = primitive.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
                let positions: Vec<_> = reader
                    .read_positions()
                    .ok_or_else(|| invalid("mesh primitive has no positions"))?
                    .collect();
                if positions.len() > u32::MAX as usize {
                    return Err(invalid("too many vertices"));
                }
                let mut vertices: Vec<_> = positions
                    .into_iter()
                    .enumerate()
                    .map(|(i, position)| Vertex {
                        position,
                        source_index: i as u32,
                        color: [1.0; 4],
                        ..Vertex::default()
                    })
                    .collect();
                let source_indices: Vec<_> = reader
                    .read_indices()
                    .map(|i| i.into_u32().collect())
                    .unwrap_or_else(|| (0..vertices.len() as u32).collect());
                let indices = triangles(&source_indices, primitive.mode())?;
                if indices.iter().any(|&i| i as usize >= vertices.len()) {
                    return Err(invalid("glTF index out of bounds"));
                }
                if let Some(normals) = reader.read_normals() {
                    for (v, n) in vertices.iter_mut().zip(normals) {
                        v.normal = n;
                    }
                } else {
                    crate::mesh::normals(&mut vertices, &indices);
                }
                if let Some(tangents) = reader.read_tangents() {
                    for (v, t) in vertices.iter_mut().zip(tangents) {
                        v.tangent = t;
                    }
                }
                if let Some(uv) = reader.read_tex_coords(0) {
                    for (v, t) in vertices.iter_mut().zip(uv.into_f32()) {
                        v.uv0 = t;
                    }
                }
                if let Some(uv) = reader.read_tex_coords(1) {
                    for (v, t) in vertices.iter_mut().zip(uv.into_f32()) {
                        v.uv1 = t;
                    }
                }
                if let Some(colors) = reader.read_colors(0) {
                    for (v, c) in vertices.iter_mut().zip(colors.into_rgba_f32()) {
                        v.color = c;
                    }
                }
                if let Some(joints) = reader.read_joints(0) {
                    for (v, j) in vertices.iter_mut().zip(joints.into_u16()) {
                        v.joints = j;
                    }
                }
                if let Some(weights) = reader.read_weights(0) {
                    for (v, w) in vertices.iter_mut().zip(weights.into_f32()) {
                        v.weights = w;
                    }
                }
                if let Some(joints) = reader.read_joints(1) {
                    for (v, j) in vertices.iter_mut().zip(joints.into_u16()) {
                        v.joints1 = j;
                    }
                }
                if let Some(weights) = reader.read_weights(1) {
                    for (v, w) in vertices.iter_mut().zip(weights.into_f32()) {
                        v.weights1 = w;
                    }
                }
                let name = format!("meshes/{}-{}.mesh", mesh_index, primitive.index());
                let deformed = skinned.contains(&mesh_index)
                    || primitive.get(&gltf::Semantic::Joints(0)).is_some()
                    || primitive.morph_targets().next().is_some();
                let normal_uv = tangents::NormalUv::for_material(&primitive.material())?;
                let material = primitive.material();
                let preserve_card_area = material.alpha_mode() != gltf::material::AlphaMode::Opaque
                    || material.pbr_metallic_roughness().base_color_texture()
                        .is_some_and(|texture| opacity_maps.contains_key(&texture.texture().source().index()));
                let artifact = Artifact {
                    name: name.clone(), kind: ArtifactKind::Mesh,
                    bytes: crate::mesh::cook_mesh_with_tangent_uv(&vertices, &indices, &mesh_settings, deformed, preserve_card_area, metrics, |v| normal_uv.apply(v))?,
                };
                let (virtual_artifact, virtual_metadata) = crate::virtual_geometry::cook(
                    &name, &vertices, &indices, &mesh_settings, deformed,
                    material_counts[&mesh_index])?;
                // Preserve all morph target deltas independently of quantized base geometry.
                let mut targets = Vec::new();
                for (positions, normals, tangents) in reader.read_morph_targets() {
                    targets.push(json!({"positions":positions.map(Iterator::collect::<Vec<_>>),"normals":normals.map(Iterator::collect::<Vec<_>>),"tangents":tangents.map(Iterator::collect::<Vec<_>>)}));
                }
                let metadata = json!({"mesh":mesh_index,"primitive":primitive.index(),"material":primitive.material().index(),"artifact":name,"morphTargets":targets,"virtualGeometry":virtual_metadata});
                Ok((artifact, virtual_artifact, metadata))
        }).collect();
            drop(mesh_timer);
            mesh_map = Vec::new();
            for (artifact, virtual_artifact, metadata) in meshes? {
                output.artifacts.push(artifact);
                output.artifacts.extend(virtual_artifact);
                mesh_map.push(metadata);
            }
            crate::geometry_memo::store(memo, geometry_key, &output.artifacts, &mesh_map)?;
        }
        let mut texture_usages: BTreeMap<(usize, u8), (TextureUsage, Vec<u8>)> = BTreeMap::new();
        for material in gltf.materials() {
            let pbr = material.pbr_metallic_roughness();
            for (texture, usage) in [
                (
                    pbr.base_color_texture().map(|t| t.texture()),
                    TextureUsage::Color,
                ),
                (
                    pbr.metallic_roughness_texture().map(|t| t.texture()),
                    TextureUsage::Linear,
                ),
                (
                    material.normal_texture().map(|t| t.texture()),
                    TextureUsage::Normal,
                ),
                (
                    material.occlusion_texture().map(|t| t.texture()),
                    TextureUsage::Linear,
                ),
                (
                    material.emissive_texture().map(|t| t.texture()),
                    TextureUsage::Color,
                ),
                (
                    material
                        .transmission()
                        .and_then(|t| t.transmission_texture())
                        .map(|t| t.texture()),
                    TextureUsage::Linear,
                ),
            ] {
                if let Some(texture) = texture {
                    let image = texture.source();
                    let key = (image.index(), usage as u8);
                    if let std::collections::btree_map::Entry::Vacant(entry) =
                        texture_usages.entry(key)
                    {
                        let mut bytes = match image.source() {
                            gltf::image::Source::Uri { uri, .. } => resource(uri, input)?,
                            gltf::image::Source::View { view, .. } => {
                                let data = &buffers[view.buffer().index()];
                                let end = view
                                    .offset()
                                    .checked_add(view.length())
                                    .ok_or_else(|| invalid("image buffer range overflow"))?;
                                data.get(view.offset()..end)
                                    .ok_or_else(|| invalid("image buffer out of bounds"))?
                                    .to_vec()
                            }
                        };
                        if usage == TextureUsage::Color
                            && let Some(mask) = opacity_maps.get(&image.index())
                        {
                            let _timer = ctx
                                .metrics
                                .map(|m| m.start(ara_asset::ImportStage::Opacity));
                            bytes = opacity::merge(&bytes, mask)?;
                        }
                        entry.insert((usage, bytes));
                    }
                }
            }
        }
        let mut texture_map = Vec::new();
        let mut textures: Vec<_> = texture_usages.into_iter().collect();
        let jobs: Vec<_> = textures
            .iter_mut()
            .map(|((image, _), (usage, bytes))| {
                let preset = mesh_settings
                    .texture_presets
                    .get(&format!("#{image}"))
                    .copied()
                    .or_else(|| {
                        gltf.images()
                            .nth(*image)
                            .and_then(|image| match image.source() {
                                gltf::image::Source::Uri { uri, .. } => {
                                    mesh_settings.texture_presets.get(uri).copied()
                                }
                                _ => None,
                            })
                    });
                (
                    TextureSettings {
                        preset,
                        usage: *usage,
                        ..Default::default()
                    },
                    std::mem::take(bytes),
                )
            })
            .collect();
        let cooked =
            crate::texture::cook_texture_batch(&jobs, input.target, input.preset, ctx, memo)?;
        for (((image, _), (usage, _)), bytes) in textures.into_iter().zip(cooked) {
            let name = format!("textures/{image}-{usage:?}.ktx2");
            output.artifacts.push(Artifact {
                name: name.clone(),
                kind: ArtifactKind::Texture,
                bytes,
            });
            texture_map.push(json!({"image":image,"usage":usage,"artifact":name}));
        }
        let mut skeleton_map = Vec::new();
        let parents: BTreeMap<_, _> = gltf
            .nodes()
            .flat_map(|n| {
                let parent = n.index();
                n.children().map(move |c| (c.index(), parent))
            })
            .collect();
        for skin in gltf.skins() {
            ctx.report(0.92, "cook skin bindings")?;
            let joints: Vec<_> = skin.joints().collect();
            let index: BTreeMap<_, _> = joints
                .iter()
                .enumerate()
                .map(|(i, n)| (n.index(), i as i32))
                .collect();
            let reader = skin.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
            let inverse: Vec<_> = reader
                .read_inverse_bind_matrices()
                .map(Iterator::collect)
                .unwrap_or_else(|| vec![identity(); joints.len()]);
            if inverse.len() != joints.len() {
                return Err(invalid("inverse-bind matrix count differs from joints"));
            }
            let bindings: Vec<_> = joints
                .iter()
                .enumerate()
                .map(|(i, n)| {
                    let (t, r, s) = n.transform().decomposed();
                    let mut p = parents.get(&n.index()).copied();
                    let mut parent = -1;
                    while let Some(node) = p {
                        if let Some(&joint) = index.get(&node) {
                            parent = joint;
                            break;
                        }
                        p = parents.get(&node).copied();
                    }
                    Joint {
                        parent,
                        node: n.index() as u32,
                        inverse_bind: std::array::from_fn(|k| inverse[i][k / 4][k % 4]),
                        rest: [t[0], t[1], t[2], r[0], r[1], r[2], r[3], s[0], s[1], s[2]],
                    }
                })
                .collect();
            let names: Vec<_> = joints.iter().map(|n| n.name().unwrap_or("")).collect();
            let bytes = write_skeleton(&bindings, &names)?;
            let name = format!("skeletons/{}.skeleton", skin.index());
            output.artifacts.push(Artifact {
                name: name.clone(),
                kind: ArtifactKind::Skeleton,
                bytes,
            });
            skeleton_map.push(json!({"skin":skin.index(),"artifact":name}));
        }
        let mut clip_map = Vec::new();
        for animation in gltf.animations() {
            ctx.report(0.95, "cook animation channels")?;
            let mut channels = Vec::new();
            let mut times = Vec::<f32>::new();
            let mut values = Vec::<f32>::new();
            for channel in animation.channels() {
                let reader = channel.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
                let input_times: Vec<_> = reader
                    .read_inputs()
                    .ok_or_else(|| invalid("missing animation input"))?
                    .collect();
                let time_offset = times.len() as u32;
                let key_count = input_times.len() as u32;
                let value_offset = values.len() as u32;
                let path = match channel.target().property() {
                    gltf::animation::Property::Translation => 0,
                    gltf::animation::Property::Rotation => 1,
                    gltf::animation::Property::Scale => 2,
                    gltf::animation::Property::MorphTargetWeights => 3,
                };
                let interpolation = match channel.sampler().interpolation() {
                    gltf::animation::Interpolation::Step => 0,
                    gltf::animation::Interpolation::Linear => 1,
                    gltf::animation::Interpolation::CubicSpline => 2,
                };
                use gltf::animation::util::ReadOutputs;
                match reader
                    .read_outputs()
                    .ok_or_else(|| invalid("missing animation output"))?
                {
                    ReadOutputs::Translations(v) | ReadOutputs::Scales(v) => {
                        values.extend(v.flatten())
                    }
                    ReadOutputs::Rotations(v) => values.extend(v.into_f32().flatten()),
                    ReadOutputs::MorphTargetWeights(v) => values.extend(v.into_f32()),
                };
                let divisor = key_count as usize * if interpolation == 2 { 3 } else { 1 };
                if divisor == 0 || !(values.len() - value_offset as usize).is_multiple_of(divisor) {
                    return Err(invalid("invalid animation sample count"));
                }
                let components = (values.len() - value_offset as usize) / divisor;
                channels.push(Channel {
                    node: channel.target().node().index() as u32,
                    path,
                    interpolation,
                    time_offset,
                    key_count,
                    value_offset,
                    components: components as u32,
                    reserved: 0,
                });
                times.extend(input_times);
            }
            let mut b = Builder::new(Kind::Clip);
            b.section(*b"CHAN", 32, bytemuck::cast_slice(&channels).to_vec())
                .section(*b"TIME", 4, bytemuck::cast_slice(&times).to_vec())
                .section(*b"VALU", 4, bytemuck::cast_slice(&values).to_vec());
            let bytes = b.finish()?;
            Clip::read(&bytes)?;
            let name = format!("clips/{}.clip", animation.index());
            output.artifacts.push(Artifact {
                name: name.clone(),
                kind: ArtifactKind::Clip,
                bytes,
            });
            clip_map.push(json!({"animation":animation.index(),"artifact":name}));
        }
        let mut document = serde_json::to_value(gltf.document.as_json())?;
        opacity::alpha_modes(&mut document, &gltf, &opacity_maps);
        let manifest = json!({"schema":"ara://cooked/scene/1","sourceFormat":"gltf2","document":document,"meshes":mesh_map,"textures":texture_map,"skeletons":skeleton_map,"clips":clip_map});
        output.artifacts.push(Artifact {
            name: "scene.json".into(),
            kind: ArtifactKind::Scene,
            bytes: json_bytes(&manifest)?,
        });
        Ok(output)
    }
}
fn identity() -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}
fn uri_path(uri: &str) -> Result<String> {
    let path = urlencoding::decode(uri).map_err(invalid)?.into_owned();
    if path.contains(':') || path.contains('\\') || path.starts_with('/') || path.contains('\0') {
        return Err(invalid("glTF external URI must be relative"));
    }
    Ok(path)
}
fn resource(uri: &str, input: &ImportInput) -> Result<Vec<u8>> {
    if let Some(data) = uri.strip_prefix("data:") {
        let (metadata, bytes) = data
            .split_once(',')
            .ok_or_else(|| invalid("invalid data URI"))?;
        if metadata.ends_with(";base64") {
            base64::engine::general_purpose::STANDARD
                .decode(bytes)
                .map_err(invalid)
        } else {
            Ok(urlencoding::decode_binary(bytes.as_bytes()).into_owned())
        }
    } else {
        input
            .dependencies
            .get(&uri_path(uri)?)
            .cloned()
            .ok_or_else(|| invalid(format!("missing glTF dependency {uri}")))
    }
}
fn triangles(indices: &[u32], mode: gltf::mesh::Mode) -> Result<Vec<u32>> {
    use gltf::mesh::Mode;
    let mut result = Vec::new();
    match mode {
        Mode::Triangles => {
            if !indices.len().is_multiple_of(3) {
                return Err(invalid("incomplete triangle"));
            }
            result.extend(indices);
        }
        Mode::TriangleStrip => {
            for i in 2..indices.len() {
                let tri = if i % 2 == 0 {
                    [indices[i - 2], indices[i - 1], indices[i]]
                } else {
                    [indices[i - 1], indices[i - 2], indices[i]]
                };
                if tri[0] != tri[1] && tri[1] != tri[2] && tri[0] != tri[2] {
                    result.extend(tri);
                }
            }
        }
        Mode::TriangleFan => {
            for i in 2..indices.len() {
                result.extend([indices[0], indices[i - 1], indices[i]]);
            }
        }
        _ => {
            return Err(invalid(
                "non-triangle glTF primitives require a line/point renderer importer",
            ));
        }
    }
    Ok(result)
}
