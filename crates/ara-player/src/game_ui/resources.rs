//! Project resources use the same packaged bytes in native and reference views.
use super::*;
use ara_diag::Diagnostic;

pub(super) fn warning(diagnostics: &mut Vec<Diagnostic>, message: String) {
    let diagnostic = Diagnostic::warning(ara_diag::Code(4051), message);
    eprintln!("ara-player: {diagnostic}");
    diagnostics.push(diagnostic);
}

pub(super) fn load(source: &GameUi) -> Result<(TextSystem, Renderer, Vec<Diagnostic>)> {
    let mut diagnostics = Vec::new();
    for message in &source.resource_diagnostics {
        warning(&mut diagnostics, message.to_owned());
    }
    let mut text = TextSystem::bundled().map_err(ui_error)?;
    for (index, font) in source.fonts.iter().enumerate() {
        if text
            .fonts
            .collection
            .register_fonts(font.clone().into(), None)
            .is_empty()
        {
            let key = format!("font:{index}");
            let path = source
                .resource_paths
                .get(&key)
                .map_or(key.as_str(), String::as_str);
            warning(
                &mut diagnostics,
                format!("UI font {path}: no valid faces; using bundled fonts"),
            );
        }
    }
    let mut renderer = Renderer::default();
    for (name, bytes) in &source.images {
        let decoded = image::load_from_memory(bytes).map(|image| image.into_rgba8());
        let image = match decoded {
            Ok(image) => image,
            Err(error) => {
                warning(
                    &mut diagnostics,
                    format!(
                        "UI image {}: {error}; drawing skipped",
                        image_path(source, name)
                    ),
                );
                continue;
            }
        };
        renderer
            .image(
                stable_id(name) as u32 & 0x7fff_ffff,
                ara_ui_render::gpu::Image {
                    width: image.width(),
                    height: image.height(),
                    rgba: image.into_raw(),
                },
            )
            .map_err(ui_error)?;
    }
    Ok((text, renderer, diagnostics))
}

fn image_path<'a>(source: &'a GameUi, name: &'a str) -> &'a str {
    source
        .resource_paths
        .get(&format!("image:{name}"))
        .map_or(name, String::as_str)
}

pub(super) fn missing(source: &GameUi, id: u32) -> String {
    let path = source.resource_paths.iter().find_map(|(key, path)| {
        let name = key.strip_prefix("image:")?;
        (stable_id(name) as u32 & 0x7fff_ffff == id).then_some(path.as_str())
    });
    match path {
        Some(path) => format!("UI image {path} (resource {id}) is unavailable; drawing skipped"),
        None => format!("UI image resource {id} is unavailable; drawing skipped"),
    }
}
