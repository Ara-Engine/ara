use crate::{Material, ParameterValue, TextureSource, Type, check, check::valid_value, codes};
use ara_diag::{Diagnostic, FixAction};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Cheap per-object authoring values. Texture overrides remain symbolic assets.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Instance {
    /// Parameter graph ID to typed value; absent values use definition defaults.
    pub overrides: BTreeMap<String, ParameterValue>,
}
/// Shader specialization identity, independent of runtime override values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantKey {
    /// BLAKE3 of canonical definition and selected static boolean values.
    pub hash: String,
}
/// One runtime parameter's row-major float4 storage contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParameterSlot {
    /// Stable graph identity, never shown in material source.
    pub node: String,
    /// Semantic type. Texture heap indices are supplied only during binding.
    pub ty: Type,
    /// Row offset relative to DrawConstants.materialIndex.
    pub offset: u32,
    /// Number of padded float4 rows.
    pub rows: u32,
}
/// A runtime GPU upload, after asset/default textures have been resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct PackedParameters {
    /// Definition-owned layout metadata.
    pub slots: Vec<ParameterSlot>,
    /// Float4 rows; texture indices here are renderer-generated GPU data only.
    pub rows: Vec<[f32; 4]>,
}
/// Derives runtime packing without resolving any GPU resources.
pub fn parameter_layout(m: &Material) -> Vec<ParameterSlot> {
    let mut offset = 0;
    let mut slots = Vec::new();
    for (id, p) in m
        .ordered_parameters()
        .into_iter()
        .filter(|(_, p)| !p.is_static)
    {
        let rows = match p.ty {
            Type::Mat3 => 3,
            Type::Mat4 => 4,
            _ => 1,
        };
        slots.push(ParameterSlot {
            node: id.clone(),
            ty: p.ty,
            offset,
            rows,
        });
        offset += rows;
    }
    slots
}
impl Instance {
    /// Rejects unknown parameters, numeric texture slots and incompatible values.
    pub fn validate(&self, m: &Material) -> Result<(), Vec<Diagnostic>> {
        let diagnostics:Vec<_>=self.overrides.iter().filter(|(id,v)|m.parameters.get(*id).is_none_or(|p|!valid_value(p.ty,v)))
            .map(|(id,_)|Diagnostic::error(codes::OVERRIDE,format!("invalid typed parameter override `{id}`; textures require assets/defaults, never heap indices"))
                .record(id).with_fix("Restore the definition default",FixAction::Command{command:"material.instance.clear".into(),params:serde_json::json!({"node":id})})).collect();
        if diagnostics.is_empty() {
            Ok(())
        } else {
            Err(diagnostics)
        }
    }
    /// Runtime colors/numbers/assets never create additional shader variants.
    pub fn variant_key(&self, m: &Material) -> Result<VariantKey, Vec<Diagnostic>> {
        check(m)?;
        self.validate(m)?;
        let selected: BTreeMap<_, _> = m
            .parameters
            .iter()
            .filter(|(_, p)| p.is_static)
            .map(|(id, p)| (id, self.overrides.get(id).unwrap_or(&p.default)))
            .collect();
        let value = serde_json::to_value((m, selected))
            .map_err(|e| vec![Diagnostic::error(codes::OVERRIDE, e.to_string())])?;
        Ok(VariantKey {
            hash: blake3::hash(
                ara_json::to_canonical_string(&ara_json::sort_keys(&value)).as_bytes(),
            )
            .to_hex()
            .to_string(),
        })
    }
    /// Packs uniforms at bind time. The renderer resolves each asset/default and
    /// semantic texture type to a resident heap index; authored files never do so.
    pub fn pack(
        &self,
        m: &Material,
        mut texture: impl FnMut(&TextureSource, Type) -> Result<u32, Diagnostic>,
    ) -> Result<PackedParameters, Vec<Diagnostic>> {
        check(m)?;
        self.validate(m)?;
        let slots = parameter_layout(m);
        let mut rows = Vec::new();
        for slot in &slots {
            let p = &m.parameters[&slot.node];
            let value = self.overrides.get(&slot.node).unwrap_or(&p.default);
            match value {
                ParameterValue::Numeric(values) => {
                    let stride = if p.ty == Type::Mat3 { 3 } else { 4 };
                    for chunk in values.chunks(stride) {
                        let mut row = [0.0; 4];
                        for (out, &v) in row.iter_mut().zip(chunk) {
                            *out = v as f32;
                        }
                        rows.push(row);
                    }
                }
                ParameterValue::Bool(v) => rows.push([if *v { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0]),
                ParameterValue::Texture(source) => {
                    let index = texture(source, p.ty).map_err(|d| vec![d])?;
                    if index >= 16 {
                        return Err(vec![Diagnostic::error(
                            codes::OVERRIDE,
                            "renderer texture allocation exceeds S1's 16-entry table",
                        )]);
                    }
                    rows.push([index as f32, 0.0, 0.0, 0.0]);
                }
            }
        }
        Ok(PackedParameters { slots, rows })
    }
}
