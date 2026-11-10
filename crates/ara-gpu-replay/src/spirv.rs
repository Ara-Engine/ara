//! WGSL resource-interface adapter to Ara's runtime descriptor arrays and push roots.
use crate::{Pass, SpirvStage, parse, validate};
use anyhow::{Context, Result, ensure};
use std::collections::BTreeMap;

/// Compile WGSL math with heap-indexed bindings. The tone LUT's temporary binding
/// is aliased onto sampled binding 5 in the emitted decorations, because naga's
/// validator rejects different typed declarations sharing one source binding.
pub fn spirv(pass: Pass) -> Result<Vec<SpirvStage>> {
    let source = adapt(pass);
    let module = parse(&source)?;
    let entries: Vec<_> = if matches!(pass, Pass::Tone) {
        vec![
            (naga::ShaderStage::Vertex, "vs"),
            (naga::ShaderStage::Fragment, "fs"),
        ]
    } else {
        vec![(naga::ShaderStage::Compute, pass.entry())]
    };
    entries
        .into_iter()
        .map(|(stage, entry)| {
            // Ara's cooked Vulkan ABI uses one `main` entry per module. Rename
            // the selected IR entry without editing the pinned WGSL shader body.
            let mut selected = module.clone();
            selected
                .entry_points
                .iter_mut()
                .find(|e| e.stage == stage && e.name == entry)
                .context("replay entry missing")?
                .name = "main".into();
            let info = validate(&selected)?;
            let mut words = naga::back::spv::write_vec(
                &selected,
                &info,
                &naga::back::spv::Options {
                    lang_version: (1, 5),
                    ..Default::default()
                },
                Some(&naga::back::spv::PipelineOptions {
                    shader_stage: stage,
                    entry_point: "main".into(),
                }),
            )?;
            // Naga 30 emits OpTypeRuntimeArray for descriptor binding arrays but does
            // not declare RuntimeDescriptorArray when their indices are uniform. The
            // array declaration itself requires it (Vulkan VUID 04680), independently
            // of nonuniform indexing. SPIR-V 1.5 makes the capability core.
            let mut capability_end = 5;
            let mut runtime_descriptors = false;
            while capability_end < words.len() && words[capability_end] & 0xffff == 17 {
                ensure!(words[capability_end] >> 16 == 2, "invalid OpCapability");
                runtime_descriptors |= words[capability_end + 1] == 5302;
                capability_end += 2;
            }
            if !runtime_descriptors {
                words.splice(capability_end..capability_end, [(2 << 16) | 17, 5302]);
            }
            let mut at = 5;
            while at < words.len() {
                let count = (words[at] >> 16) as usize;
                ensure!(
                    count > 0 && at + count <= words.len(),
                    "invalid SPIR-V instruction"
                );
                // OpDecorate target Binding(33) 7 -> 5; no other decorations are touched.
                if words[at] & 0xffff == 71
                    && count == 4
                    && words[at + 2] == 33
                    && words[at + 3] == 7
                {
                    words[at + 3] = 5;
                }
                at += count;
            }
            repair_image_arguments(&mut words)?;
            Ok(SpirvStage {
                stage,
                entry: "main".into(),
                words,
            })
        })
        .collect()
}
fn adapt(pass: Pass) -> String {
    let body = pass
        .source()
        .lines()
        .filter(|line| !line.trim_start().starts_with("@group("))
        .collect::<Vec<_>>()
        .join("\n");
    let mut names = BTreeMap::from([
        ("input_colour", "ara_textures[ara_root.w0]"),
        ("colour", "ara_textures[ara_root.w0]"),
        ("second_colour", "ara_textures[ara_root.w1]"),
        ("bloom", "ara_textures[ara_root.w1]"),
        ("auxiliary", "ara_textures[ara_root.w2]"),
        ("velocity_tiles", "ara_textures[ara_root.w3]"),
        ("output_colour", "ara_outputs[ara_root.w4]"),
        ("settings", "ara_settings[ara_root.w5]"),
        ("linear_sampler", "ara_samplers[ara_root.w6]"),
        ("lut", "ara_luts[ara_root.w7]"),
    ]);
    // Only replace globals actually declared by this module: `colour` is a local
    // in the filter module but a resource in the tone module.
    if matches!(pass, Pass::Tone) {
        names.retain(|name, _| {
            matches!(
                *name,
                "colour" | "bloom" | "lut" | "settings" | "linear_sampler"
            )
        });
    } else {
        names.retain(|name, _| !matches!(*name, "colour" | "bloom" | "lut"));
    }
    let mut out = String::new();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if let Some(replacement) = names.get(word.as_str()) {
            out.push_str(replacement);
        } else {
            out.push_str(word);
        }
        word.clear();
    };
    for c in body.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    format!(
        r#"
enable wgpu_binding_array;
struct AraRoot {{w0:u32,w1:u32,w2:u32,w3:u32,w4:u32,w5:u32,w6:u32,w7:u32,}};
var<immediate> ara_root:AraRoot;
@group(0) @binding(5) var ara_textures:binding_array<texture_2d<f32>>;
@group(0) @binding(3) var ara_outputs:binding_array<texture_storage_2d<rgba16float,write>>;
@group(0) @binding(2) var ara_samplers:binding_array<sampler>;
@group(0) @binding(1) var<storage,read> ara_settings:binding_array<Settings>;
@group(0) @binding(7) var ara_luts:binding_array<texture_3d<f32>>;
{out}
"#
    )
}

/// Naga 30 passes a loaded image value for binding-array elements although its
/// resource-parameter ABI expects the UniformConstant pointer. Repair only this
/// exact, type-checked mismatch; unexpected parameter mismatches remain errors.
fn repair_image_arguments(words: &mut [u32]) -> Result<()> {
    let mut result_types = BTreeMap::new();
    let mut pointers = BTreeMap::new();
    let mut loads = BTreeMap::new();
    let mut signatures = BTreeMap::new();
    let mut functions = BTreeMap::new();
    let mut at = 5;
    while at < words.len() {
        let count = (words[at] >> 16) as usize;
        ensure!(
            count > 0 && at + count <= words.len(),
            "invalid SPIR-V instruction"
        );
        match words[at] & 0xffff {
            32 => {
                pointers.insert(words[at + 1], (words[at + 2], words[at + 3]));
            }
            33 => {
                signatures.insert(words[at + 1], words[at + 3..at + count].to_vec());
            }
            54 => {
                functions.insert(words[at + 2], words[at + 4]);
            }
            55 | 59 | 65 | 66 => {
                result_types.insert(words[at + 2], words[at + 1]);
            }
            61 => {
                result_types.insert(words[at + 2], words[at + 1]);
                loads.insert(words[at + 2], (words[at + 1], words[at + 3]));
            }
            _ => {}
        }
        at += count;
    }
    at = 5;
    while at < words.len() {
        let count = (words[at] >> 16) as usize;
        if words[at] & 0xffff == 57 {
            let signature = functions
                .get(&words[at + 3])
                .and_then(|id| signatures.get(id))
                .context("function signature missing")?;
            ensure!(
                signature.len() == count - 4,
                "function argument count mismatch"
            );
            for (argument, expected) in words[at + 4..at + count].iter_mut().zip(signature) {
                if let Some(&(loaded_type, pointer)) = loads.get(argument)
                    && let Some(&(storage, pointee)) = pointers.get(expected)
                    && storage == 0
                    && pointee == loaded_type
                {
                    ensure!(
                        result_types.get(&pointer) == Some(expected),
                        "image pointer type mismatch"
                    );
                    *argument = pointer;
                }
            }
        }
        at += count;
    }
    Ok(())
}
