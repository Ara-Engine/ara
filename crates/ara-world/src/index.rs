use crate::{CellKey, Manifest, Result, invalid};
use ara_doc::{AssetId, RecordId, Workspace};
use serde::Serialize;
use std::collections::BTreeMap;

/// Text-derived entity location, available even while the runtime cell is unloaded.
#[derive(Debug, Clone, Serialize)]
pub struct IndexedEntity {
    /// Project-wide stable record identity.
    pub id: RecordId,
    /// Owning scene document identity.
    pub document: AssetId,
    /// Human-readable `@path#entityId` address.
    pub reference: String,
    /// Manifest cell record identity.
    pub cell: String,
    /// Streaming key.
    pub key: CellKey,
    /// Data layer record identity.
    pub layer: String,
    /// World-space point used by spatial queries.
    pub position: [f32; 3],
}

/// Immutable spatial index built on a worker from all canonical cell/entity files.
/// Publish a replacement after committed authoring transactions; runtime residency
/// never removes entries. Buckets are keyed by actual entity positions.
#[derive(Debug)]
pub struct TextIndex {
    entities: BTreeMap<String, IndexedEntity>,
    buckets: BTreeMap<(i32, i32), Vec<String>>,
    cell_size: f32,
}

impl TextIndex {
    /// Scans every manifest document and rejects duplicate stable entity identities.
    /// Prefab inheritance is resolved for authored actor positions; prefab-internal
    /// rows are cooked for runtime but addressed for authoring through their instance.
    pub fn build(
        manifest: &Manifest,
        workspace: &Workspace,
        schema: &ara_schema::SchemaDb,
    ) -> Result<Self> {
        let mut index = Self {
            cell_size: manifest.settings.cell_size,
            entities: BTreeMap::new(),
            buckets: BTreeMap::new(),
        };
        for (cell_id, cell) in &manifest.cells {
            for (path, doc) in manifest.documents(workspace, cell_id)? {
                if !doc.conflicts.is_empty() {
                    return Err(invalid("unresolved cell conflicts"));
                }
                let mut positions = BTreeMap::new();
                let flat = crate::cook::flatten_document(doc, workspace, schema)?;
                for id in doc.records().keys() {
                    let position =
                        world_position(id.as_str(), &flat, &mut positions, &mut Vec::new())?;
                    let bucket = (
                        grid(position[0], index.cell_size)?,
                        grid(position[2], index.cell_size)?,
                    );
                    index
                        .buckets
                        .entry(bucket)
                        .or_default()
                        .push(id.to_string());
                    let row = IndexedEntity {
                        id: id.clone(),
                        document: doc.id.clone(),
                        reference: format!("@{path}#{id}"),
                        cell: cell_id.clone(),
                        key: cell.key(),
                        layer: cell.layer.clone(),
                        position,
                    };
                    if index.entities.insert(id.to_string(), row).is_some() {
                        return Err(invalid(format!("duplicate world entity {id}")));
                    }
                }
            }
        }
        Ok(index)
    }

    /// Stable lookup independent of current cell residency or filenames.
    pub fn entity(&self, id: &str) -> Option<&IndexedEntity> {
        self.entities.get(id)
    }

    /// Number of indexed entities, including unloaded cells.
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    /// Whether there are any indexed entities.
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// Point-in-box query, inclusive, returned in stable ID order.
    pub fn query(
        &self,
        min: [f32; 3],
        max: [f32; 3],
        layer: Option<&str>,
        limit: usize,
    ) -> Result<Vec<&IndexedEntity>> {
        if min.iter().chain(&max).any(|v| !v.is_finite()) || (0..3).any(|i| min[i] > max[i]) {
            return Err(invalid("invalid spatial bounds"));
        }
        let lo = (grid(min[0], self.cell_size)?, grid(min[2], self.cell_size)?);
        let hi = (grid(max[0], self.cell_size)?, grid(max[2], self.cell_size)?);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut rows = BTreeMap::new();
        // Iterate occupied buckets, never the potentially enormous empty coordinate range.
        for (&(x, z), ids) in self.buckets.range((lo.0, i32::MIN)..=(hi.0, i32::MAX)) {
            if z < lo.1 || z > hi.1 || x < lo.0 || x > hi.0 {
                continue;
            }
            for id in ids {
                let row = &self.entities[id];
                if layer.is_none_or(|l| l == row.layer)
                    && (0..3).all(|i| row.position[i] >= min[i] && row.position[i] <= max[i])
                {
                    rows.insert(row.id.as_str(), row);
                    if rows.len() > limit {
                        rows.pop_last();
                    }
                }
            }
        }
        Ok(rows.into_values().collect())
    }
}

fn grid(value: f32, size: f32) -> Result<i32> {
    let v = (f64::from(value) / f64::from(size)).floor();
    if v < f64::from(i32::MIN) || v > f64::from(i32::MAX) {
        return Err(invalid("position outside grid"));
    }
    Ok(v as i32)
}

fn world_position(
    id: &str,
    doc: &ara_doc::Document,
    cache: &mut BTreeMap<String, [f32; 3]>,
    stack: &mut Vec<String>,
) -> Result<[f32; 3]> {
    // Full affine transforms preserve parent scale and rotation in spatial queries.
    fn matrix(id: &str, doc: &ara_doc::Document, stack: &mut Vec<String>) -> Result<glam::Mat4> {
        if stack.len() >= 128 || stack.iter().any(|p| p == id) {
            return Err(invalid("entity hierarchy cycle"));
        }
        let row = doc
            .record(id)
            .ok_or_else(|| invalid("missing hierarchy parent"))?;
        stack.push(id.to_owned());
        let transform: ara_components::Transform = serde_json::from_value(
            row.get("Transform")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({})),
        )?;
        let rotation = transform.rotation.map(f32::to_radians);
        let local = glam::Mat4::from_scale_rotation_translation(
            transform.scale.into(),
            glam::Quat::from_euler(glam::EulerRot::XYZ, rotation[0], rotation[1], rotation[2]),
            transform.position.into(),
        );
        let world = if let Some(parent) = ara_doc::parent_of(row) {
            matrix(parent, doc, stack)? * local
        } else {
            local
        };
        stack.pop();
        Ok(world)
    }
    if let Some(position) = cache.get(id) {
        return Ok(*position);
    }
    let position = matrix(id, doc, stack)?
        .transform_point3(glam::Vec3::ZERO)
        .to_array();
    if position.iter().any(|v| !v.is_finite()) {
        return Err(invalid("nonfinite world position"));
    }
    cache.insert(id.into(), position);
    Ok(position)
}
