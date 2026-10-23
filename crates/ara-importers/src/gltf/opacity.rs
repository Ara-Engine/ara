//! Explicit companion opacity images become base-color alpha during the cook.
use ara_asset::{ImportInput, Result};
use image::ImageEncoder;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn dependencies(maps: &BTreeMap<String, String>) -> Result<Vec<String>> {
    maps.values()
        .filter(|uri| !uri.starts_with("data:"))
        .map(|uri| super::uri_path(uri))
        .collect::<Result<BTreeSet<_>>>()
        .map(|paths| paths.into_iter().collect())
}

pub(super) fn resolve(
    gltf: &gltf::Gltf,
    input: &ImportInput,
    maps: &BTreeMap<String, String>,
) -> Result<BTreeMap<usize, Vec<u8>>> {
    let colors: BTreeSet<_> = gltf
        .materials()
        .filter_map(|m| {
            m.pbr_metallic_roughness()
                .base_color_texture()
                .map(|t| t.texture().source().index())
        })
        .collect();
    let mut resolved = BTreeMap::new();
    for (key, uri) in maps {
        let image = gltf
            .images()
            .find(|image| {
                key == &format!("#{}", image.index())
                    || match image.source() {
                        gltf::image::Source::Uri { uri, .. } => {
                            uri == key || super::uri_path(uri).is_ok_and(|path| &path == key)
                        }
                        _ => false,
                    }
            })
            .ok_or_else(|| super::invalid(format!("opacityMaps image `{key}` does not exist")))?;
        if !colors.contains(&image.index()) {
            return Err(super::invalid(format!(
                "opacityMaps image `{key}` is not used as base color"
            )));
        }
        if resolved
            .insert(image.index(), super::resource(uri, input)?)
            .is_some()
        {
            return Err(super::invalid(format!(
                "opacityMaps names image `{key}` more than once"
            )));
        }
    }
    Ok(resolved)
}

pub(super) fn merge(color: &[u8], opacity: &[u8]) -> Result<Vec<u8>> {
    let mut color = image::load_from_memory(color)
        .map_err(super::invalid)?
        .to_rgba8();
    let opacity = image::load_from_memory(opacity)
        .map_err(super::invalid)?
        .to_rgba8();
    let opacity = if opacity.dimensions() == color.dimensions() {
        opacity
    } else {
        image::imageops::resize(
            &opacity,
            color.width(),
            color.height(),
            image::imageops::FilterType::Triangle,
        )
    };
    for (color, opacity) in color.pixels_mut().zip(opacity.pixels()) {
        color[3] = ((u16::from(color[3]) * u16::from(opacity[0]) + 127) / 255) as u8;
    }
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(
            &color,
            color.width(),
            color.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(super::invalid)?;
    Ok(bytes)
}

pub(super) fn alpha_modes(
    document: &mut Value,
    gltf: &gltf::Gltf,
    maps: &BTreeMap<usize, Vec<u8>>,
) {
    for material in gltf.materials() {
        if material
            .pbr_metallic_roughness()
            .base_color_texture()
            .is_some_and(|t| maps.contains_key(&t.texture().source().index()))
            && let Some(index) = material.index()
        {
            let material = &mut document["materials"][index];
            if material["alphaMode"].is_null() || material["alphaMode"] == "OPAQUE" {
                material["alphaMode"] = "MASK".into();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(pixels: &[u8], width: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(pixels, width, 1, image::ExtendedColorType::Rgba8)
            .expect("PNG");
        bytes
    }

    #[test]
    fn companion_red_multiplies_existing_alpha_and_preserves_rgb() {
        let color = png(&[10, 20, 30, 128, 40, 50, 60, 255], 2);
        let opacity = png(&[128, 0, 255, 255, 0, 255, 255, 255], 2);
        let bytes = merge(&color, &opacity).expect("merge");
        let result = image::load_from_memory(&bytes).expect("PNG").to_rgba8();
        assert_eq!(result.as_raw(), &[10, 20, 30, 64, 40, 50, 60, 0]);
        assert_eq!(merge(&color, &opacity).expect("repeat"), bytes);
        let resized = merge(&color, &png(&[255; 4], 1)).expect("resize mask");
        assert_eq!(
            image::load_from_memory(&resized)
                .expect("PNG")
                .to_rgba8()
                .as_raw(),
            &[10, 20, 30, 128, 40, 50, 60, 255]
        );
    }
}
