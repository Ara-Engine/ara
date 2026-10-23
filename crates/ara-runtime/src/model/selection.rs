//! Node selection reuses immutable cooked primitives and restores the selected pivot.
use super::Model;
use crate::{Result, RuntimeError};
use std::ops::Range;

/// One node of the source glTF's default scene, including its drawable descendants.
#[derive(Debug, Clone)]
pub struct ModelNode {
    /// Zero-based glTF node index.
    pub index: u32,
    /// Source node name, empty when unnamed.
    pub name: String,
    /// Model-space pivot after source parent transforms.
    pub origin: [f32; 3],
    /// Contiguous drawable subtree in [`Model::primitives`].
    pub primitives: Range<usize>,
}

/// A resolved model/node/primitive selector, without duplicating geometry.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelSelection {
    /// Selected indices in [`Model::primitives`].
    pub primitives: Range<usize>,
    /// Subtree start before the optional primitive filter; override slots are relative to it.
    pub base: usize,
    /// Translation that places a selected node's source pivot at the entity origin.
    pub offset: [f32; 3],
}

impl Model {
    /// Validates selectors against this generation before presentation starts.
    pub fn validate_renderer(&self, renderer: &crate::components::MeshRenderer) -> Result<()> {
        self.select(renderer.node.as_deref(), renderer.primitive)?;
        if renderer.emissive_factor.is_some() {
            for selector in &renderer.emissive_materials {
                if !self.materials.iter().enumerate().any(|(index, material)| {
                    crate::components::matches_emissive_material(selector, index, &material.name)
                }) {
                    return Err(RuntimeError(format!(
                        "{}: no emissive material `{selector}`",
                        self.asset
                    )));
                }
            }
        }
        Ok(())
    }
    /// Selects a named node or numeric source index and an optional subtree-local
    /// primitive. `name:` disambiguates numeric node names. With no node, the whole
    /// default scene retains its authored placement. Node selections restore their
    /// pivot while retaining source rotation/scale and descendant transforms.
    pub fn select(&self, node: Option<&str>, primitive: Option<u32>) -> Result<ModelSelection> {
        let mut range = 0..self.primitives.len();
        let mut offset = [0.; 3];
        if let Some(selector) = node {
            let index = if let Some(name) = selector.strip_prefix("name:") {
                self.named_node(name)?
            } else if let Ok(index) = selector.parse::<u32>() {
                index
            } else {
                self.named_node(selector)?
            };
            let selected = self.nodes.get(&index).ok_or_else(|| {
                RuntimeError(format!("{}: no model node `{selector}`", self.asset))
            })?;
            range = selected.primitives.start..selected.primitives.end;
            offset = selected.origin.map(|v| -v);
        }
        let base = range.start;
        if let Some(index) = primitive {
            let index = index as usize;
            if index >= range.len() {
                return Err(RuntimeError(format!(
                    "{}: primitive {index} is outside the selected subtree ({} primitives)",
                    self.asset,
                    range.len()
                )));
            }
            range = base + index..base + index + 1;
        }
        if range.is_empty() {
            return Err(RuntimeError(format!(
                "{}: selected node has no drawable primitives",
                self.asset
            )));
        }
        Ok(ModelSelection {
            primitives: range,
            base,
            offset,
        })
    }

    fn named_node(&self, name: &str) -> Result<u32> {
        match self.node_names.get(name) {
            Some(Some(index)) => Ok(*index),
            Some(None) => Err(RuntimeError(format!(
                "{}: ambiguous node name `{name}`; use a numeric node index",
                self.asset
            ))),
            None => Err(RuntimeError(format!(
                "{}: no model node named `{name}`",
                self.asset
            ))),
        }
    }
}
