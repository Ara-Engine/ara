use ara_diag::Diagnostic;
use ara_schema::{AssetRef, DocKindDesc, Schema, SchemaDb};
use serde::{Deserialize, Serialize};

/// Vegetation/prop family used by render extraction and impostor selection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Schema, Serialize, Deserialize)]
#[ara(name = "FoliageSpecies", module = "foliage")]
#[serde(rename_all = "camelCase")]
pub enum Species {
    /// Alpha-tested ground cover with vertex wind.
    #[default]
    Grass,
    /// Trunk/canopy instances with distance impostors.
    Tree,
    /// Static opaque scattered rocks.
    Rock,
}

/// Terrain dependency for a foliage document.
#[derive(Debug, Clone, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "f"), name = "FoliageHeader", module = "foliage")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Header {
    /// Canonical terrain asset reference.
    pub terrain: AssetRef,
}

/// Seeded procedural placement rule, evaluated on workers in global cell space.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "r"), name = "FoliageRule", module = "foliage")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Rule {
    /// Display name for editor/agent tools.
    #[serde(default)]
    pub name: String,
    /// Species/geometry family.
    #[serde(default)]
    pub species: Species,
    /// Source model or foliage asset; resolves to cooked mesh/impostor LODs.
    pub asset: AssetRef,
    /// Explicit deterministic seed, independent of request/iteration order.
    #[serde(default)]
    pub seed: u64,
    /// Global candidate-cell width in metres, strictly positive.
    #[serde(default)]
    pub cell_size: f32,
    /// Expected accepted instances per square metre before surface masks.
    #[serde(default)]
    pub density: f32,
    /// Inclusive elevation range in world metres.
    #[serde(default)]
    pub height: [f32; 2],
    /// Inclusive slope range in degrees.
    #[serde(default)]
    pub slope: [f32; 2],
    /// Splat channel controlling density, zero to three.
    #[serde(default)]
    pub channel: u32,
    /// Minimum terrain weight required before multiplying density by that weight.
    #[serde(default)]
    pub min_weight: f32,
    /// Low-frequency deterministic noise modulation (zero disables it).
    #[serde(default)]
    pub noise: f32,
    /// Inclusive uniform scale range.
    #[serde(default)]
    pub scale: [f32; 2],
    /// Whether this rule participates in regeneration.
    #[serde(default)]
    pub enabled: bool,
}
impl Rule {
    /// Checks finite, bounded candidate counts and valid surface filters.
    pub fn validate(&self) -> Result<(), Diagnostic> {
        if !(0.25..=1024.).contains(&self.cell_size)
            || !(0.0..=16.).contains(&self.density)
            || self
                .height
                .iter()
                .chain(&self.slope)
                .chain(&self.scale)
                .any(|v| !v.is_finite())
            || self.height[0] > self.height[1]
            || self.slope[0] > self.slope[1]
            || self.slope[0] < 0.
            || self.slope[1] > 90.
            || self.channel > 3
            || !(0.0..=1.0).contains(&self.min_weight)
            || !(0.0..=1.0).contains(&self.noise)
            || self.scale[0] <= 0.
            || self.scale[1] < self.scale[0]
            || self.scale[1] > 1000.
            || self.density * self.cell_size * self.cell_size > 1.00001
        {
            return Err(ara_terrain::invalid(
                "invalid foliage rule or density exceeds one candidate per cell",
            ));
        }
        Ok(())
    }
}

/// Circular exclusion zone, applied identically to procedural and manual foliage.
#[derive(Debug, Clone, PartialEq, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "x"), name = "FoliageExclusion", module = "foliage")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Exclusion {
    /// World X/Z centre in metres.
    #[serde(default)]
    pub center: [f32; 2],
    /// Radius in metres.
    #[serde(default)]
    pub radius: f32,
}

/// One manual brush gesture, retained for deterministic regeneration and undo.
#[derive(Debug, Clone, Schema, Serialize, Deserialize)]
#[ara(record(prefix = "p"), name = "FoliagePaint", module = "foliage")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Paint {
    /// Existing rule ID whose asset and filters supply instance properties.
    #[serde(default)]
    pub rule: String,
    /// Explicit seed for per-point scale/yaw/wind phase.
    #[serde(default)]
    pub seed: u64,
    /// Manual world X/Z points, ordered within a single gesture.
    #[serde(default)]
    pub points: Vec<[f32; 2]>,
}

/// Registers canonical foliage document records.
pub fn register(db: &mut SchemaDb) -> Result<(), Diagnostic> {
    db.register::<Header>()
        .register::<Rule>()
        .register::<Exclusion>()
        .register::<Paint>();
    db.add_kind(DocKindDesc {
        name: "foliage".into(),
        version: 1,
        doc: "Seeded scatter rules and manual foliage strokes.".into(),
        id_prefix: "fo".into(),
        extension: "foliage".into(),
        records: vec![
            "FoliageHeader".into(),
            "FoliageRule".into(),
            "FoliageExclusion".into(),
            "FoliagePaint".into(),
        ],
        single_root: false,
    })
}
