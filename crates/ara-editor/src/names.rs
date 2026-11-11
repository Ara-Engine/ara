//! Asset names as people read them: file names without their extension (the type
//! shows as an icon and badge; the full file name is in the tooltip and Inspector),
//! fitted to tile captions on at most two lines, breaking at `_`, `-`, `.`, spaces or
//! camelCase humps, with an ellipsis when even two lines are not enough.

/// `apron.gltf` → `apron`; names without an extension (and dot-files) stay whole.
pub(crate) fn stem(name: &str) -> &str {
    match name.rfind('.') {
        Some(0) | None => name,
        Some(dot) => &name[..dot],
    }
}

/// `innerAngle` → `Inner angle`; `_` becomes a space.
pub(crate) fn sentence(label: &str) -> String {
    let mut out = String::new();
    for (i, c) in label.chars().enumerate() {
        if i == 0 {
            out.extend(c.to_uppercase());
        } else if c.is_uppercase() {
            out.push(' ');
            out.extend(c.to_lowercase());
        } else if c == '_' {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

/// Image files the texture importer reads (lowercase extensions), plus cooked KTX2.
pub(crate) fn is_texture(extension: &str) -> bool {
    matches!(
        extension,
        "png"
            | "jpg"
            | "jpeg"
            | "hdr"
            | "exr"
            | "tga"
            | "tif"
            | "tiff"
            | "webp"
            | "bmp"
            | "dds"
            | "ktx2"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems_drop_only_the_last_extension() {
        assert_eq!(stem("apron.gltf"), "apron");
        assert_eq!(stem("main.scene"), "main");
        assert_eq!(stem("archive.tar.gz"), "archive.tar");
        assert_eq!(stem(".gitignore"), ".gitignore");
        assert_eq!(stem("README"), "README");
    }
}
