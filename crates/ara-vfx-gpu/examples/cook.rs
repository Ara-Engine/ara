//! Explicit offline tool; never invoked by a build script or the runtime.
use anyhow::Result;
use ara_gpu::cooked::{RenderState, Stage};
use ara_shader_cook::{PipelineSource, Toolchain};
use ara_vfx_gpu::COMPUTE_ENTRIES;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let sdk = std::env::var_os("VULKAN_SDK")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:/VulkanSDK/1.4.363.0"));
    let tools = Toolchain {
        slangc: sdk.join("Bin/slangc.exe"),
        spirv_val: sdk.join("Bin/spirv-val.exe"),
    };
    let mut effects = ara_vfx::samples::all()
        .into_iter()
        .map(|(e, _)| e)
        .collect::<Vec<_>>();
    effects.push(ara_vfx::samples::million());
    // Exercise the two additional geometry paths with real offline cooks too.
    let mut ribbon = ara_vfx::samples::magic_swirl();
    ribbon.name = "ribbon".into();
    ribbon.emitters[0].stacks.render[0].module = ara_vfx::Module::Ribbon {
        width: 0.025,
        domain: ara_material::Domain::Unlit,
    };
    effects.push(ribbon);
    let mut mesh = ara_vfx::samples::sparks();
    mesh.name = "mesh".into();
    mesh.emitters[0].stacks.render[0].module = ara_vfx::Module::Mesh {
        triangles: vec![[[-0.5, 0., 0.], [0.5, 0., 0.], [0., 1., 0.]]],
        domain: ara_material::Domain::Surface,
        sorting: ara_vfx::Sorting::Age,
    };
    effects.push(mesh);
    let selected = std::env::args().nth(1);
    for effect in effects {
        if selected.as_ref().is_some_and(|name| name != &effect.name) {
            continue;
        }
        let source = root.join("generated").join(&effect.name);
        std::fs::create_dir_all(&source)?;
        std::fs::write(
            source.join("effect.slang"),
            ara_vfx_gpu::cook_source(&effect, None)?,
        )?;
        let mut manifest = BTreeMap::new();
        for entry in COMPUTE_ENTRIES {
            manifest.insert(
                (*entry).to_owned(),
                PipelineSource {
                    source: "effect.slang".into(),
                    entries: BTreeMap::from([((*entry).into(), Stage::Compute)]),
                    state: RenderState::default(),
                },
            );
        }
        for (name, fragment, formats, blend) in [
            ("backdrop", "backdropFragment", vec![109, 100], false),
            ("particles", "particleFragment", vec![109], true),
            ("tonemap", "tonemapFragment", vec![37, 100], false),
        ] {
            let vertex = if name == "particles" {
                "particleVertex"
            } else {
                "screenVertex"
            };
            manifest.insert(
                name.into(),
                PipelineSource {
                    source: "effect.slang".into(),
                    entries: BTreeMap::from([
                        (vertex.into(), Stage::Vertex),
                        (fragment.into(), Stage::Fragment),
                    ]),
                    state: RenderState {
                        color_formats: formats,
                        blend,
                        ..RenderState::default()
                    },
                },
            );
        }
        // The canonical writer owns text ordering for this derived manifest.
        let value = serde_json::to_value(
            manifest
                .iter()
                .map(|(name, p)| {
                    (
                        name,
                        serde_json::json!({"source":p.source,"entries":p.entries,"state":p.state}),
                    )
                })
                .collect::<BTreeMap<_, _>>(),
        )?;
        let path = source.join("pipelines.json");
        std::fs::write(&path, ara_json::to_canonical_string(&value))?;
        let output = root.join("cooked").join(&effect.name);
        let count = ara_shader_cook::cook(&path, &output, &tools)?;
        let metadata =
            serde_json::json!({"hash":effect.content_hash().map_err(|d|anyhow::anyhow!("{d:?}"))?});
        std::fs::write(
            output.join("effect.json"),
            ara_json::to_canonical_string(&metadata),
        )?;
        let samples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../ara-vfx/examples");
        std::fs::create_dir_all(&samples)?;
        std::fs::write(
            samples.join(format!("{}.vfx", effect.name)),
            effect.text().map_err(|d| anyhow::anyhow!("{d:?}"))?,
        )?;
        println!("{}: {count} pipelines cooked", effect.name);
    }
    Ok(())
}
