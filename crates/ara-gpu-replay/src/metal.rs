//! Naga MSL plus a checked entry shim; shader algorithms remain generated code.
use crate::{MetalProgram, Pass, parse, validate};
use anyhow::{Context, Result, bail, ensure};
use naga::back::msl;

/// Translate original WGSL and adapt only its entry interface to Ara's global heap.
pub fn msl(pass: Pass) -> Result<MetalProgram> {
    let mut module = parse(pass.source())?;
    for (_, global) in module.global_variables.iter_mut() {
        if global.space == naga::AddressSpace::Uniform {
            global.space = naga::AddressSpace::Storage {
                access: naga::StorageAccess::LOAD,
            };
        }
    }
    let info = validate(&module)?;
    let mut options = msl::Options {
        lang_version: (3, 0),
        fake_missing_bindings: false,
        ..Default::default()
    };
    let mut bindings = msl::BindingMap::default();
    for (_, global) in module.global_variables.iter() {
        if let Some(binding) = global.binding {
            let mut target = msl::BindTarget::default();
            match module.types[global.ty].inner {
                naga::TypeInner::Image { .. } => target.texture = Some(binding.binding as u8),
                naga::TypeInner::Sampler { .. } => {
                    target.sampler = Some(msl::BindSamplerTarget::Resource(0))
                }
                _ => target.buffer = Some(0),
            }
            bindings.insert(binding, target);
        }
    }
    for entry in &module.entry_points {
        options.per_entry_point_map.insert(
            entry.name.clone(),
            msl::EntryPointResources {
                resources: bindings.clone(),
                ..Default::default()
            },
        );
    }
    let (mut source, translation) =
        msl::write_string(&module, &info, &options, &msl::PipelineOptions::default())?;
    let names = translation
        .entry_point_names
        .into_iter()
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let at = module
        .entry_points
        .iter()
        .position(|e| e.name == pass.entry())
        .context("selected entry not emitted")?;
    let name = &names[at];
    let needle = format!("{name}(");
    let name_at = source.find(&needle).context("entry signature missing")?;
    let start = source[..name_at].rfind('\n').map_or(0, |i| i + 1);
    let open = name_at + name.len();
    let close = matching_paren(&source, open)?;
    let prefix = source[start..name_at].to_string();
    let stage = if matches!(pass, Pass::Tone) {
        "fragment"
    } else {
        "kernel"
    };
    let stage_at = prefix
        .find(&format!("{stage} "))
        .context("entry stage qualifier missing")?;
    let result_type = prefix[stage_at + stage.len() + 1..].trim().to_string();
    ensure!(!result_type.is_empty(), "entry return type missing");
    let args = split_args(&source[open + 1..close]);
    let mut plain = Vec::new();
    let mut calls = Vec::new();
    for arg in args {
        let (parameter, attribute) = arg
            .split_once("[[")
            .context("entry argument has no binding")?;
        let parameter = parameter.trim();
        let identifier = parameter
            .split_whitespace()
            .last()
            .context("entry argument missing name")?
            .trim_start_matches('&');
        let call = if attribute.contains("thread_position_in_grid") {
            "ara_id".to_owned()
        } else if attribute.contains("position") {
            "ara_position".to_owned()
        } else {
            resource(identifier)?
        };
        plain.push(parameter.to_string());
        calls.push(call);
    }
    let ordinary = format!("{result_type} {name}({})", plain.join(", "));
    source.replace_range(start..close + 1, &ordinary);
    let entry = format!("ara_{}", pass.entry());
    let builtin = if matches!(pass, Pass::Tone) {
        "float4 ara_position [[position]]"
    } else {
        "uint3 ara_id [[thread_position_in_grid]]"
    };
    source.push_str("\nstruct AraWriteImageEntry {ulong unused; texture2d<float, access::write> texture; ulong metadata;};\nstruct AraVolumeEntry {ulong unused; texture3d<float> texture; ulong metadata;};\n");
    let return_call = if result_type == "void" {
        format!("{name}({});", calls.join(", "))
    } else {
        format!("return {name}({});", calls.join(", "))
    };
    source.push_str(&format!("{stage} {result_type} {entry}(constant TopLevel& top [[buffer(2)]], {builtin}) {{ {return_call} }}\n"));
    let vertex_entry = module
        .entry_points
        .iter()
        .position(|e| e.stage == naga::ShaderStage::Vertex)
        .map(|i| names[i].clone());
    Ok(MetalProgram {
        source,
        entry,
        vertex_entry,
    })
}
fn resource(name: &str) -> Result<String> {
    Ok(match name {
        "input_colour" | "colour" => "top.textures[top.root[0]].texture".into(),
        "second_colour" | "bloom" => "top.textures[top.root[1]].texture".into(),
        "auxiliary" => "top.textures[top.root[2]].texture".into(),
        "velocity_tiles" => "top.textures[top.root[3]].texture".into(),
        "output_colour" => {
            "((device AraWriteImageEntry*)top.storage_images)[top.root[4]].texture".into()
        }
        "settings" => "*reinterpret_cast<device Settings*>(top.buffers[top.root[5]].data)".into(),
        "linear_sampler" => "top.samplers[top.root[6]].state".into(),
        "lut" => "((device AraVolumeEntry*)top.textures)[top.root[7]].texture".into(),
        _ => bail!("unmapped naga entry parameter {name}"),
    })
}
fn matching_paren(source: &str, open: usize) -> Result<usize> {
    let mut depth = 0;
    for (offset, c) in source[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(open + offset);
                }
            }
            _ => {}
        }
    }
    bail!("unterminated entry signature")
}
fn split_args(args: &str) -> Vec<&str> {
    let mut output = Vec::new();
    let mut start = 0;
    let mut depth = 0;
    for (i, c) in args.char_indices() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                output.push(args[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    if !args[start..].trim().is_empty() {
        output.push(args[start..].trim());
    }
    output
}
