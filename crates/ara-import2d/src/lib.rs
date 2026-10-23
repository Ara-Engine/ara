#![warn(missing_docs)]
//! Worker-only PNG atlas and tilemap cache cooks, with immutable snapshot inputs.
use ara_2d::{Atlas, SpriteRegion, Tilemap};
use ara_asset::{
    Artifact, ArtifactKind, Error, ImportContext, ImportInput, ImportOutput, Importer,
    ImporterRegistry, Result,
};
use ara_schema::Schema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Region of one source image to pack. Zero dimensions select its remaining extent.
#[derive(Debug, Clone, Default, Schema, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceRegion {
    /// Stable atlas subasset name.
    pub name: String,
    /// Dependency snapshot path; empty selects the primary source PNG.
    pub source: String,
    /// Source crop and authoring pivot/borders.
    pub region: SpriteRegion,
}
/// Deterministic packing settings carried by an ara-asset import sidecar.
#[derive(Debug, Clone, Schema, Serialize, Deserialize)]
#[ara(params)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct AtlasSettings {
    /// Maximum atlas side; power of two in [16,8192].
    #[ara(default = 2048)]
    pub max_size: u32,
    /// Extruded edge pixels, in [1,16], to prevent filtering bleed.
    #[ara(default = 2)]
    pub padding: u32,
    /// Source regions; empty packs the entire primary PNG as `sprite`.
    pub regions: Vec<SourceRegion>,
}
impl Default for AtlasSettings {
    fn default() -> Self {
        Self {
            max_size: 2048,
            padding: 2,
            regions: Vec::new(),
        }
    }
}
/// Pure PNG atlas plugin; install only in an out-of-process worker.
pub struct AtlasImporter;
impl Importer for AtlasImporter {
    fn id(&self) -> &str {
        "ara.atlas2d"
    }
    fn version(&self) -> &str {
        "1"
    }
    fn register_settings(&self, db: &mut ara_schema::SchemaDb) {
        db.register::<AtlasSettings>();
    }
    fn import(&self, input: &ImportInput, ctx: &mut ImportContext<'_>) -> Result<ImportOutput> {
        let mut settings: AtlasSettings = serde_json::from_value(input.settings.clone())?;
        if !settings.max_size.is_power_of_two()
            || !(16..=8192).contains(&settings.max_size)
            || !(1..=16).contains(&settings.padding)
        {
            return Err(Error::Invalid("invalid atlas size/padding".into()));
        }
        if settings.regions.is_empty() {
            settings.regions.push(SourceRegion {
                name: "sprite".into(),
                ..Default::default()
            });
        }
        if settings.regions.len() > 65536 {
            return Err(Error::Invalid("too many atlas regions".into()));
        }
        let mut decoded = BTreeMap::new();
        let mut names = BTreeSet::new();
        for region in &settings.regions {
            ctx.cancellation.check()?;
            if region.name.is_empty() || !names.insert(&region.name) {
                return Err(Error::Invalid("duplicate/empty sprite name".into()));
            }
            if !decoded.contains_key(&region.source) {
                let bytes = if region.source.is_empty() {
                    &input.source
                } else {
                    input.dependencies.get(&region.source).ok_or_else(|| {
                        Error::Invalid(format!("missing PNG dependency {}", region.source))
                    })?
                };
                let mut reader = image::ImageReader::with_format(
                    std::io::Cursor::new(bytes),
                    image::ImageFormat::Png,
                );
                let mut limits = image::Limits::default();
                limits.max_image_width = Some(8192);
                limits.max_image_height = Some(8192);
                limits.max_alloc = Some(512 * 1024 * 1024);
                reader.limits(limits);
                let image = reader
                    .decode()
                    .map_err(|e| Error::Invalid(e.to_string()))?
                    .to_rgba8();
                decoded.insert(region.source.clone(), image);
            }
        }
        for region in &mut settings.regions {
            let image = &decoded[&region.source];
            let r = &mut region.region;
            if r.x >= image.width() || r.y >= image.height() {
                return Err(Error::Invalid("sprite crop outside PNG".into()));
            }
            if r.width == 0 {
                r.width = image.width() - r.x;
            }
            if r.height == 0 {
                r.height = image.height() - r.y;
            }
            if r.width > image.width() - r.x
                || r.height > image.height() - r.y
                || r.width + settings.padding * 2 > settings.max_size
                || r.height + settings.padding * 2 > settings.max_size
                || !r
                    .pivot
                    .iter()
                    .chain(r.borders.iter())
                    .all(|f| f.is_finite())
                || r.borders.iter().any(|f| *f < 0.)
                || r.borders[0] + r.borders[2] > r.width as f32
                || r.borders[1] + r.borders[3] > r.height as f32
            {
                return Err(Error::Invalid("invalid sprite crop/pivot/borders".into()));
            }
        }
        settings.regions.sort_by(|a, b| {
            b.region
                .height
                .cmp(&a.region.height)
                .then_with(|| a.name.cmp(&b.name))
        });
        let mut placements = Vec::new();
        let mut x = 0;
        let mut y = 0;
        let mut row = 0;
        let mut used_width = 0;
        for source in &settings.regions {
            let w = source.region.width + settings.padding * 2;
            let h = source.region.height + settings.padding * 2;
            if x + w > settings.max_size {
                x = 0;
                y += row;
                row = 0;
            }
            if y + h > settings.max_size {
                return Err(Error::Invalid(
                    "atlas is full; split it or increase maxSize".into(),
                ));
            }
            placements.push((x, y));
            x += w;
            row = row.max(h);
            used_width = used_width.max(x);
        }
        let width = used_width.next_power_of_two();
        let height = (y + row).next_power_of_two();
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        let mut atlas = Atlas {
            width,
            height,
            regions: BTreeMap::new(),
        };
        for (index, (source, &(x, y))) in settings.regions.iter().zip(&placements).enumerate() {
            ctx.report(
                index as f32 / settings.regions.len() as f32,
                "packing/extruding PNG regions",
            )?;
            let r = &source.region;
            let image = &decoded[&source.source];
            let padding = settings.padding;
            for dy in 0..r.height + padding * 2 {
                for dx in 0..r.width + padding * 2 {
                    let sx = r.x + dx.saturating_sub(padding).min(r.width - 1);
                    let sy = r.y + dy.saturating_sub(padding).min(r.height - 1);
                    let at = (((y + dy) * width + x + dx) * 4) as usize;
                    pixels[at..at + 4].copy_from_slice(&image.get_pixel(sx, sy).0);
                }
            }
            atlas.regions.insert(
                source.name.clone(),
                SpriteRegion {
                    x: x + padding,
                    y: y + padding,
                    ..r.clone()
                },
            );
        }
        let mut bytes = b"AR2A\x01\0\0\0".to_vec();
        bytes.extend(width.to_le_bytes());
        bytes.extend(height.to_le_bytes());
        bytes.extend(pixels);
        Ok(ImportOutput {
            artifacts: vec![
                Artifact {
                    name: "atlas.json".into(),
                    kind: ArtifactKind::Scene,
                    bytes: ara_2d::canonical(&atlas)
                        .map_err(|e| Error::Invalid(e.to_string()))?
                        .into_bytes(),
                },
                Artifact {
                    name: "atlas.rgba".into(),
                    kind: ArtifactKind::Custom,
                    bytes,
                },
            ],
        })
    }
}
/// Pure tilemap chunk cook; authoring text is retained as the rebuildable manifest.
pub struct TilemapImporter;
impl Importer for TilemapImporter {
    fn id(&self) -> &str {
        "ara.tilemap2d"
    }
    fn version(&self) -> &str {
        "1"
    }
    fn import(&self, input: &ImportInput, ctx: &mut ImportContext<'_>) -> Result<ImportOutput> {
        let map: Tilemap = serde_json::from_slice(&input.source)?;
        let chunks = ara_2d::tilemap::chunks(&map).map_err(|e| Error::Invalid(e.to_string()))?;
        let mut artifacts = vec![Artifact {
            name: "tilemap.json".into(),
            kind: ArtifactKind::Scene,
            bytes: ara_2d::canonical(&map)
                .map_err(|e| Error::Invalid(e.to_string()))?
                .into_bytes(),
        }];
        for chunk in chunks {
            ctx.cancellation.check()?;
            artifacts.push(Artifact {
                name: format!(
                    "chunks/{}/{},{}.{}",
                    chunk.layer, chunk.coordinate[0], chunk.coordinate[1], chunk.digest
                ),
                kind: ArtifactKind::Custom,
                bytes: chunk.bytes,
            });
        }
        Ok(ImportOutput { artifacts })
    }
}
/// Creates a worker registry; source snapshots must include paths named in settings.
pub fn registry() -> Result<ImporterRegistry> {
    let mut registry = ImporterRegistry::default();
    registry.register(AtlasImporter)?;
    registry.register(TilemapImporter)?;
    Ok(registry)
}
