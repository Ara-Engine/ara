#![allow(clippy::unwrap_used)]
use ara_asset::*;
use ara_importers::*;
use base64::Engine;
use image::{ImageEncoder, RgbaImage};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn context<'a>(
    cancel: &'a Cancellation,
    progress: &'a mut (dyn FnMut(Progress) + Send),
) -> ImportContext<'a> {
    ImportContext {
        metrics: None,
        cancellation: cancel,
        progress,
    }
}
fn input(source: Vec<u8>, importer: &dyn Importer, settings: Value) -> ImportInput {
    ImportInput {
        preset: Default::default(),
        thread_budget: 16,
        source_path: "fixture".into(),
        source,
        importer: importer.id().into(),
        version: importer.version().into(),
        settings,
        target: TargetPlatform::Windows,
        dependencies: BTreeMap::new(),
        asset_dependencies: BTreeMap::new(),
    }
}
fn png(w: u32, h: u32) -> Vec<u8> {
    let image = RgbaImage::from_fn(w, h, |x, y| {
        image::Rgba([(x * 255 / w) as u8, (y * 255 / h) as u8, 128, 255])
    });
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(&image, w, h, image::ExtendedColorType::Rgba8)
        .unwrap();
    bytes
}
fn cook(input: &ImportInput) -> ImportOutput {
    registry()
        .unwrap()
        .import(input, &mut context(&Cancellation::default(), &mut |_| {}))
        .unwrap()
}

#[test]
fn importer_settings_are_reflected_with_defaults_and_constraints() {
    let mut db = ara_schema::SchemaDb::new();
    ara_asset::register_schema(&mut db);
    registry().unwrap().register_schemas(&mut db);
    assert!(db.verify().is_empty(), "{:?}", db.verify());
    for name in [
        "ImportSettings",
        "MeshSettings",
        "TextureSettings",
        "TextureUsage",
        "VirtualGeometrySettings",
    ] {
        assert!(db.get(name).is_some());
    }
    let settings = db.get("MeshSettings").unwrap();
    assert_eq!(
        db.field_default(settings.field("lodCount").unwrap()),
        json!(4)
    );
    assert_eq!(
        db.field_default(db.get("TextureSettings").unwrap().field("mipmaps").unwrap()),
        json!(true)
    );
}

#[test]
fn virtual_geometry_runs_through_the_builtin_asset_worker_registry() {
    let source = ara_geo::sample::rock(12, 24);
    let bytes = ara_json::to_canonical_string(&ara_json::to_value(&source).unwrap()).into_bytes();
    let mut request = input(bytes, &ara_geo::GeoImporter, json!({"groupSize":8}));
    request.thread_budget = 1;
    let a = cook(&request);
    request.thread_budget = 2;
    let b = cook(&request);
    assert_eq!(a, b);
    assert_eq!(a.artifacts.len(), 1);
    let geometry = ara_geo::Geometry::decode(&a.artifacts[0].bytes).unwrap();
    assert_eq!(
        geometry.diagnostics.source_triangles,
        source.indices.len() as u64 / 3
    );
}

#[test]
fn all_bcn_usages_generate_deterministic_complete_npot_mipchains() {
    for (usage, format) in [
        (
            TextureUsage::Color,
            ara_asset_format::TextureFormat::Bc7Srgb,
        ),
        (TextureUsage::Linear, ara_asset_format::TextureFormat::Bc7),
        (TextureUsage::Normal, ara_asset_format::TextureFormat::Bc5),
        (TextureUsage::Mask, ara_asset_format::TextureFormat::Bc4),
        (TextureUsage::Hdr, ara_asset_format::TextureFormat::Bc6h),
    ] {
        let input = input(png(9, 5), &TextureImporter, json!({"usage":usage}));
        let first = cook(&input);
        assert_eq!(first, cook(&input));
        let texture = ara_asset_format::Texture::read(&first.artifacts[0].bytes).unwrap();
        assert_eq!(texture.format(), format);
        assert_eq!(texture.levels().len(), 4);
    }
}
#[test]
fn bc7_quality_is_measured_by_an_independent_decoder() {
    let bytes = png(128, 96);
    let reference = image::load_from_memory(&bytes).unwrap().into_rgba8();
    let start = std::time::Instant::now();
    let output = cook(&input(bytes, &TextureImporter, Value::Null));
    let elapsed = start.elapsed();
    let texture = ara_asset_format::Texture::read(&output.artifacts[0].bytes).unwrap();
    let mut decoded = vec![0; 128 * 96];
    texture2ddecoder::decode_bc7(texture.levels().next().unwrap(), 128, 96, &mut decoded).unwrap();
    let mut square = 0.0;
    for (p, &d) in reference.pixels().zip(&decoded) {
        let channels = d.to_le_bytes();
        for (a, b) in p.0[..3].iter().zip([channels[2], channels[1], channels[0]]) {
            square += (*a as f64 - b as f64).powi(2);
        }
    }
    let mse = square / (decoded.len() * 3) as f64;
    let psnr = 10.0 * (255.0f64.powi(2) / mse).log10();
    println!(
        "BC7 gradient: {psnr:.3} dB RGB PSNR, {} pixels, {:.3} ms cook",
        decoded.len(),
        elapsed.as_secs_f64() * 1000.0
    );
    assert!(psnr > 40.0, "BC7 quality regression: {psnr:.3}");
    for (label, settings) in [
        ("basic", intel_tex_2::bc7::opaque_basic_settings()),
        ("slow", intel_tex_2::bc7::opaque_slow_settings()),
    ] {
        let start = std::time::Instant::now();
        let blocks = intel_tex_2::bc7::compress_blocks(
            &settings,
            &intel_tex_2::RgbaSurface {
                data: reference.as_raw(),
                width: 128,
                height: 96,
                stride: 128 * 4,
            },
        );
        let elapsed = start.elapsed();
        let mut decoded = vec![0; 128 * 96];
        texture2ddecoder::decode_bc7(&blocks, 128, 96, &mut decoded).unwrap();
        let square: f64 = reference
            .pixels()
            .zip(&decoded)
            .map(|(p, &d)| {
                let bytes = d.to_le_bytes();
                p.0[..3]
                    .iter()
                    .zip([bytes[2], bytes[1], bytes[0]])
                    .map(|(a, b)| (*a as f64 - b as f64).powi(2))
                    .sum::<f64>()
            })
            .sum();
        let psnr = 10.0 * (255.0f64.powi(2) / (square / (decoded.len() * 3) as f64)).log10();
        println!(
            "ISPC BC7 {label}: {psnr:.3} dB, {:.3} ms kernel",
            elapsed.as_secs_f64() * 1000.0
        );
    }
}
#[test]
fn normal_mips_renormalize_and_srgb_mips_filter_in_linear_light() {
    let mut pixels = RgbaImage::new(2, 1);
    pixels.put_pixel(0, 0, image::Rgba([0, 0, 0, 255]));
    pixels.put_pixel(1, 0, image::Rgba([255, 255, 255, 255]));
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&pixels, 2, 1, image::ExtendedColorType::Rgba8)
        .unwrap();
    let output = cook(&input(png, &TextureImporter, Value::Null));
    let t = ara_asset_format::Texture::read(&output.artifacts[0].bytes).unwrap();
    let mut decoded = [0u32; 1];
    texture2ddecoder::decode_bc7(t.levels().last().unwrap(), 1, 1, &mut decoded).unwrap();
    let red = (decoded[0] >> 16) & 255;
    assert!(
        (184..=191).contains(&red),
        "sRGB mip must encode linear 0.5, got {red}"
    );
    let output = cook(&input(
        png_normal(),
        &TextureImporter,
        json!({"usage":"normal"}),
    ));
    let t = ara_asset_format::Texture::read(&output.artifacts[0].bytes).unwrap();
    let mut decoded = [0u32; 1];
    texture2ddecoder::decode_bc5(t.levels().last().unwrap(), 1, 1, &mut decoded).unwrap();
    let red = (decoded[0] >> 16) & 255;
    let green = (decoded[0] >> 8) & 255;
    assert!((124..=132).contains(&red));
    assert!((124..=132).contains(&green));
}
fn png_normal() -> Vec<u8> {
    let pixels = [255u8, 128, 255, 255, 0, 127, 255, 255];
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(&pixels, 2, 1, image::ExtendedColorType::Rgba8)
        .unwrap();
    bytes
}

fn gltf_fixture() -> Vec<u8> {
    let mut binary = Vec::new();
    let mut views = Vec::new();
    let mut accessors = Vec::new();
    let mut add = |bytes: &[u8], component: u32, count: u32, ty: &str, extra: Value| {
        binary.resize(binary.len().next_multiple_of(4), 0);
        let offset = binary.len();
        binary.extend(bytes);
        let view = views.len();
        views.push(json!({"buffer":0,"byteOffset":offset,"byteLength":bytes.len()}));
        let mut accessor =
            json!({"bufferView":view,"componentType":component,"count":count,"type":ty});
        for (k, v) in extra.as_object().unwrap() {
            accessor[k] = v.clone();
        }
        let i = accessors.len();
        accessors.push(accessor);
        i
    };
    let positions = add(
        bytemuck::cast_slice(&[0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
        5126,
        3,
        "VEC3",
        json!({"min":[0,0,0],"max":[1,1,0]}),
    );
    let joints = add(&[0u8; 12], 5121, 3, "VEC4", json!({}));
    let weights = add(
        bytemuck::cast_slice(&[
            1.0f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0,
        ]),
        5126,
        3,
        "VEC4",
        json!({}),
    );
    let times = add(
        bytemuck::cast_slice(&[0.0f32, 1.0]),
        5126,
        2,
        "SCALAR",
        json!({"min":[0],"max":[1]}),
    );
    let values = add(
        bytemuck::cast_slice(&[0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0]),
        5126,
        2,
        "VEC3",
        json!({}),
    );
    let encoded = base64::engine::general_purpose::STANDARD.encode(&binary);
    let image = base64::engine::general_purpose::STANDARD.encode(png(5, 3));
    serde_json::to_vec(&json!({"asset":{"version":"2.0"},"buffers":[{"uri":format!("data:application/octet-stream;base64,{encoded}"),"byteLength":binary.len()}],"bufferViews":views,"accessors":accessors,"meshes":[{"primitives":[{"attributes":{"POSITION":positions,"JOINTS_0":joints,"WEIGHTS_0":weights},"material":0}]}],"images":[{"uri":format!("data:image/png;base64,{image}")}],"textures":[{"source":0}],"materials":[{"pbrMetallicRoughness":{"baseColorTexture":{"index":0}},"normalTexture":{"index":0}}],"nodes":[{"mesh":0,"skin":0,"children":[1]},{"name":"root"}],"skins":[{"joints":[1]}],"animations":[{"samplers":[{"input":times,"output":values,"interpolation":"LINEAR"}],"channels":[{"sampler":0,"target":{"node":1,"path":"translation"}}]}],"scenes":[{"nodes":[0]}],"scene":0})).unwrap()
}
#[test]
fn generated_gltf_cooks_mesh_skin_clip_materials_and_hierarchy_byte_identically() {
    let mut input = input(gltf_fixture(), &GltfImporter, Value::Null);
    let first = cook(&input);
    input.thread_budget = 1;
    assert_eq!(first, cook(&input));
    assert_eq!(first.artifacts.len(), 6);
    assert!(first.artifacts.iter().all(|a| !a.name.ends_with(".arag")));
    let mesh = first
        .artifacts
        .iter()
        .find(|a| a.kind == ArtifactKind::Mesh)
        .unwrap();
    let mesh = ara_asset_format::Mesh::read(&mesh.bytes).unwrap();
    assert_eq!(mesh.vertices.len() / 52, 3);
    assert_eq!(mesh.meshlets.len() / 48, 1);
    let skin = first
        .artifacts
        .iter()
        .find(|a| a.kind == ArtifactKind::Skeleton)
        .unwrap();
    assert_eq!(
        ara_asset_format::Skeleton::read(&skin.bytes)
            .unwrap()
            .joints
            .len(),
        112
    );
    let clip = first
        .artifacts
        .iter()
        .find(|a| a.kind == ArtifactKind::Clip)
        .unwrap();
    assert_eq!(
        ara_asset_format::Clip::read(&clip.bytes)
            .unwrap()
            .times
            .len(),
        8
    );
    let scene = first
        .artifacts
        .iter()
        .find(|a| a.kind == ArtifactKind::Scene)
        .unwrap();
    let manifest: Value = serde_json::from_slice(&scene.bytes).unwrap();
    assert_eq!(manifest["document"]["nodes"][0]["children"], json!([1]));
    // Exercise independent mesh jobs and stable manifest ordering, not only one primitive.
    let mut fixture: Value = serde_json::from_slice(&input.source).unwrap();
    fixture["meshes"] = Value::Array(vec![fixture["meshes"][0].clone(); 8]);
    input.source = serde_json::to_vec(&fixture).unwrap();
    let serial = cook(&input);
    input.thread_budget = 16;
    assert_eq!(serial, cook(&input));
    assert_eq!(
        serial
            .artifacts
            .iter()
            .filter(|a| a.kind == ArtifactKind::Mesh)
            .count(),
        8
    );
}

#[test]
fn static_gltf_automatically_cooks_a_versioned_dag_and_supports_opt_out() {
    let mut fixture: Value = serde_json::from_slice(&gltf_fixture()).unwrap();
    fixture["nodes"][0].as_object_mut().unwrap().remove("skin");
    fixture.as_object_mut().unwrap().remove("skins");
    let attributes = fixture["meshes"][0]["primitives"][0]["attributes"]
        .as_object_mut()
        .unwrap();
    attributes.remove("JOINTS_0");
    attributes.remove("WEIGHTS_0");
    let mut request = input(
        serde_json::to_vec(&fixture).unwrap(),
        &GltfImporter,
        Value::Null,
    );
    let first = cook(&request);
    request.thread_budget = 1;
    assert_eq!(first, cook(&request));
    let artifact = first
        .artifacts
        .iter()
        .find(|a| a.name.ends_with(".arag"))
        .unwrap();
    assert_eq!(
        ara_geo::Geometry::decode(&artifact.bytes)
            .unwrap()
            .diagnostics
            .source_triangles,
        1
    );
    let automatic_key = request.key().unwrap();
    request.settings = json!({"virtualGeometry":false});
    assert_ne!(automatic_key, request.key().unwrap());
    assert!(
        cook(&request)
            .artifacts
            .iter()
            .all(|a| !a.name.ends_with(".arag"))
    );
}
#[test]
fn external_dependency_discovery_is_sorted_and_missing_inputs_fail() {
    let mut gltf: Value = serde_json::from_slice(&gltf_fixture()).unwrap();
    gltf["images"][0]["uri"] = json!("textures/normal%20map.png");
    let source = serde_json::to_vec(&gltf).unwrap();
    assert_eq!(
        GltfImporter.dependencies(&source).unwrap(),
        ["textures/normal map.png"]
    );
    let input = input(source, &GltfImporter, Value::Null);
    assert!(
        registry()
            .unwrap()
            .import(&input, &mut context(&Cancellation::default(), &mut |_| {}))
            .is_err()
    );
}
#[test]
fn cancellation_and_mobile_profiles_are_explicit() {
    let cancel = Cancellation::default();
    cancel.cancel();
    let mut progress = |_| {};
    let mut ctx = context(&cancel, &mut progress);
    assert!(matches!(
        cook_texture(
            &png(4, 4),
            &TextureSettings::default(),
            TargetPlatform::Windows,
            &mut ctx
        ),
        Err(Error::Cancelled)
    ));
    let cancel = Cancellation::default();
    let mut progress = |_| {};
    let mut ctx = context(&cancel, &mut progress);
    assert!(
        cook_texture(
            &png(4, 4),
            &TextureSettings::default(),
            TargetPlatform::Android,
            &mut ctx
        )
        .is_err()
    );
}

#[test]
fn generated_fbx_cooks_geometry_and_hierarchy_deterministically() {
    let input = input(
        include_bytes!("fixtures/triangle.fbx").to_vec(),
        &FbxImporter,
        Value::Null,
    );
    assert!(FbxImporter.dependencies(&input.source).unwrap().is_empty());
    let first = cook(&input);
    assert_eq!(first, cook(&input));
    let mesh = first
        .artifacts
        .iter()
        .find(|a| a.kind == ArtifactKind::Mesh)
        .unwrap();
    let mesh = ara_asset_format::Mesh::read(&mesh.bytes).unwrap();
    assert_eq!(mesh.vertices.len() / 52, 3);
    let scene = first
        .artifacts
        .iter()
        .find(|a| a.kind == ArtifactKind::Scene)
        .unwrap();
    let scene: Value = serde_json::from_slice(&scene.bytes).unwrap();
    assert_eq!(scene["sourceFormat"], "fbx");
    assert!(scene["nodes"].as_array().unwrap().len() >= 2);
}

#[test]
fn jpeg_hdr_and_exr_decode_to_correct_cooked_transfer_functions() {
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95)
        .encode(&[128u8; 8 * 8 * 3], 8, 8, image::ExtendedColorType::Rgb8)
        .unwrap();
    let output = cook(&input(jpeg, &TextureImporter, Value::Null));
    assert_eq!(
        ara_asset_format::Texture::read(&output.artifacts[0].bytes)
            .unwrap()
            .format(),
        ara_asset_format::TextureFormat::Bc7Srgb
    );
    let mut hdr = Vec::new();
    let pixels = vec![image::Rgb([4.0f32, 0.5, 1.5]); 16];
    image::codecs::hdr::HdrEncoder::new(&mut hdr)
        .encode(&pixels, 4, 4)
        .unwrap();
    let output = cook(&input(hdr, &TextureImporter, Value::Null));
    assert_eq!(
        ara_asset_format::Texture::read(&output.artifacts[0].bytes)
            .unwrap()
            .format(),
        ara_asset_format::TextureFormat::Bc6h
    );
    let image = image::DynamicImage::ImageRgba32F(image::ImageBuffer::from_pixel(
        4,
        4,
        image::Rgba([4.0, 0.5, 1.5, 1.0]),
    ));
    let mut exr = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut exr, image::ImageFormat::OpenExr)
        .unwrap();
    let output = cook(&input(exr.into_inner(), &TextureImporter, Value::Null));
    assert_eq!(
        ara_asset_format::Texture::read(&output.artifacts[0].bytes)
            .unwrap()
            .format(),
        ara_asset_format::TextureFormat::Bc6h
    );
}

#[test]
fn signed_hdr_and_hdr_alpha_retain_values_in_float_ktx2() {
    for (value, format) in [
        (
            [-2.0f32, 0.5, 1.0, 0.5],
            ara_asset_format::TextureFormat::Rgba16Float,
        ),
        (
            [100000.0f32, 0.5, 1.0, 1.0],
            ara_asset_format::TextureFormat::Rgba32Float,
        ),
        (
            [3e38f32, 0.5, 1.0, 1.0],
            ara_asset_format::TextureFormat::Rgba32Float,
        ),
    ] {
        let image = image::DynamicImage::ImageRgba32F(image::ImageBuffer::from_pixel(
            4,
            4,
            image::Rgba(value),
        ));
        let mut exr = std::io::Cursor::new(Vec::new());
        image
            .write_to(&mut exr, image::ImageFormat::OpenExr)
            .unwrap();
        let output = cook(&input(exr.into_inner(), &TextureImporter, Value::Null));
        let texture = ara_asset_format::Texture::read(&output.artifacts[0].bytes).unwrap();
        assert_eq!(texture.format(), format);
    }
}

#[test]
fn mesh_cook_emits_reducing_lods_meshlets_and_validates_bad_indices() {
    let n = 17u32;
    let vertices: Vec<_> = (0..n * n)
        .map(|i| Vertex {
            position: [(i % n) as f32, (i / n) as f32, 0.0],
            normal: [0.0, 0.0, 1.0],
            tangent: [1.0, 0.0, 0.0, 1.0],
            uv0: [(i % n) as f32 / 16.0, (i / n) as f32 / 16.0],
            color: [1.0; 4],
            source_index: i,
            ..Default::default()
        })
        .collect();
    let mut indices = Vec::new();
    for y in 0..n - 1 {
        for x in 0..n - 1 {
            let a = y * n + x;
            indices.extend([a, a + 1, a + n, a + 1, a + n + 1, a + n]);
        }
    }
    let bytes = cook_mesh(&vertices, &indices, MeshSettings::default()).unwrap();
    assert_eq!(
        bytes,
        cook_mesh(&vertices, &indices, MeshSettings::default()).unwrap()
    );
    let mesh = ara_asset_format::Mesh::read(&bytes).unwrap();
    assert_eq!(mesh.lods.len() / 16, 4);
    assert!(mesh.meshlets.len() / 48 > 1);
    let counts: Vec<_> = mesh
        .lods
        .as_chunks::<16>()
        .0
        .iter()
        .map(|lod| u32::from_le_bytes(lod[4..8].try_into().unwrap()))
        .collect();
    assert!(counts.windows(2).all(|pair| pair[1] < pair[0]));
    let start = mesh.indices.as_ptr() as usize - bytes.as_ptr() as usize;
    let mut invalid = bytes;
    invalid[start..start + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(ara_asset_format::Mesh::read(&invalid).is_err());
}
