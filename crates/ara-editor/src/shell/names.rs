//! Asset names as people read them: file names without their extension (the type
//! shows as an icon and badge; the full file name is in the tooltip and Inspector),
//! fitted to tile captions on at most two lines, breaking at `_`, `-`, `.`, spaces or
//! camelCase humps, with an ellipsis when even two lines are not enough.
use super::assets::Asset;
use ara_ui_text::{Text, TextSystem};
use std::collections::HashMap;

/// `apron.gltf` → `apron`; names without an extension (and dot-files) stay whole.
pub(crate) fn stem(name: &str) -> &str {
    match name.rfind('.') {
        Some(0) | None => name,
        Some(dot) => &name[..dot],
    }
}

/// The asset's file name (last path segment).
pub(crate) fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// What kind of asset this is, in words ("Model", "Texture", …).
pub(crate) fn kind_label(asset: &Asset) -> &'static str {
    match asset.kind.as_str() {
        "folder" => "Folder",
        "glb" | "gltf" | "fbx" | "obj" => "Model",
        "png" | "jpg" | "jpeg" | "hdr" | "exr" | "ktx2" | "tga" | "bmp" => "Texture",
        "graph" | "material" => match asset.domain.as_deref() {
            Some("vfx") => "Visual effect",
            _ => "Material",
        },
        "materialinstance" => "Material instance",
        "scene" => "Scene",
        "prefab" => "Prefab",
        "animation" => "Animation",
        "terrain" => "Terrain",
        "foliage" => "Foliage",
        "world" => "World",
        "music" => "Music",
        "wav" | "ogg" | "mp3" | "flac" => "Audio",
        "cs" => "C# script",
        "json" => "JSON",
        _ => "File",
    }
}

/// The asset's type color (a chart token): Assets badges, list icons.
pub(crate) fn kind_color(asset: &Asset) -> &'static str {
    match asset.kind.as_str() {
        "glb" | "gltf" | "fbx" | "obj" => "chart-1",
        "png" | "jpg" | "jpeg" | "hdr" | "exr" | "ktx2" | "tga" | "bmp" => "chart-2",
        "graph" | "material" | "materialinstance" => match asset.domain.as_deref() {
            Some("vfx") => "chart-5",
            _ => "chart-3",
        },
        "scene" | "prefab" => "chart-4",
        "animation" => "chart-6",
        _ => "muted-foreground",
    }
}

/// The Assets list's rows: the name (no extension) and its type or, while searching,
/// its folder. Keys are the paths' stable IDs.
#[derive(Default)]
pub(crate) struct AssetRows {
    pub items: Vec<(u64, String, String)>,
}
impl ara_ui::virtualize::Rows for AssetRows {
    fn len(&self) -> usize {
        self.items.len()
    }
    fn row(&self, index: usize) -> ara_ui::virtualize::Row<'_> {
        let (key, name, detail) = &self.items[index];
        ara_ui::virtualize::Row {
            key: *key,
            depth: 0,
            cells: vec![name, detail],
        }
    }
}
/// The detail column's width in a list `width` wide.
pub(crate) fn detail_width(width: f32) -> f32 {
    (width * 0.32).clamp(72., 220.)
}

/// Caption lines for names, cached per name, width and size.
#[derive(Default)]
pub(crate) struct NameFit {
    cache: HashMap<(String, u32, u32, usize), Vec<String>>,
}
impl NameFit {
    /// `name` on at most `max` lines of `width` logical pixels at `size` px.
    pub fn lines(
        &mut self,
        text: &mut TextSystem,
        name: &str,
        width: f32,
        size: f32,
        max: usize,
    ) -> &[String] {
        // Bounded: browsing many folders never grows it without limit.
        if self.cache.len() > 4096 {
            self.cache.clear();
        }
        self.cache
            .entry((name.to_owned(), width.to_bits(), size.to_bits(), max))
            .or_insert_with(|| fit(text, name, width, size, max))
    }
}

/// Greedy line fitting over one shaping pass: each line takes as many clusters as fit,
/// backing off to the last good break when that keeps at least 40% of the line; the
/// last line ends in "…" when the name does not fit.
pub(crate) fn fit(
    text: &mut TextSystem,
    name: &str,
    width: f32,
    size: f32,
    max: usize,
) -> Vec<String> {
    let mut shaped = Text::new(name.to_owned());
    text.measure(&mut shaped, None, size);
    // Byte start and advance of every cluster, in text order.
    let mut clusters: Vec<(usize, f32)> = shaped
        .layout
        .lines()
        .flat_map(|line| line.runs().collect::<Vec<_>>())
        .flat_map(|run| {
            run.clusters()
                .map(|c| (c.text_range().start, c.advance()))
                .collect::<Vec<_>>()
        })
        .collect();
    clusters.sort_by_key(|c| c.0);
    let at = |i: usize| clusters.get(i).map_or(name.len(), |c| c.0);
    let span = |a: usize, b: usize| clusters[a..b].iter().map(|c| c.1).sum::<f32>();
    let mut ellipsis = Text::new("…".to_owned());
    let ellipsis = text.measure(&mut ellipsis, None, size).width;
    let mut lines = Vec::new();
    let mut start = 0;
    while start < clusters.len() && lines.len() < max {
        let mut end = start;
        let mut used = 0.;
        while end < clusters.len() && used + clusters[end].1 <= width {
            used += clusters[end].1;
            end += 1;
        }
        if end == clusters.len() {
            lines.push(name[at(start)..].to_owned());
            break;
        }
        if lines.len() + 1 == max {
            while end > start && span(start, end) + ellipsis > width {
                end -= 1;
            }
            let kept = name[at(start)..at(end)].trim_end_matches(['_', '-', '.', ' ']);
            lines.push(format!("{kept}…"));
            break;
        }
        // At least one cluster per line, however narrow.
        let end = end.max(start + 1);
        let cut = (start + 1..end)
            .rev()
            .find(|&i| breaks_before(name, at(i)))
            .filter(|&i| span(start, i) >= width * 0.4)
            .unwrap_or(end);
        lines.push(name[at(start)..at(cut)].to_owned());
        start = cut;
    }
    lines
}

/// A line may start at byte `i`: after `_`, `-`, `.` or a space, or at a camelCase hump.
fn breaks_before(name: &str, i: usize) -> bool {
    let before = name[..i].chars().next_back();
    let here = name[i..].chars().next();
    matches!(before, Some('_' | '-' | '.' | ' '))
        || matches!((before, here), (Some(b), Some(h)) if b.is_lowercase() && h.is_uppercase())
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

    #[test]
    fn long_names_break_at_separators_and_end_in_an_ellipsis() {
        let mut text = crate::shell::text_system().expect("fonts");
        let width = |text: &mut TextSystem, s: &str| {
            let mut t = Text::new(s.to_owned());
            text.measure(&mut t, None, 11.).width
        };
        assert_eq!(fit(&mut text, "apron", 84., 11., 2), ["apron"]);
        let two = fit(&mut text, "dry_ground_01_diff_2k", 84., 11., 2);
        assert_eq!(two.len(), 2);
        assert!(two[0].ends_with('_'), "breaks after an underscore: {two:?}");
        assert_eq!(two.concat(), "dry_ground_01_diff_2k");
        let long = fit(
            &mut text,
            "rusty_corrugated_iron_sheet_nor_gl_4k_variant",
            84.,
            11.,
            2,
        );
        assert_eq!(long.len(), 2);
        assert!(long[1].ends_with('…'), "{long:?}");
        for line in &long {
            assert!(width(&mut text, line) <= 84.5, "{line} fits");
        }
        let camel = fit(&mut text, "ContainerBlueVariantLarge", 84., 11., 2);
        assert!(camel[0].ends_with(|c: char| c.is_lowercase()), "{camel:?}");
    }
}
