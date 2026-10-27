//! `add`: resolves an asset at its source, downloads every file into the blob cache to
//! hash it, and records the pinned entry (URL, size, BLAKE3, license, authors).
use crate::{
    fetch::Cache,
    lock::{Asset, File, Kind, Lock},
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

/// Where an asset comes from.
pub enum Source {
    /// A Poly Haven model, texture or HDRI (all CC0), by Poly Haven ID.
    PolyHaven {
        /// Poly Haven asset ID (`Barrel_01`).
        id: String,
        /// Resolution (`1k`, `2k`, `4k`).
        resolution: String,
    },
    /// Explicit files with explicit attribution (Khronos samples, Mesh2Motion).
    Urls {
        /// Display title.
        title: String,
        /// Source page.
        source: String,
        /// SPDX license.
        license: String,
        /// Credited authors.
        authors: Vec<String>,
        /// `path=url` pairs.
        files: Vec<(String, String)>,
    },
}

/// Resolves, downloads and hashes `source`, then inserts or replaces `id` in `lock`.
pub fn add(
    lock: &mut Lock,
    cache: &Cache,
    id: &str,
    kind: Kind,
    projects: Vec<String>,
    source: Source,
) -> Result<()> {
    let (title, page, license, authors, files, meters) = match source {
        Source::PolyHaven { id: ph, resolution } => {
            let info: Value = serde_json::from_str(&Cache::get_text(&format!(
                "https://api.polyhaven.com/info/{ph}"
            ))?)?;
            let files: Value = serde_json::from_str(&Cache::get_text(&format!(
                "https://api.polyhaven.com/files/{ph}"
            ))?)?;
            let title = info["name"].as_str().unwrap_or(&ph).to_owned();
            let authors = info["authors"]
                .as_object()
                .map(|a| a.keys().cloned().collect())
                .unwrap_or_default();
            // Poly Haven records texture dimensions in millimetres.
            let meters = info["dimensions"].as_array().and_then(|d| {
                Some([
                    d.first()?.as_f64()? as f32 / 1000.0,
                    d.get(1)?.as_f64()? as f32 / 1000.0,
                ])
            });
            let picked = polyhaven_files(&files, kind, &resolution)
                .with_context(|| format!("Poly Haven {ph} at {resolution}"))?;
            (
                title,
                format!("https://polyhaven.com/a/{ph}"),
                "CC0-1.0".to_owned(),
                authors,
                picked,
                meters.filter(|_| kind == Kind::Texture),
            )
        }
        Source::Urls {
            title,
            source,
            license,
            authors,
            files,
        } => (title, source, license, authors, files, None),
    };
    let mut pinned = Vec::new();
    for (path, url) in files {
        eprintln!("  {id}: {path}");
        let (blake3, size) = cache.download(&url, None)?;
        pinned.push(File {
            path,
            url,
            size,
            blake3,
        });
    }
    lock.assets.retain(|a| a.id != id);
    lock.assets.push(Asset {
        id: id.to_owned(),
        title,
        kind,
        source: page,
        license,
        authors,
        projects,
        meters,
        files: pinned,
    });
    Ok(())
}

/// The `(path, url)` pairs of a Poly Haven asset at one resolution: a model's glTF with
/// its buffers and textures, a texture's colour/normal/ARM maps, or an HDRI.
fn polyhaven_files(files: &Value, kind: Kind, resolution: &str) -> Result<Vec<(String, String)>> {
    let url = |v: &Value| -> Result<String> {
        Ok(v["url"].as_str().context("file without url")?.to_owned())
    };
    let name = |u: &str| u.rsplit('/').next().unwrap_or(u).to_owned();
    Ok(match kind {
        Kind::Model | Kind::Character => {
            let gltf = &files["gltf"][resolution]["gltf"];
            ensure!(gltf.is_object(), "no glTF at this resolution");
            let main = url(gltf)?;
            let mut out = vec![(name(&main), main)];
            for (path, include) in gltf["include"].as_object().context("no includes")? {
                out.push((path.clone(), url(include)?));
            }
            // glTF downloads carry JPG colour maps, so cut-out opacity ships as
            // separate maps; the importer merges them through opacityMaps settings.
            for (map, resolutions) in files.as_object().context("no files")? {
                let lower = map.to_ascii_lowercase();
                if lower.contains("alpha") || lower.contains("opacity") {
                    let file = &resolutions[resolution]["png"];
                    if file.is_object() {
                        let u = url(file)?;
                        out.push((format!("textures/{}", name(&u)), u));
                    }
                }
            }
            out
        }
        Kind::Texture => {
            let mut out = Vec::new();
            for map in ["Diffuse", "nor_gl", "arm"] {
                let file = &files[map][resolution]["jpg"];
                if file.is_object() {
                    let u = url(file)?;
                    out.push((name(&u), u));
                } else if map != "arm" {
                    bail!("texture has no {map} map");
                }
            }
            if !files["arm"].is_object() {
                for map in ["Rough", "AO"] {
                    let file = &files[map][resolution]["jpg"];
                    if file.is_object() {
                        let u = url(file)?;
                        out.push((name(&u), u));
                    }
                }
            }
            out
        }
        Kind::Audio => bail!("Poly Haven has no audio"),
        Kind::Archive | Kind::Font => bail!("Use add-urls for archive and font packs"),
        Kind::Hdri => {
            let file = &files["hdri"][resolution]["hdr"];
            ensure!(file.is_object(), "no HDR at this resolution");
            let u = url(file)?;
            vec![(name(&u), u)]
        }
    })
}
