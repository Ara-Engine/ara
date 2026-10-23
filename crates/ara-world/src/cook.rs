use crate::{CellKey, Manifest, Result, invalid};
use ara_doc::{Record, Workspace};
use ara_schema::SchemaDb;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const MAGIC: &[u8; 8] = b"ARACELL1";
/// Maximum cooked cell size; larger cells must be split by the authoring tools.
pub const MAX_CELL_BYTES: usize = 64 * 1024 * 1024;

/// A flattened authored entity. IDs are world-wide, never cell-relative.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CellEntity {
    /// Stable entity ID.
    pub identity: String,
    /// Component values and authoring metadata.
    pub record: Record,
}

/// Precomputed renderer proxy. Geometry bytes live in the ordinary asset pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HlodProxy {
    /// Cooked merged mesh or impostor reference, `@path#assetId`.
    pub asset: String,
    /// World-space bounding-box minimum.
    pub min: [f32; 3],
    /// World-space bounding-box maximum.
    pub max: [f32; 3],
    /// Geometric error in metres for renderer transition selection.
    pub error: f32,
}

/// Offline seam for a renderer's merged-mesh or impostor cooker.
pub trait HlodCooker {
    /// Builds deterministic proxy descriptors from a flattened cell snapshot.
    fn cook(&self, cell: &CellBlob) -> Result<Vec<HlodProxy>>;
}

/// Versioned, checksummed cell payload; decoding and validation belong on workers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CellBlob {
    /// Stable manifest cell ID.
    pub cell: String,
    /// Grid/layer key.
    pub key: CellKey,
    /// Reflection compatibility hash; includes component and document schemas.
    pub schema: [u8; 32],
    /// Parent-before-child activation order; unloading reverses this order.
    pub entities: Vec<CellEntity>,
    /// Precomputed presentation proxies, integrated by the renderer later.
    pub proxies: Vec<HlodProxy>,
}

impl CellBlob {
    /// Deterministic binary derived data with no text parsing in the shipping path.
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut payload = Vec::new();
        ciborium::ser::into_writer(self, &mut payload).map_err(|e| invalid(e.to_string()))?;
        if payload.len() > MAX_CELL_BYTES - 48 {
            return Err(invalid("cell exceeds 64 MiB; split it"));
        }
        let mut bytes = Vec::with_capacity(payload.len() + 48);
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        bytes.extend_from_slice(blake3::hash(&payload).as_bytes());
        bytes.extend(payload);
        Ok(bytes)
    }

    /// Checks framing/checksum before bounded CBOR decoding; rejects trailing data.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 48 || bytes.len() > MAX_CELL_BYTES || &bytes[..8] != MAGIC {
            return Err(invalid("invalid cooked cell header or size"));
        }
        let size = u64::from_le_bytes(
            bytes[8..16]
                .try_into()
                .map_err(|_| invalid("cell length"))?,
        );
        if size != (bytes.len() - 48) as u64
            || bytes[16..48] != *blake3::hash(&bytes[48..]).as_bytes()
        {
            return Err(invalid("cell length/checksum mismatch"));
        }
        let mut cursor = std::io::Cursor::new(&bytes[48..]);
        let cell: Self =
            ciborium::de::from_reader(&mut cursor).map_err(|e| invalid(e.to_string()))?;
        if cursor.position() != size {
            return Err(invalid("trailing cell payload"));
        }
        cell.validate()?;
        Ok(cell)
    }

    /// One logical pack input per cell; ara-pack never combines boundaries across files.
    pub fn pack_input(&self) -> Result<ara_pack::Input> {
        Ok(ara_pack::Input {
            path: format!("world/cells/{}.aracell", self.cell),
            bytes: self.encode()?,
        })
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if ara_doc::RecordId::parse(&self.cell)
            .map_err(invalid)?
            .prefix()
            != "c"
            || self.entities.len() > 65_536
        {
            return Err(invalid("invalid cell identity or entity count"));
        }
        let mut seen = BTreeSet::new();
        for row in &self.entities {
            if ara_doc::RecordId::parse(&row.identity)
                .map_err(invalid)?
                .prefix()
                != "e"
                || seen.contains(row.identity.as_str())
            {
                return Err(invalid("invalid or duplicate cooked entity"));
            }
            if let Some(parent) = ara_doc::parent_of(&row.record)
                && !seen.contains(parent)
            {
                return Err(invalid(
                    "cell entities must be ordered parents before children",
                ));
            }
            seen.insert(row.identity.as_str());
        }
        for proxy in &self.proxies {
            crate::model::document_ref(&proxy.asset)?;
            if !proxy.error.is_finite()
                || proxy.error < 0.0
                || proxy.min.iter().chain(&proxy.max).any(|v| !v.is_finite())
                || (0..3).any(|i| proxy.min[i] > proxy.max[i])
            {
                return Err(invalid("invalid HLOD bounds/error"));
            }
        }
        Ok(())
    }
}

/// Cooks canonical scene documents to topologically ordered cell rows and proxies.
/// Run off the editor thread; callers supply an immutable source snapshot.
pub fn cook_cell(
    manifest: &Manifest,
    cell: &str,
    workspace: &Workspace,
    schema: &SchemaDb,
    hlod: Option<&dyn HlodCooker>,
) -> Result<CellBlob> {
    let spec = manifest
        .cells
        .get(cell)
        .ok_or_else(|| invalid("unknown cell"))?;
    let mut rows = BTreeMap::new();
    for (_, document) in manifest.documents(workspace, cell)? {
        let flattened = flatten_document(document, workspace, schema)?;
        for (identity, record) in flattened.records() {
            if rows.insert(identity.to_string(), record.clone()).is_some() {
                return Err(invalid("duplicate cell entity identity"));
            }
        }
    }
    let mut entities = Vec::with_capacity(rows.len());
    let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut ready = BTreeSet::new();
    for (id, record) in &rows {
        if let Some(parent) = ara_doc::parent_of(record) {
            if !rows.contains_key(parent) {
                return Err(invalid("cell parent is not present"));
            }
            children
                .entry(parent.into())
                .or_default()
                .push(id.to_owned());
        } else {
            ready.insert(id.to_owned());
        }
    }
    while let Some(identity) = ready.pop_first() {
        let record = rows.remove(&identity).expect("ready entity exists");
        if let Some(next) = children.remove(&identity) {
            ready.extend(next);
        }
        entities.push(CellEntity { identity, record });
    }
    if !rows.is_empty() {
        return Err(invalid("cell hierarchy cycle"));
    }
    let mut blob = CellBlob {
        cell: cell.into(),
        key: spec.key(),
        schema: schema_hash(schema),
        entities,
        proxies: Vec::new(),
    };
    if let Some(hlod) = hlod {
        blob.proxies = hlod.cook(&blob)?;
    }
    blob.validate()?;
    Ok(blob)
}

pub(crate) fn flatten_document(
    document: &ara_doc::Document,
    workspace: &Workspace,
    schema: &SchemaDb,
) -> Result<ara_doc::Document> {
    let diagnostics = workspace.check(&document.id, None, schema);
    if diagnostics.iter().any(|d| d.is_error()) {
        return Err(crate::Error::Transaction(diagnostics));
    }
    let mut diagnostics = Vec::new();
    let resolved = workspace.resolve(&document.id, schema, &mut diagnostics);
    if diagnostics.iter().any(|d| d.is_error()) {
        return Err(crate::Error::Transaction(diagnostics));
    }
    let identities: BTreeMap<_, _> = resolved
        .keys()
        .map(|address| {
            let id = if address.contains('/') {
                format!("e_{}", &blake3::hash(address.as_bytes()).to_hex()[..24])
            } else {
                address.to_owned()
            };
            (address.as_str(), id)
        })
        .collect();
    let mut flat = ara_doc::Document::new("scene", document.id.clone());
    for (address, row) in &resolved {
        let mut record = row.record.clone();
        record.remove("prefab");
        record.remove("overrides");
        record.remove("removed");
        if let Some(parent) = &row.parent {
            record.insert(
                "parent".into(),
                identities
                    .get(parent.as_str())
                    .ok_or_else(|| invalid("missing resolved parent"))?
                    .to_owned()
                    .into(),
            );
        } else {
            record.remove("parent");
        }
        if let Some(order) = &row.order {
            record.insert("order".into(), order.to_owned().into());
        }
        let mut repairs = Vec::new();
        schema.visit(
            &ara_schema::ValueType::Named("Entity".into()),
            &serde_json::Value::Object(record.clone()),
            "",
            &mut |ty, value, path| {
                if *ty == ara_schema::ValueType::Entity
                    && let Some(reference) = value.as_str()
                    && let Some(identity) = identities.get(reference)
                {
                    repairs.push((path.to_owned(), identity.to_owned()));
                }
            },
        );
        for (path, identity) in repairs {
            let path = path.parse::<ara_schema::FieldPath>().map_err(invalid)?;
            let ty = schema
                .get("Entity")
                .ok_or_else(|| invalid("entity schema missing"))?;
            let resolved = schema
                .resolve_path(ty, &path)
                .map_err(|e| invalid(e.message))?;
            ara_schema::path::set(&mut record, &resolved.steps, Some(identity.into()))
                .map_err(invalid)?;
        }
        let identity = identities[address.as_str()].to_owned();
        flat.insert_unchecked(
            ara_doc::RecordId::parse(&identity).map_err(invalid)?,
            ara_doc::canonical_record(schema, schema.get("Entity"), &record),
        );
    }
    Ok(flat)
}

pub(crate) fn schema_hash(schema: &SchemaDb) -> [u8; 32] {
    *blake3::hash(
        ara_json::to_canonical_string(&ara_json::sort_keys(&schema.reflection())).as_bytes(),
    )
    .as_bytes()
}
