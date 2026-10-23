use std::collections::BTreeMap;

use ara_diag::Diagnostic;
use serde::{Deserialize, Serialize};

use crate::{Material, Type, check, check::valid_values, codes};

/// Cheap per-object overrides; static choices are explicitly declared by definitions.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Instance {
    /// Parameter node ID to finite lanes, with absent values using defaults.
    pub overrides: BTreeMap<String, Vec<f64>>,
}

/// Content-addressed shader specialization identity, independent of runtime values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantKey {
    /// BLAKE3 of canonical semantic AST and selected static values.
    pub hash: String,
}

/// One uniform parameter's row-major float4 storage contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParameterSlot {
    /// Stable parameter node identity.
    pub node: String,
    /// Semantic type; texture handles use an exact integer in the x float lane.
    pub ty: Type,
    /// Offset relative to DrawConstants.materialIndex in the material float4 buffer.
    pub offset: u32,
    /// Number of float4 rows (matrix rows remain separate, with padding).
    pub rows: u32,
}

/// Runtime instance upload and its definition-owned layout.
#[derive(Debug, Clone, PartialEq)]
pub struct PackedParameters {
    /// Deterministic parameter slot metadata.
    pub slots: Vec<ParameterSlot>,
    /// Padded float4 rows ready for the bindless material buffer.
    pub rows: Vec<[f32; 4]>,
}

impl Instance {
    /// Validates unknown IDs, shapes, resource indices and static enum membership.
    pub fn validate(&self, m: &Material) -> Result<(), Vec<Diagnostic>> {
        let mut diags = Vec::new();
        for (id, values) in &self.overrides {
            match m.parameters.get(id) {
                Some(p)
                    if valid_values(p.ty, values)
                        && (p.variants.is_empty() || p.variants.contains(values)) => {}
                _ => diags.push(
                    Diagnostic::error(
                        codes::OVERRIDE,
                        format!("invalid parameter override `{id}`"),
                    )
                    .record(id),
                ),
            }
        }
        if diags.is_empty() { Ok(()) } else { Err(diags) }
    }

    /// Builds a shader key; ordinary artistic variation does not create variants.
    pub fn variant_key(&self, m: &Material) -> Result<VariantKey, Vec<Diagnostic>> {
        check(m)?;
        self.validate(m)?;
        let selected: BTreeMap<_, _> = m
            .parameters
            .iter()
            .filter(|(_, p)| !p.variants.is_empty())
            .map(|(id, p)| (id, self.overrides.get(id).unwrap_or(&p.default)))
            .collect();
        let value = serde_json::to_value((m, selected))
            .map_err(|e| vec![Diagnostic::error(codes::OVERRIDE, e.to_string())])?;
        let bytes = ara_json::to_canonical_string(&ara_json::sort_keys(&value));
        Ok(VariantKey {
            hash: blake3::hash(bytes.as_bytes()).to_hex().to_string(),
        })
    }

    /// Packs runtime defaults/overrides for the S1 bindless material buffer ABI.
    /// Mat3 uses 3 padded rows; static parameters are compiled into source instead.
    pub fn pack(&self, m: &Material) -> Result<PackedParameters, Vec<Diagnostic>> {
        check(m)?;
        self.validate(m)?;
        let mut slots = Vec::new();
        let mut data = Vec::new();
        for (id, p) in m.parameters.iter().filter(|(_, p)| p.variants.is_empty()) {
            let values = self.overrides.get(id).unwrap_or(&p.default);
            let stride = if p.ty == Type::Mat3 { 3 } else { 4 };
            let offset = data.len() as u32;
            for chunk in values.chunks(stride) {
                let mut row = [0.0; 4];
                for (out, &v) in row.iter_mut().zip(chunk) {
                    *out = v as f32;
                }
                data.push(row);
            }
            slots.push(ParameterSlot {
                node: id.clone(),
                ty: p.ty,
                offset,
                rows: data.len() as u32 - offset,
            });
        }
        Ok(PackedParameters { slots, rows: data })
    }
}
